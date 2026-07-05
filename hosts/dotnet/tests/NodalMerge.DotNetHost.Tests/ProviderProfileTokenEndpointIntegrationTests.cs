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
                    ["NodalMerge:Providers:Auth"] = "Default"
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
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
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
    public async Task Sync_token_validate_rejects_embedded_token_when_issuer_mismatches()
    {
        await using var issuerAApp = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "issuer-a",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                }
            );
        });
        await issuerAApp.StartAsync();

        var minted = await MintTokenAsync(issuerAApp.GetTestClient(), "room-a", "peer-a", 120, ["read:**"]);

        await using var issuerBApp = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "issuer-b",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                }
            );
        });
        await issuerBApp.StartAsync();

        var validate = await ValidateTokenAsync(
            issuerBApp.GetTestClient(),
            "room-a",
            minted.PeerPubkeyHex,
            minted.ExpiryUnixSeconds,
            minted.Capabilities,
            minted.SignatureHex
        );

        Assert.False(validate.Valid);
        Assert.Equal("invalid embedded token", validate.Reason);
    }

    [Fact]
    public async Task Sync_token_validate_allows_previous_key_during_embedded_rotation_overlap()
    {
        const string oldKey = "old-signing-key-1234567890-abcdef1234";
        const string newKey = "new-signing-key-1234567890-abcdef1234";

        await using var oldKeyApp = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = oldKey
                }
            );
        });
        await oldKeyApp.StartAsync();

        var minted = await MintTokenAsync(oldKeyApp.GetTestClient(), "room-a", "peer-a", 120, ["read:**"]);

        await using var overlapApp = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = newKey,
                    ["NodalMerge:Auth:JwtBridgeEmbedded:PreviousSigningKeys:0"] = oldKey,
                    ["NodalMerge:Auth:JwtBridgeEmbedded:ClockSkewSeconds"] = "0"
                }
            );
        });
        await overlapApp.StartAsync();

        var validate = await ValidateTokenAsync(
            overlapApp.GetTestClient(),
            "room-a",
            minted.PeerPubkeyHex,
            minted.ExpiryUnixSeconds,
            minted.Capabilities,
            minted.SignatureHex
        );

        Assert.True(validate.Valid);
        Assert.Null(validate.Reason);
    }

    [Fact]
    public async Task Sync_token_validate_rejects_previous_key_when_overlap_not_configured()
    {
        const string oldKey = "old-signing-key-1234567890-abcdef1234";
        const string newKey = "new-signing-key-1234567890-abcdef1234";

        await using var oldKeyApp = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = oldKey
                }
            );
        });
        await oldKeyApp.StartAsync();

        var minted = await MintTokenAsync(oldKeyApp.GetTestClient(), "room-a", "peer-a", 120, ["read:**"]);

        await using var rotatedNoOverlapApp = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = newKey,
                    ["NodalMerge:Auth:JwtBridgeEmbedded:ClockSkewSeconds"] = "0"
                }
            );
        });
        await rotatedNoOverlapApp.StartAsync();

        var validate = await ValidateTokenAsync(
            rotatedNoOverlapApp.GetTestClient(),
            "room-a",
            minted.PeerPubkeyHex,
            minted.ExpiryUnixSeconds,
            minted.Capabilities,
            minted.SignatureHex
        );

        Assert.False(validate.Valid);
        Assert.Equal("invalid embedded token", validate.Reason);
    }

    [Fact]
    public async Task Sync_token_validate_rejects_when_embedded_token_capability_set_differs()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeEmbedded",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Issuer"] = "test-issuer",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:Audience"] = "test-audience",
                    ["NodalMerge:Auth:JwtBridgeEmbedded:SigningKey"] = "test-signing-key-1234567890-abcdef"
                }
            );
        });
        await app.StartAsync();

        var minted = await MintTokenAsync(app.GetTestClient(), "room-a", "peer-a", 120, ["read:**", "write:world/**"]);
        var validate = await ValidateTokenAsync(
            app.GetTestClient(),
            "room-a",
            minted.PeerPubkeyHex,
            minted.ExpiryUnixSeconds,
            ["read:**"],
            minted.SignatureHex
        );

        Assert.False(validate.Valid);
        Assert.Equal("capability mismatch", validate.Reason);
    }

    [Fact]
    public async Task Sync_token_mints_and_validates_in_jwt_bridge_sidecar_profile()
    {
        await using var app = BuildTestApp(config =>
        {
            config.AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeSidecar",
                    ["NodalMerge:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["NodalMerge:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
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
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeSidecar",
                    ["NodalMerge:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["NodalMerge:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
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
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeSidecar",
                    ["NodalMerge:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["NodalMerge:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
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
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeSidecar",
                    ["NodalMerge:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["NodalMerge:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
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
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeSidecar",
                    ["NodalMerge:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["NodalMerge:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
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
                    ["NodalMerge:Providers:Auth"] = "JwtBridgeSidecar",
                    ["NodalMerge:Auth:JwtBridgeSidecar:BaseUrl"] = "https://sidecar.test",
                    ["NodalMerge:Auth:JwtBridgeSidecar:TimeoutSeconds"] = "5"
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

    private static async Task<(string PeerPubkeyHex, long ExpiryUnixSeconds, string SignatureHex, string[] Capabilities)> MintTokenAsync(
        HttpClient client,
        string room,
        string peer,
        int lifetimeSeconds,
        string[] capabilities)
    {
        var mintResponse = await client.PostAsJsonAsync(
            "/sync/token",
            new
            {
                room,
                peerPubkeyHex = peer,
                lifetimeSeconds,
                capabilities
            }
        );
        Assert.Equal(HttpStatusCode.OK, mintResponse.StatusCode);

        using var mintStream = await mintResponse.Content.ReadAsStreamAsync();
        using var mintDoc = await JsonDocument.ParseAsync(mintStream);

        var mintedCaps = mintDoc.RootElement.GetProperty("capabilities")
            .EnumerateArray()
            .Select(x => x.GetString())
            .Where(x => !string.IsNullOrWhiteSpace(x))
            .Cast<string>()
            .ToArray();

        return (
            mintDoc.RootElement.GetProperty("peer_pubkey_hex").GetString()!,
            mintDoc.RootElement.GetProperty("expiry_secs").GetInt64(),
            mintDoc.RootElement.GetProperty("sig_hex").GetString()!,
            mintedCaps
        );
    }

    private static async Task<(bool Valid, string? Reason)> ValidateTokenAsync(
        HttpClient client,
        string room,
        string peer,
        long expiry,
        string[] capabilities,
        string signature)
    {
        var validateResponse = await client.PostAsJsonAsync(
            "/sync/token/validate",
            new
            {
                room,
                peer_pubkey_hex = peer,
                expiry_secs = expiry,
                capabilities,
                sig_hex = signature
            }
        );

        Assert.Equal(HttpStatusCode.OK, validateResponse.StatusCode);
        using var validateStream = await validateResponse.Content.ReadAsStreamAsync();
        using var validateDoc = await JsonDocument.ParseAsync(validateStream);

        return (
            validateDoc.RootElement.GetProperty("valid").GetBoolean(),
            validateDoc.RootElement.TryGetProperty("reason", out var reason) && reason.ValueKind != JsonValueKind.Null
                ? reason.GetString()
                : null
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
