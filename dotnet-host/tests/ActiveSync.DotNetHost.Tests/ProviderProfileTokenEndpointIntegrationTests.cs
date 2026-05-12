using ActiveSync.DotNetHost;
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

namespace ActiveSync.DotNetHost.Tests;

public sealed class ProviderProfileTokenEndpointIntegrationTests
{
    [Fact]
    public async Task Sync_token_returns_501_in_default_profile()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "Default"
                }
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room = "room-a",
                peerPubkeyHex = "peer-a"
            }
        );

        Assert.Equal(HttpStatusCode.NotImplemented, response.StatusCode);
    }

    [Fact]
    public async Task Sync_token_mints_and_validates_in_jwt_bridge_embedded_profile()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["ActiveSync:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["ActiveSync:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["ActiveSync:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                }
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var mintResponse = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room = "room-a",
                peerPubkeyHex = "peer-a",
                lifetimeSeconds = 120,
                capabilities = new[] { "read:**", "write:world/**" }
            }
        );

        Assert.Equal(HttpStatusCode.OK, mintResponse.StatusCode);

        using var mintStream = await mintResponse.Content.ReadAsStreamAsync();
        using var mintDoc = await JsonDocument.ParseAsync(mintStream);

        var peer = mintDoc.RootElement.GetProperty("peer_pubkey_hex").GetString();
        var expiry = mintDoc.RootElement.GetProperty("expiry_secs").GetInt64();
        var sig = mintDoc.RootElement.GetProperty("sig_hex").GetString();
        var caps = mintDoc.RootElement.GetProperty("capabilities")
            .EnumerateArray()
            .Select(x => x.GetString())
            .Where(x => !string.IsNullOrWhiteSpace(x))
            .Cast<string>()
            .ToArray();

        Assert.Equal("peer-a", peer);
        Assert.True(expiry > DateTimeOffset.UtcNow.ToUnixTimeSeconds());
        Assert.NotNull(sig);
        Assert.Contains('.', sig!);

        var validateResponse = await client.PostAsJsonAsync(
            "/sync/token/validate",
            new
            {
                room = "room-a",
                peer_pubkey_hex = peer,
                expiry_secs = expiry,
                capabilities = caps,
                sig_hex = sig
            }
        );

        Assert.Equal(HttpStatusCode.OK, validateResponse.StatusCode);
        using var validateStream = await validateResponse.Content.ReadAsStreamAsync();
        using var validateDoc = await JsonDocument.ParseAsync(validateStream);

        Assert.True(validateDoc.RootElement.GetProperty("valid").GetBoolean());
        if (validateDoc.RootElement.TryGetProperty("reason", out var reason))
        {
            Assert.True(reason.ValueKind == JsonValueKind.Null || reason.GetString() is null);
        }
    }

    [Fact]
    public async Task Sync_token_mints_and_validates_in_jwt_bridge_sidecar_profile()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeSidecar",
                    ["ActiveSync:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["ActiveSync:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
                }
            );
        }, services =>
        {
            services.AddSingleton<IHttpClientFactory>(
                new FakeHttpClientFactory(new SidecarSuccessHandler())
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var mintResponse = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room = "room-a",
                peerPubkeyHex = "peer-a",
                lifetimeSeconds = 120,
                capabilities = new[] { "read:**", "write:world/**" }
            }
        );

        Assert.Equal(HttpStatusCode.OK, mintResponse.StatusCode);
        using var mintStream = await mintResponse.Content.ReadAsStreamAsync();
        using var mintDoc = await JsonDocument.ParseAsync(mintStream);

        var peer = mintDoc.RootElement.GetProperty("peer_pubkey_hex").GetString();
        var expiry = mintDoc.RootElement.GetProperty("expiry_secs").GetInt64();
        var sig = mintDoc.RootElement.GetProperty("sig_hex").GetString();
        var caps = mintDoc.RootElement.GetProperty("capabilities")
            .EnumerateArray()
            .Select(x => x.GetString())
            .Where(x => !string.IsNullOrWhiteSpace(x))
            .Cast<string>()
            .ToArray();

        Assert.Equal("peer-a", peer);
        Assert.Equal(1700000123L, expiry);
        Assert.Equal("sidecar-sig", sig);

        var validateResponse = await client.PostAsJsonAsync(
            "/sync/token/validate",
            new
            {
                room = "room-a",
                peer_pubkey_hex = peer,
                expiry_secs = expiry,
                capabilities = caps,
                sig_hex = sig
            }
        );

        Assert.Equal(HttpStatusCode.OK, validateResponse.StatusCode);
        using var validateStream = await validateResponse.Content.ReadAsStreamAsync();
        using var validateDoc = await JsonDocument.ParseAsync(validateStream);
        Assert.True(validateDoc.RootElement.GetProperty("valid").GetBoolean());
    }

    [Fact]
    public async Task Sync_token_returns_504_when_jwt_bridge_sidecar_times_out()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeSidecar",
                    ["ActiveSync:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["ActiveSync:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
                }
            );
        }, services =>
        {
            services.AddSingleton<IHttpClientFactory>(
                new FakeHttpClientFactory(new SidecarTimeoutHandler())
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room = "room-a",
                peerPubkeyHex = "peer-a"
            }
        );

        Assert.Equal(HttpStatusCode.GatewayTimeout, response.StatusCode);
    }

    [Fact]
    public async Task Sync_token_returns_502_when_jwt_bridge_sidecar_returns_5xx()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeSidecar",
                    ["ActiveSync:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["ActiveSync:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
                }
            );
        }, services =>
        {
            services.AddSingleton<IHttpClientFactory>(
                new FakeHttpClientFactory(new SidecarServerErrorHandler())
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room = "room-a",
                peerPubkeyHex = "peer-a"
            }
        );

        Assert.Equal(HttpStatusCode.BadGateway, response.StatusCode);
    }

    [Fact]
    public async Task Sync_token_validate_returns_504_when_jwt_bridge_sidecar_times_out()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeSidecar",
                    ["ActiveSync:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["ActiveSync:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
                }
            );
        }, services =>
        {
            services.AddSingleton<IHttpClientFactory>(
                new FakeHttpClientFactory(new SidecarTimeoutHandler())
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/sync/token/validate",
            new
            {
                room = "room-a",
                peer_pubkey_hex = "peer-a",
                expiry_secs = 1700000123,
                capabilities = new[] { "read:**" },
                sig_hex = "sidecar-sig"
            }
        );

        Assert.Equal(HttpStatusCode.GatewayTimeout, response.StatusCode);
    }

    [Fact]
    public async Task Sync_token_validate_returns_502_when_jwt_bridge_sidecar_returns_5xx()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeSidecar",
                    ["ActiveSync:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["ActiveSync:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
                }
            );
        }, services =>
        {
            services.AddSingleton<IHttpClientFactory>(
                new FakeHttpClientFactory(new SidecarServerErrorHandler())
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/sync/token/validate",
            new
            {
                room = "room-a",
                peer_pubkey_hex = "peer-a",
                expiry_secs = 1700000123,
                capabilities = new[] { "read:**" },
                sig_hex = "sidecar-sig"
            }
        );

        Assert.Equal(HttpStatusCode.BadGateway, response.StatusCode);
    }

    [Fact]
    public async Task Sync_token_returns_501_when_jwt_bridge_sidecar_mint_is_not_supported()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["ActiveSync:Providers:Auth"] = "JwtBridgeSidecar",
                    ["ActiveSync:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["ActiveSync:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
                }
            );
        }, services =>
        {
            services.AddSingleton<IHttpClientFactory>(
                new FakeHttpClientFactory(new SidecarMintNotSupportedHandler())
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room = "room-a",
                peerPubkeyHex = "peer-a"
            }
        );

        Assert.Equal(HttpStatusCode.NotImplemented, response.StatusCode);
    }

    private static WebApplication BuildTestApp(
        Action<IConfigurationBuilder>? configureConfiguration = null,
        Action<IServiceCollection>? configureServices = null
    )
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg => configureConfiguration?.Invoke(cfg)
        );
    }
}

internal sealed class FakeHttpClientFactory : IHttpClientFactory
{
    private readonly HttpClient _client;

    public FakeHttpClientFactory(HttpMessageHandler handler)
    {
        _client = new HttpClient(handler)
        {
            BaseAddress = new Uri("https://sidecar.test")
        };
    }

    public HttpClient CreateClient(string name)
    {
        return _client;
    }
}

internal sealed class SidecarSuccessHandler : HttpMessageHandler
{
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        if (request.RequestUri?.AbsolutePath.EndsWith("/mint", StringComparison.OrdinalIgnoreCase) == true)
        {
            return Task.FromResult(BuildJsonResponse(
                HttpStatusCode.OK,
                "{\"peerPubkeyHex\":\"peer-a\",\"expiryUnixSeconds\":1700000123,\"capabilities\":[\"read:**\",\"write:world/**\"],\"signatureHex\":\"sidecar-sig\"}"
            ));
        }

        if (request.RequestUri?.AbsolutePath.EndsWith("/validate", StringComparison.OrdinalIgnoreCase) == true)
        {
            return Task.FromResult(BuildJsonResponse(
                HttpStatusCode.OK,
                "{\"valid\":true,\"reason\":null}"
            ));
        }

        return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotFound));
    }

    private static HttpResponseMessage BuildJsonResponse(HttpStatusCode statusCode, string json)
    {
        return new HttpResponseMessage(statusCode)
        {
            Content = new StringContent(json, Encoding.UTF8, "application/json")
        };
    }
}

internal sealed class SidecarTimeoutHandler : HttpMessageHandler
{
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        throw new TaskCanceledException("simulated timeout");
    }
}

internal sealed class SidecarServerErrorHandler : HttpMessageHandler
{
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        return Task.FromResult(new HttpResponseMessage(HttpStatusCode.InternalServerError)
        {
            Content = new StringContent("sidecar failure", Encoding.UTF8, "text/plain")
        });
    }
}

internal sealed class SidecarMintNotSupportedHandler : HttpMessageHandler
{
    protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
    {
        if (request.RequestUri?.AbsolutePath.EndsWith("/mint", StringComparison.OrdinalIgnoreCase) == true)
        {
            return Task.FromResult(new HttpResponseMessage(HttpStatusCode.NotImplemented));
        }

        return Task.FromResult(new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent("{\"valid\":true,\"reason\":null}", Encoding.UTF8, "application/json")
        });
    }
}
