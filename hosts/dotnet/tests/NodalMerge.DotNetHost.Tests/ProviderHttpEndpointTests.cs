using NodalMerge.DotNetHost;
using NodalMerge.Host.Abstractions.Providers;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using System.Net;
using System.Net.Http.Json;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Tests;

public sealed class ProviderHttpEndpointTests
{
    [Fact]
    public async Task Sync_token_returns_501_when_default_auth_provider_does_not_support_mint()
    {
        await using var app = BuildTestApp();
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
    public async Task Sync_token_returns_minted_payload_when_custom_provider_is_registered()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRoomTokenAuthProvider>(
                new FakeTokenProvider(
                    new RoomTokenMintResult(
                        IsSupported: true,
                        PeerPubkeyHex: "abcd",
                        ExpiryUnixSeconds: 1700000123,
                        Capabilities: ["read:**"],
                        SignatureHex: "beef"
                    )
                )
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsJsonAsync(
            "/api/sync/token",
            new
            {
                room = "room-a",
                peerPubkeyHex = "peer-a"
            }
        );

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);

        Assert.Equal("abcd", doc.RootElement.GetProperty("peer_pubkey_hex").GetString());
        Assert.Equal(1700000123L, doc.RootElement.GetProperty("expiry_secs").GetInt64());
        Assert.Equal("beef", doc.RootElement.GetProperty("sig_hex").GetString());
    }

    [Fact]
    public async Task Sync_token_validate_returns_provider_validation_result()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRoomTokenAuthProvider>(
                new FakeTokenProvider(
                    RoomTokenMintResult.NotSupported,
                    RoomTokenValidationResult.Invalid("expired")
                )
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
                expiry_secs = 1700000000,
                capabilities = new[] { "read:**" },
                sig_hex = "beef"
            }
        );

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);

        Assert.False(doc.RootElement.GetProperty("valid").GetBoolean());
        Assert.Equal("expired", doc.RootElement.GetProperty("reason").GetString());
    }

    [Fact]
    public async Task Sync_blob_url_put_uses_resolver_and_returns_url_payload()
    {
        var expiry = DateTimeOffset.UtcNow.AddMinutes(5);
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(
                new FakeBlobUrlResolverProvider(
                    putResult: new PresignedBlobUrl("https://upload.example/put", expiry)
                )
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync(
            "/sync/blob-url?op=put&room=room-a&namespace=assets&hash=sha256%3Aabc&size=1024&contentType=audio%2Faac"
        );

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);
        Assert.Equal("https://upload.example/put", doc.RootElement.GetProperty("url").GetString());
        Assert.Equal(expiry.ToUnixTimeSeconds(), doc.RootElement.GetProperty("expiresAt").GetInt64());
    }

    [Fact]
    public async Task Sync_blob_url_rejects_put_without_size()
    {
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync(
            "/api/sync/blob-url?op=put&hash=sha256%3Aabc"
        );

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    private static WebApplication BuildTestApp(Action<IServiceCollection>? configureServices = null)
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer()
        );
    }
}

internal sealed class FakeTokenProvider : IRoomTokenAuthProvider
{
    private readonly RoomTokenMintResult _mintResult;
    private readonly RoomTokenValidationResult _validationResult;

    public FakeTokenProvider(
        RoomTokenMintResult mintResult,
        RoomTokenValidationResult? validationResult = null
    )
    {
        _mintResult = mintResult;
        _validationResult = validationResult ?? RoomTokenValidationResult.Valid;
    }

    public ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(_validationResult);
    }

    public ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(_mintResult);
    }
}

internal sealed class FakeBlobUrlResolverProvider : IBlobUrlResolverProvider
{
    private readonly PresignedBlobUrl? _putResult;
    private readonly PresignedBlobUrl? _getResult;

    public FakeBlobUrlResolverProvider(PresignedBlobUrl? putResult = null, PresignedBlobUrl? getResult = null)
    {
        _putResult = putResult;
        _getResult = getResult;
    }

    public ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
        BlobPutUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(_putResult);
    }

    public ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
        BlobGetUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(_getResult);
    }
}
