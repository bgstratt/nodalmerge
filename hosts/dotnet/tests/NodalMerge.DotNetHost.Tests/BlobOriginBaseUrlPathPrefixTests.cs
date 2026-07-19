using System.Net;
using System.Text;
using Microsoft.Extensions.Logging.Abstractions;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 4.3 (blob-cas-remediation.md): a reverse-proxy path prefix in
/// <see cref="RemoteBlobOriginOptions.BaseUrl"/> (e.g.
/// <c>https://host/nodalmerge</c>) must survive into every request
/// <see cref="HttpRemoteBlobStoreProvider"/> and
/// <see cref="S3DirectBlobStoreProvider"/> send — pre-fix, root-relative
/// request paths (<c>/blobs/{hash}</c>) silently dropped it, so every request
/// landed on <c>/blobs/{hash}</c> instead of <c>/nodalmerge/blobs/{hash}</c>.
/// Reuses the fake-handler harness style from
/// <c>HttpRemoteBlobStoreProviderTests.cs</c> / <c>S3DirectBlobStoreProviderTests.cs</c>.
///
/// Covers BOTH trailing-slash spellings of the prefix
/// (<c>https://host/nodalmerge</c> and <c>.../nodalmerge/</c>) — RFC 3986's
/// relative-reference merge treats a base path with no trailing "/" as ending
/// in a "file" segment a relative reference REPLACES, so a fix that only
/// dropped the leading "/" from request paths (without also normalizing the
/// base) would still lose the prefix for the no-trailing-slash spelling. See
/// <see cref="RemoteBlobOriginOptions.ResolveBaseUri"/>'s doc for the exact
/// trap.
///
/// <see cref="HttpClient.BaseAddress"/> is deliberately set to the RAW
/// configured <c>BaseUrl</c> in every test here (no test-side trailing-slash
/// normalization) — proving the fix lives entirely in the providers' own
/// absolute-URI construction, not in ambient <c>HttpClient</c> hygiene the
/// composition root happens to apply.
/// </summary>
public sealed class BlobOriginBaseUrlPathPrefixTests
{
    private static readonly string Hash = new('a', 64);

    [Theory]
    [InlineData("https://origin.test/nodalmerge")]
    [InlineData("https://origin.test/nodalmerge/")]
    public async Task HttpRemoteBlobStoreProvider_get_reaches_prefixed_path(string baseUrl)
    {
        HttpRequestMessage? captured = null;
        var handler = new CapturingRequestHandler(req =>
        {
            captured = req;
            return new HttpResponseMessage(HttpStatusCode.NotFound);
        });
        var provider = BuildHttpRemoteProvider(baseUrl, handler);

        await provider.TryGetBlobAsync(Hash);

        Assert.NotNull(captured);
        Assert.Equal($"/nodalmerge/blobs/{Hash}", captured!.RequestUri!.AbsolutePath);
    }

    [Theory]
    [InlineData("https://origin.test/nodalmerge")]
    [InlineData("https://origin.test/nodalmerge/")]
    public async Task HttpRemoteBlobStoreProvider_put_reaches_prefixed_path(string baseUrl)
    {
        HttpRequestMessage? captured = null;
        var handler = new CapturingRequestHandler(req =>
        {
            captured = req;
            return new HttpResponseMessage(HttpStatusCode.OK);
        });
        var provider = BuildHttpRemoteProvider(baseUrl, handler);

        await provider.PutBlobAsync(Hash, [1, 2, 3], null);

        Assert.NotNull(captured);
        Assert.Equal($"/nodalmerge/blobs/{Hash}", captured!.RequestUri!.AbsolutePath);
    }

    [Theory]
    [InlineData("https://origin.test/nodalmerge")]
    [InlineData("https://origin.test/nodalmerge/")]
    public async Task HttpRemoteBlobStoreProvider_exists_head_reaches_prefixed_path(string baseUrl)
    {
        HttpRequestMessage? captured = null;
        var handler = new CapturingRequestHandler(req =>
        {
            captured = req;
            return new HttpResponseMessage(HttpStatusCode.OK);
        });
        var provider = BuildHttpRemoteProvider(baseUrl, handler);

        await provider.ExistsAsync(Hash);

        Assert.NotNull(captured);
        Assert.Equal(HttpMethod.Head, captured!.Method);
        Assert.Equal($"/nodalmerge/blobs/{Hash}", captured.RequestUri!.AbsolutePath);
    }

    /// <summary>Regression guard: an un-prefixed BaseUrl must behave exactly as before.</summary>
    [Fact]
    public async Task HttpRemoteBlobStoreProvider_unprefixed_BaseUrl_is_unaffected()
    {
        HttpRequestMessage? captured = null;
        var handler = new CapturingRequestHandler(req =>
        {
            captured = req;
            return new HttpResponseMessage(HttpStatusCode.NotFound);
        });
        var provider = BuildHttpRemoteProvider("https://origin.test", handler);

        await provider.TryGetBlobAsync(Hash);

        Assert.NotNull(captured);
        Assert.Equal($"/blobs/{Hash}", captured!.RequestUri!.AbsolutePath);
    }

    [Theory]
    [InlineData("https://origin.test/nodalmerge")]
    [InlineData("https://origin.test/nodalmerge/")]
    public async Task S3DirectBlobStoreProvider_url_resolution_reaches_prefixed_path(string baseUrl)
    {
        HttpRequestMessage? captured = null;
        var origin = new CapturingRequestHandler(req =>
        {
            captured = req;
            return new HttpResponseMessage(HttpStatusCode.NotImplemented);
        });
        var bucket = new CapturingRequestHandler(_ => new HttpResponseMessage(HttpStatusCode.OK));
        var provider = BuildS3DirectProvider(baseUrl, origin, bucket);

        await provider.TryGetBlobAsync(Hash);

        Assert.NotNull(captured);
        Assert.Equal($"/nodalmerge/blobs/{Hash}/url", captured!.RequestUri!.AbsolutePath);
    }

    [Theory]
    [InlineData("https://origin.test/nodalmerge")]
    [InlineData("https://origin.test/nodalmerge/")]
    public async Task S3DirectBlobStoreProvider_uploaded_confirm_reaches_prefixed_path(string baseUrl)
    {
        var capturedRequests = new List<HttpRequestMessage>();
        var origin = new SequencedHandler(
            _ => JsonResponse(HttpStatusCode.OK, """{"url":"https://bucket.test/obj","expiresAtUtc":"2026-07-16T00:00:00Z"}"""),
            req =>
            {
                capturedRequests.Add(req);
                return new HttpResponseMessage(HttpStatusCode.OK);
            }
        );
        var bucket = new CapturingRequestHandler(_ => new HttpResponseMessage(HttpStatusCode.OK));
        var provider = BuildS3DirectProvider(baseUrl, origin, bucket);

        await provider.PutBlobAsync(Hash, [1, 2, 3], null);

        var confirmRequest = Assert.Single(capturedRequests);
        Assert.Equal($"/nodalmerge/blobs/{Hash}/uploaded", confirmRequest.RequestUri!.AbsolutePath);
    }

    private static HttpRemoteBlobStoreProvider BuildHttpRemoteProvider(string baseUrl, HttpMessageHandler handler)
    {
        var client = new HttpClient(handler) { BaseAddress = new Uri(baseUrl, UriKind.Absolute) };
        var factory = new FakeHttpClientFactory(client);
        var options = new RemoteBlobOriginOptions(
            BaseUrl: baseUrl,
            AuthToken: null,
            TimeoutSeconds: 30,
            MaxRetries: 0,
            CircuitBreakerFailureThreshold: 3,
            CircuitBreakerOpenSeconds: 30
        );
        return new HttpRemoteBlobStoreProvider(factory, options, NullLogger<HttpRemoteBlobStoreProvider>.Instance);
    }

    private static S3DirectBlobStoreProvider BuildS3DirectProvider(
        string baseUrl,
        HttpMessageHandler originHandler,
        HttpMessageHandler bucketHandler
    )
    {
        var originClient = new HttpClient(originHandler) { BaseAddress = new Uri(baseUrl, UriKind.Absolute) };
        var bucketClient = new HttpClient(bucketHandler);
        var factory = new NamedFakeHttpClientFactory(new Dictionary<string, HttpClient>
        {
            [HttpRemoteBlobStoreProvider.HttpClientName] = originClient,
            [S3DirectBlobStoreProvider.BucketHttpClientName] = bucketClient
        });

        var originOptions = new RemoteBlobOriginOptions(
            BaseUrl: baseUrl,
            AuthToken: null,
            TimeoutSeconds: 30,
            MaxRetries: 0,
            CircuitBreakerFailureThreshold: 3,
            CircuitBreakerOpenSeconds: 30
        );
        var s3DirectOptions = new S3DirectBlobOriginOptions(
            Enabled: true,
            TimeoutSeconds: 30,
            MaxRetries: 0,
            CircuitBreakerFailureThreshold: 3,
            CircuitBreakerOpenSeconds: 30,
            CapabilityProbeCooldownSeconds: 0,
            Compression: "Off",
            CompressionLevel: 3,
            CompressionMinBytes: 4096
        );

        return new S3DirectBlobStoreProvider(
            factory,
            originOptions,
            s3DirectOptions,
            NullLogger<S3DirectBlobStoreProvider>.Instance
        );
    }

    private static HttpResponseMessage JsonResponse(HttpStatusCode status, string json)
    {
        return new HttpResponseMessage(status)
        {
            Content = new StringContent(json, Encoding.UTF8, "application/json")
        };
    }

    private sealed class FakeHttpClientFactory(HttpClient client) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => client;
    }

    private sealed class NamedFakeHttpClientFactory(IReadOnlyDictionary<string, HttpClient> clients) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) =>
            clients.TryGetValue(name, out var client)
                ? client
                : throw new InvalidOperationException($"no fake HttpClient registered for '{name}'");
    }

    private sealed class CapturingRequestHandler(Func<HttpRequestMessage, HttpResponseMessage> respond) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
            => Task.FromResult(respond(request));
    }

    private sealed class SequencedHandler : HttpMessageHandler
    {
        private readonly Queue<Func<HttpRequestMessage, HttpResponseMessage>> _responders;

        public SequencedHandler(params Func<HttpRequestMessage, HttpResponseMessage>[] responders)
        {
            _responders = new Queue<Func<HttpRequestMessage, HttpResponseMessage>>(responders);
        }

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
            => Task.FromResult(_responders.Dequeue()(request));
    }
}
