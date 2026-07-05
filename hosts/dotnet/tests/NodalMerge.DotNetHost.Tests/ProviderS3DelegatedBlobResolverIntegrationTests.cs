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
    [Fact]
    public async Task Sync_blob_url_returns_presigned_url_when_s3_delegated_resolver_returns_success()
    {
        await using var app = BuildTestApp(
            new DelegatedSuccessHandler(),
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
        var response = await client.GetAsync("/sync/blob-url?op=put&room=room-a&namespace=assets&hash=sha256%3Aabc&size=1024");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);

        Assert.Equal("https://upload.example/presigned", doc.RootElement.GetProperty("url").GetString());
        Assert.Equal(1700000123L, doc.RootElement.GetProperty("expiresAt").GetInt64());
    }

    [Fact]
    public async Task Sync_blob_url_returns_404_when_s3_delegated_resolver_fails()
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
        var response = await client.GetAsync("/sync/blob-url?op=get&room=room-a&namespace=assets&hash=sha256%3Aabc");

        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    [Fact]
    public async Task Sync_blob_url_retries_on_timeout_then_falls_back_to_404()
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
        var response = await client.GetAsync("/sync/blob-url?op=get&room=room-a&namespace=assets&hash=sha256%3Aabc");

        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
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

        var first = await client.GetAsync("/sync/blob-url?op=get&room=room-a&namespace=assets&hash=sha256%3Aabc");
        Assert.Equal(HttpStatusCode.NotFound, first.StatusCode);
        Assert.Equal(1, handler.CallCount);

        var second = await client.GetAsync("/sync/blob-url?op=get&room=room-a&namespace=assets&hash=sha256%3Aabc");
        Assert.Equal(HttpStatusCode.NotFound, second.StatusCode);
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
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent("{\"url\":\"https://upload.example/presigned\",\"expires_at_epoch_seconds\":1700000123}", Encoding.UTF8, "application/json")
        });
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
