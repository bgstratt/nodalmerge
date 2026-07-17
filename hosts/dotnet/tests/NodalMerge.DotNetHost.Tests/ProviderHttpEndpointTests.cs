using NodalMerge.DotNetHost;
using NodalMerge.Host.Abstractions.Providers;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.DependencyInjection.Extensions;
using System.Net;
using System.Net.Http.Json;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Tests;

public sealed class ProviderHttpEndpointTests
{
    // Canonical 64-lowercase-hex test hash — matches the shared parity
    // vectors' `canonical_hash` (engine/commands/blob-layout-vectors.v1.json)
    // so it satisfies IsCanonicalBlobHash / the blob-surface hash-validation
    // rule shared by GET/HEAD/PUT /blobs/{hash} and the new URL-resolution
    // endpoints.
    private const string CanonicalHash =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

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
        // Legacy /sync/blob-url was restored to `main`'s pre-existing
        // behavior by slice 4.1 (blob-cas-remediation.md, finding #12): it is
        // NOT an alias of GET /blobs/{hash}/url — unix-seconds "expiresAt",
        // never the new route's ISO-8601 "expiresAtUtc". See
        // LegacySyncBlobUrlCompatTests.cs for the full golden-behavior suite;
        // this test only needed its expected field renamed/reshaped to match.
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
            $"/sync/blob-url?op=put&room=room-a&namespace=assets&hash={CanonicalHash}&size=1024&contentType=audio%2Faac"
        );

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);
        Assert.Equal("https://upload.example/put", doc.RootElement.GetProperty("url").GetString());
        Assert.False(doc.RootElement.TryGetProperty("expiresAtUtc", out _), "legacy route must not emit expiresAtUtc");
        Assert.Equal(expiry.ToUnixTimeSeconds(), doc.RootElement.GetProperty("expiresAt").GetInt64());
    }

    [Fact]
    public async Task Sync_blob_url_rejects_put_without_size()
    {
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync(
            $"/api/sync/blob-url?op=put&hash={CanonicalHash}"
        );

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    [Fact]
    public async Task Sync_blob_url_accepts_malformed_hash_unlike_the_new_route()
    {
        // Slice 4.1 (blob-cas-remediation.md, finding #12): the legacy route
        // was restored to `main`'s loose-hash, anonymous behavior — a
        // malformed hash is NOT rejected here (contrast BlobUrl_rejects_malformed_hash
        // below, the new frozen route, which does reject it). No backend is configured
        // (BuildTestApp()'s default WsOnly composition), so a well-formed
        // request falls through to "no presign-capable backend" -> 404, not
        // the 400 this test previously (incorrectly) expected.
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync("/api/sync/blob-url?op=get&hash=not-a-hash");

        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    // -- GET /blobs/{hash}/url (docs/BLOB_HTTP_SURFACE.md, slice S4.1) -------

    [Fact]
    public async Task BlobUrl_get_op_returns_frozen_shape()
    {
        var expiry = DateTimeOffset.UtcNow.AddHours(1);
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(
                new FakeBlobUrlResolverProvider(
                    getResult: new PresignedBlobUrl("https://bucket.example/get", expiry)
                )
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync($"/blobs/{CanonicalHash}/url?op=get");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);
        Assert.Equal("https://bucket.example/get", doc.RootElement.GetProperty("url").GetString());
        Assert.Equal(
            expiry.UtcDateTime.ToString("o"),
            doc.RootElement.GetProperty("expiresAtUtc").GetString()
        );
    }

    [Fact]
    public async Task BlobUrl_put_op_with_size_and_contentType_returns_frozen_shape()
    {
        var expiry = DateTimeOffset.UtcNow.AddMinutes(15);
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(
                new FakeBlobUrlResolverProvider(
                    putResult: new PresignedBlobUrl("https://bucket.example/put", expiry)
                )
            );
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync(
            $"/api/blobs/{CanonicalHash}/url?op=put&size=2048&contentType=text%2Fplain"
        );

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);
        Assert.Equal("https://bucket.example/put", doc.RootElement.GetProperty("url").GetString());
        Assert.Equal(
            expiry.UtcDateTime.ToString("o"),
            doc.RootElement.GetProperty("expiresAtUtc").GetString()
        );
    }

    [Fact]
    public async Task BlobUrl_returns_501_when_backend_has_no_presign_capability()
    {
        // Default composition (BlobStorage=WsOnly) registers a resolver
        // whose Resolve*UrlAsync always return null — the "no delegated
        // backend" case the frozen contract requires answering 501 for.
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync($"/blobs/{CanonicalHash}/url?op=get");

        Assert.Equal(HttpStatusCode.NotImplemented, response.StatusCode);
    }

    [Fact]
    public async Task BlobUrl_returns_501_when_no_resolver_registered_at_all()
    {
        await using var app = BuildTestApp(services =>
        {
            services.RemoveAll<IBlobUrlResolverProvider>();
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync($"/blobs/{CanonicalHash}/url?op=get");

        Assert.Equal(HttpStatusCode.NotImplemented, response.StatusCode);
    }

    [Fact]
    public async Task BlobUrl_rejects_malformed_hash()
    {
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync("/blobs/not-a-hash/url?op=get");

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
        using var stream = await response.Content.ReadAsStreamAsync();
        using var doc = await JsonDocument.ParseAsync(stream);
        Assert.Equal("non-canonical hash", doc.RootElement.GetProperty("error").GetString());
    }

    [Fact]
    public async Task BlobUrl_rejects_invalid_op()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(new FakeBlobUrlResolverProvider());
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.GetAsync($"/blobs/{CanonicalHash}/url?op=delete");

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
    }

    // -- POST /blobs/{hash}/uploaded (docs/BLOB_HTTP_SURFACE.md, slice S4.1) -

    [Fact]
    public async Task BlobUploaded_accepts_when_resolver_registered()
    {
        // No bucket visibility in the .NET host today — MAY-accept without
        // verification once a resolver exists (WsOnly default composition
        // included).
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsync($"/blobs/{CanonicalHash}/uploaded", content: null);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
    }

    [Fact]
    public async Task BlobUploaded_returns_501_when_no_resolver_registered()
    {
        await using var app = BuildTestApp(services =>
        {
            services.RemoveAll<IBlobUrlResolverProvider>();
        });
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsync($"/api/blobs/{CanonicalHash}/uploaded", content: null);

        Assert.Equal(HttpStatusCode.NotImplemented, response.StatusCode);
    }

    [Fact]
    public async Task BlobUploaded_rejects_malformed_hash()
    {
        await using var app = BuildTestApp();
        await app.StartAsync();

        var client = app.GetTestClient();
        var response = await client.PostAsync("/blobs/not-a-hash/uploaded", content: null);

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
