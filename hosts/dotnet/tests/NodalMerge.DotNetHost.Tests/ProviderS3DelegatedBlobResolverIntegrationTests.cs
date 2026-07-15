using NodalMerge.DotNetHost;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using System.Net;
using System.Net.Http;
using System.Net.Http.Json;
using System.Text;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Tests;

public sealed class ProviderS3DelegatedBlobResolverIntegrationTests
{
    // Canonical 64-lowercase-hex test hash (see BlobLayoutParityTests /
    // ProviderHttpEndpointTests for the same constant) — required since
    // slice S4.1 aligned /sync/blob-url's hash validation to
    // docs/BLOB_HTTP_SURFACE.md's canonical-hash rule.
    private const string CanonicalHash =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    [Fact]
    public async Task Sync_blob_url_returns_presigned_url_when_s3_delegated_resolver_returns_success()
    {
        var handler = new DelegatedSuccessHandler();
        await using var app = BuildTestApp(
            handler,
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                        ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://delegate.test"
                    }
                );
            }
        );
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync($"/sync/blob-url?op=put&room=room-a&namespace=assets&hash={CanonicalHash}&size=1024");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);

        Assert.Equal("https://upload.example/presigned", doc.RootElement.GetProperty("url").GetString());
        // Slice S4.1 aligned /sync/blob-url's response shape to the frozen
        // GET /blobs/{hash}/url contract (docs/BLOB_HTTP_SURFACE.md): an
        // ISO-8601 "expiresAtUtc" string, not a unix-seconds "expiresAt".
        // Protocol v1 itself still carries no expiry field (see
        // docs/BLOB_STORAGE_LAYOUT.md §7) — the resolver computes its own
        // from the configured TTL (default 900s), matching Rust's
        // PresignedUrl::with_ttl.
        var expiresAtUtc = DateTime.Parse(
            doc.RootElement.GetProperty("expiresAtUtc").GetString()!,
            null,
            System.Globalization.DateTimeStyles.RoundtripKind
        );
        var now = DateTime.UtcNow;
        Assert.InRange((expiresAtUtc - now).TotalSeconds, 890, 910);

        // Pin the actual outbound request shape against protocol v1.
        Assert.NotNull(handler.LastRequestBody);
        using var reqDoc = JsonDocument.Parse(handler.LastRequestBody!);
        var root = reqDoc.RootElement;
        Assert.Equal("put", root.GetProperty("op").GetString());
        Assert.Equal("room-a", root.GetProperty("room").GetString());
        Assert.Equal(CanonicalHash, root.GetProperty("hash").GetString());
        Assert.Equal("blake3", root.GetProperty("algorithm").GetString());
        Assert.Equal(1024, root.GetProperty("size").GetInt64());
        Assert.Equal(900, root.GetProperty("ttl_seconds").GetInt64());
        Assert.Equal("assets", root.GetProperty("namespace").GetString());
        Assert.False(root.TryGetProperty("room_id", out _), "protocol v1 uses `room`, not `room_id`");
    }

    [Fact]
    public async Task Sync_blob_url_returns_501_when_s3_delegated_resolver_fails()
    {
        await using var app = BuildTestApp(
            new DelegatedServerErrorHandler(),
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                        ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://delegate.test"
                    }
                );
            }
        );
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync($"/sync/blob-url?op=get&room=room-a&namespace=assets&hash={CanonicalHash}");

        // The frozen contract answers "no presign-capable backend" with 501,
        // not 404 (docs/BLOB_HTTP_SURFACE.md "Blob URL resolution").
        Assert.Equal(HttpStatusCode.NotImplemented, response.StatusCode);
    }

    [Fact]
    public async Task Sync_blob_url_retries_on_timeout_then_falls_back_to_501()
    {
        var handler = new DelegatedTimeoutHandler();
        await using var app = BuildTestApp(
            handler,
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                        ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://delegate.test",
                        ["NodalMerge:Storage:S3Delegated:MaxRetries"] = "2",
                        ["NodalMerge:Storage:S3Delegated:CircuitBreakerFailureThreshold"] = "10",
                        ["NodalMerge:Storage:S3Delegated:CircuitBreakerOpenSeconds"] = "60"
                    }
                );
            }
        );
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync($"/sync/blob-url?op=get&room=room-a&namespace=assets&hash={CanonicalHash}");

        Assert.Equal(HttpStatusCode.NotImplemented, response.StatusCode);
        Assert.Equal(3, handler.CallCount);
    }

    [Fact]
    public async Task Sync_blob_url_opens_circuit_after_5xx_threshold_and_short_circuits_follow_up_request()
    {
        var handler = new DelegatedServerErrorHandler();
        await using var app = BuildTestApp(
            handler,
            config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                        ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://delegate.test",
                        ["NodalMerge:Storage:S3Delegated:MaxRetries"] = "0",
                        ["NodalMerge:Storage:S3Delegated:CircuitBreakerFailureThreshold"] = "1",
                        ["NodalMerge:Storage:S3Delegated:CircuitBreakerOpenSeconds"] = "60"
                    }
                );
            }
        );
        await app.StartAsync();

        var client = app.GetTestClient();

        var first = await client.GetAsync($"/sync/blob-url?op=get&room=room-a&namespace=assets&hash={CanonicalHash}");
        Assert.Equal(HttpStatusCode.NotImplemented, first.StatusCode);
        Assert.Equal(1, handler.CallCount);

        var second = await client.GetAsync($"/sync/blob-url?op=get&room=room-a&namespace=assets&hash={CanonicalHash}");
        Assert.Equal(HttpStatusCode.NotImplemented, second.StatusCode);
        Assert.Equal(1, handler.CallCount);
    }

    private static WebApplication BuildTestApp(
        HttpMessageHandler delegatedHandler,
        Action<IConfigurationBuilder> configureConfiguration
    )
    {
        return HostApplication.Build(
            [],
            configureServices: services =>
            {
                services.AddSingleton<IHttpClientFactory>(new FakeDelegatedHttpClientFactory(delegatedHandler));
            },
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg => configureConfiguration(cfg)
        );
    }
}

internal sealed class FakeDelegatedHttpClientFactory : IHttpClientFactory
{
    private readonly HttpClient _client;

    public FakeDelegatedHttpClientFactory(HttpMessageHandler handler)
    {
        _client = new HttpClient(handler)
        {
            BaseAddress = new Uri("https://delegate.test")
        };
    }

    public HttpClient CreateClient(string name)
    {
        return _client;
    }
}

internal sealed class DelegatedSuccessHandler : HttpMessageHandler
{
    public string? LastRequestBody { get; private set; }

    protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        LastRequestBody = request.Content is null
            ? null
            : await request.Content.ReadAsStringAsync(cancellationToken);

        return new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent("{\"url\":\"https://upload.example/presigned\"}", Encoding.UTF8, "application/json")
        };
    }
}

internal sealed class DelegatedServerErrorHandler : HttpMessageHandler
{
    public int CallCount { get; private set; }

    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        CallCount++;
        return Task.FromResult(new HttpResponseMessage(HttpStatusCode.InternalServerError)
        {
            Content = new StringContent("delegate failure", Encoding.UTF8, "text/plain")
        });
    }
}

internal sealed class DelegatedTimeoutHandler : HttpMessageHandler
{
    public int CallCount { get; private set; }

    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        CallCount++;
        throw new TaskCanceledException("simulated timeout");
    }
}
