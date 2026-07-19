using System.Net;
using System.Net.Http.Headers;
using Microsoft.Extensions.Logging.Abstractions;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Drives <see cref="HttpRemoteBlobStoreProvider"/> directly against a fake
/// handler — no DI, no real network — covering the retry/circuit-breaker
/// policy and the frozen client-behavior contract (docs/BLOB_HTTP_SURFACE.md).
/// </summary>
public sealed class HttpRemoteBlobStoreProviderTests
{
    private const string BaseUrl = "https://origin.test";
    private static readonly string Hash = new('a', 64);

    [Fact]
    public async Task Get_200_returns_bytes()
    {
        var bytes = new byte[] { 1, 2, 3 };
        var handler = new QueueHandler([MakeResponse(HttpStatusCode.OK, bytes)]);
        var provider = BuildProvider(handler);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(bytes, result.Bytes);
        Assert.Equal(1, handler.CallCount);
    }

    [Fact]
    public async Task Get_404_returns_missing()
    {
        var handler = new QueueHandler([MakeResponse(HttpStatusCode.NotFound)]);
        var provider = BuildProvider(handler);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.False(result.Found);
    }

    [Fact]
    public async Task Get_500_then_200_is_retried_to_success()
    {
        var bytes = new byte[] { 4, 5, 6 };
        var handler = new QueueHandler([
            MakeResponse(HttpStatusCode.InternalServerError),
            MakeResponse(HttpStatusCode.OK, bytes)
        ]);
        var provider = BuildProvider(handler, maxRetries: 2);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(bytes, result.Bytes);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task Breaker_opens_after_threshold_and_short_circuits_subsequent_get()
    {
        var handler = new QueueHandler(() => MakeResponse(HttpStatusCode.InternalServerError));
        var provider = BuildProvider(handler, maxRetries: 0, circuitThreshold: 1);

        // First call exhausts its single attempt (no retries configured),
        // throws, and that failure is what opens the breaker (threshold=1).
        await Assert.ThrowsAsync<InvalidOperationException>(() => provider.TryGetBlobAsync(Hash).AsTask());
        Assert.Equal(1, handler.CallCount);

        // Second call: breaker is open, so it degrades to a miss WITHOUT
        // making another HTTP call.
        var second = await provider.TryGetBlobAsync(Hash);
        Assert.False(second.Found);
        Assert.Equal(1, handler.CallCount);
    }

    [Fact]
    public async Task Bearer_header_present_when_token_configured()
    {
        AuthenticationHeaderValue? capturedAuth = null;
        var handler = new CapturingRequestHandler(req =>
        {
            capturedAuth = req.Headers.Authorization;
            return MakeResponse(HttpStatusCode.OK, [1]);
        });
        var provider = BuildProvider(handler, authToken: "secret-token");

        await provider.TryGetBlobAsync(Hash);

        Assert.NotNull(capturedAuth);
        Assert.Equal("Bearer", capturedAuth!.Scheme);
        Assert.Equal("secret-token", capturedAuth.Parameter);
    }

    [Fact]
    public async Task No_authorization_header_when_token_not_configured()
    {
        AuthenticationHeaderValue? capturedAuth = null;
        var handler = new CapturingRequestHandler(req =>
        {
            capturedAuth = req.Headers.Authorization;
            return MakeResponse(HttpStatusCode.OK, [1]);
        });
        var provider = BuildProvider(handler);

        await provider.TryGetBlobAsync(Hash);

        Assert.Null(capturedAuth);
    }

    [Fact]
    public async Task Put_422_throws_immediately_without_retry()
    {
        var handler = new QueueHandler([MakeResponse(HttpStatusCode.UnprocessableEntity)]);
        var provider = BuildProvider(handler, maxRetries: 3);

        await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1], null).AsTask()
        );
        Assert.Equal(1, handler.CallCount);
    }

    [Fact]
    public async Task Put_200_and_201_are_both_success()
    {
        var okHandler = new QueueHandler([MakeResponse(HttpStatusCode.OK)]);
        await BuildProvider(okHandler).PutBlobAsync(Hash, [1], null);

        var createdHandler = new QueueHandler([MakeResponse(HttpStatusCode.Created)]);
        await BuildProvider(createdHandler).PutBlobAsync(Hash, [1], null);
    }

    [Fact]
    public async Task Head_200_and_404_map_to_true_and_false()
    {
        var foundHandler = new QueueHandler([MakeResponse(HttpStatusCode.OK)]);
        Assert.True(await BuildProvider(foundHandler).ExistsAsync(Hash));

        var missingHandler = new QueueHandler([MakeResponse(HttpStatusCode.NotFound)]);
        Assert.False(await BuildProvider(missingHandler).ExistsAsync(Hash));
    }

    private static HttpRemoteBlobStoreProvider BuildProvider(
        HttpMessageHandler handler,
        int maxRetries = 2,
        int circuitThreshold = 3,
        string? authToken = null
    )
    {
        var client = new HttpClient(handler) { BaseAddress = new Uri(BaseUrl) };
        var factory = new FakeHttpClientFactory(client);
        var options = new RemoteBlobOriginOptions(
            BaseUrl: BaseUrl,
            AuthToken: authToken,
            TimeoutSeconds: 30,
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitThreshold,
            CircuitBreakerOpenSeconds: 30
        );
        return new HttpRemoteBlobStoreProvider(factory, options, NullLogger<HttpRemoteBlobStoreProvider>.Instance);
    }

    private static HttpResponseMessage MakeResponse(HttpStatusCode status, byte[]? body = null)
    {
        var response = new HttpResponseMessage(status);
        if (body is not null)
        {
            response.Content = new ByteArrayContent(body);
        }
        return response;
    }

    private sealed class FakeHttpClientFactory(HttpClient client) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => client;
    }

    private sealed class QueueHandler : HttpMessageHandler
    {
        private readonly Queue<HttpResponseMessage>? _queue;
        private readonly Func<HttpResponseMessage>? _factory;

        public int CallCount { get; private set; }

        public QueueHandler(IEnumerable<HttpResponseMessage> responses)
        {
            _queue = new Queue<HttpResponseMessage>(responses);
        }

        public QueueHandler(Func<HttpResponseMessage> factory)
        {
            _factory = factory;
        }

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            CallCount++;
            var response = _factory is not null ? _factory() : _queue!.Dequeue();
            return Task.FromResult(response);
        }
    }

    private sealed class CapturingRequestHandler(Func<HttpRequestMessage, HttpResponseMessage> respond) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            return Task.FromResult(respond(request));
        }
    }
}
