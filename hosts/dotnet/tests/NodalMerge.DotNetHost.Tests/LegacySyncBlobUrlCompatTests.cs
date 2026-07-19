using System.Net;
using System.Text.Json;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Golden tests for the legacy <c>/sync/blob-url</c> (+ <c>/api/sync/blob-url</c>)
/// route, restored to <c>main</c>'s pre-existing behavior by slice 4.1
/// (blob-cas-remediation.md, finding #12). Ground truth is <c>main</c>'s own
/// <c>HandleBlobUrlAsync</c> (recovered via <c>git show main:hosts/dotnet/src/
/// NodalMerge.DotNetHost/WebApplicationExtensions.cs</c>, pre-blobExpansion):
/// unix-seconds <c>expiresAt</c> (never ISO-8601 <c>expiresAtUtc</c>), 404 for
/// "no backend" (never 501), and NO hash-shape or auth validation at all (any
/// non-empty hash, anonymous, always — this route never even reads
/// <see cref="BlobHttpOptions"/>). Pre-fix (the S4.1 refactor that turned this
/// alias into a thin wrapper around the new frozen <c>/blobs/{hash}/url</c>
/// handler) every assertion below failed; the slice's report captures the
/// exact pre-fix values (expiresAtUtc present instead of expiresAt, 501
/// instead of 404, 400 "non-canonical hash" instead of 200/"hash is
/// required").
///
/// Reuses <c>FakeBlobUrlResolverProvider</c> (declared in
/// ProviderHttpEndpointTests.cs, <c>internal</c> in this same test assembly)
/// and the <c>HostApplication.Build</c> TestServer harness
/// (BlobUrlResolutionVectorTests.cs's style) rather than duplicating either.
/// </summary>
public sealed class LegacySyncBlobUrlCompatTests
{
    private static readonly string CanonicalHash = new('a', 64);

    [Theory]
    [InlineData("/sync/blob-url")]
    [InlineData("/api/sync/blob-url")]
    public async Task Get_happy_path_returns_unix_seconds_expiresAt_not_iso8601(string route)
    {
        var expiry = DateTimeOffset.UtcNow.AddSeconds(900);
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(
                new FakeBlobUrlResolverProvider(getResult: new PresignedBlobUrl("https://bucket.test/obj", expiry))
            );
        });
        await app.StartAsync();
        var client = app.GetTestClient();

        // Canonical hash here so this test isolates the response-shape bug
        // (expiresAt vs expiresAtUtc) from the separate "loose hash accepted"
        // property, which Non_canonical_but_non_empty_hash_is_accepted covers
        // on its own — otherwise a pre-fix 400 from hash validation would
        // mask the shape assertions below instead of exercising them.
        var response = await client.GetAsync($"{route}?hash={CanonicalHash}&op=get");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        Assert.Equal("https://bucket.test/obj", doc.RootElement.GetProperty("url").GetString());
        Assert.False(
            doc.RootElement.TryGetProperty("expiresAtUtc", out _),
            "legacy route must NOT emit expiresAtUtc (that is the new route's shape)"
        );
        Assert.True(doc.RootElement.TryGetProperty("expiresAt", out var expiresAtEl), "legacy route must emit expiresAt");
        Assert.Equal(JsonValueKind.Number, expiresAtEl.ValueKind);
        Assert.Equal(expiry.ToUnixTimeSeconds(), expiresAtEl.GetInt64());
    }

    [Fact]
    public async Task Put_happy_path_returns_unix_seconds_expiresAt()
    {
        var expiry = DateTimeOffset.UtcNow.AddSeconds(300);
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(
                new FakeBlobUrlResolverProvider(putResult: new PresignedBlobUrl("https://bucket.test/put-obj", expiry))
            );
        });
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync($"/sync/blob-url?hash={CanonicalHash}&op=put&size=1024");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        Assert.Equal("https://bucket.test/put-obj", doc.RootElement.GetProperty("url").GetString());
        Assert.Equal(expiry.ToUnixTimeSeconds(), doc.RootElement.GetProperty("expiresAt").GetInt64());
    }

    [Theory]
    [InlineData("")]
    [InlineData("&size=0")]
    [InlineData("&size=-5")]
    public async Task Put_missing_or_non_positive_size_returns_400(string sizeQuery)
    {
        await using var app = BuildTestApp(_ => { });
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync($"/sync/blob-url?hash={CanonicalHash}&op=put{sizeQuery}");

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        Assert.Equal("size must be a positive integer for op=put", doc.RootElement.GetProperty("error").GetString());
    }

    [Fact]
    public async Task No_backend_configured_returns_404_not_501()
    {
        // Default provider composition (NodalMergeHostProviderOptions.Defaults,
        // BlobStorage=WsOnly) registers a real IBlobUrlResolverProvider whose
        // Resolve*Url always returns null — exactly main's WsOnlyBlobStoreProvider
        // shape. No override needed to model "no backend".
        await using var app = BuildTestApp(_ => { });
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync($"/sync/blob-url?hash={CanonicalHash}&op=get");

        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
    }

    [Fact]
    public async Task Non_canonical_but_non_empty_hash_is_accepted()
    {
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(
                new FakeBlobUrlResolverProvider(
                    getResult: new PresignedBlobUrl("https://bucket.test/obj", DateTimeOffset.UtcNow.AddMinutes(5))
                )
            );
        });
        await app.StartAsync();
        var client = app.GetTestClient();

        // Not 64 lowercase hex chars — the new route's BlobHash.IsCanonical
        // check would 400 this; the legacy route must accept any non-empty
        // hash, exactly as loose as `main` was.
        var response = await client.GetAsync("/sync/blob-url?hash=not-a-real-hash&op=get");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
    }

    [Fact]
    public async Task Empty_hash_returns_400_hash_is_required()
    {
        await using var app = BuildTestApp(_ => { });
        await app.StartAsync();
        var client = app.GetTestClient();

        // hash=<empty value>, NOT an omitted key: BlobUrlQuery.Hash is a
        // non-nullable string, so ASP.NET's own [AsParameters] binding 400s
        // an omitted key before the request ever reaches our handler (an
        // empty, non-JSON body — verified directly; same on `main` and both
        // pre/post-fix, since it's framework binding, not this slice's
        // logic). An explicitly empty value, exactly like an explicit
        // whitespace-only value, DOES reach the handler and is what `main`'s
        // own `string.IsNullOrWhiteSpace(query.Hash)` guards.
        var response = await client.GetAsync("/sync/blob-url?hash=&op=get");

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        // Distinct wording from the new route's "non-canonical hash" — proves
        // this is NOT routed through ValidateBlobRequest.
        Assert.Equal("hash is required", doc.RootElement.GetProperty("error").GetString());
    }

    [Fact]
    public async Task Op_missing_or_invalid_returns_400()
    {
        await using var app = BuildTestApp(_ => { });
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync($"/sync/blob-url?hash={CanonicalHash}&op=frobnicate");

        Assert.Equal(HttpStatusCode.BadRequest, response.StatusCode);
        using var doc = JsonDocument.Parse(await response.Content.ReadAsStringAsync());
        Assert.Equal("op must be 'get' or 'put'", doc.RootElement.GetProperty("error").GetString());
    }

    /// <summary>
    /// <c>main</c>'s legacy handler took <c>IBlobUrlResolverProvider</c>
    /// directly with no <see cref="BlobHttpOptions"/>/auth involvement
    /// whatsoever, so this route must stay anonymous even when the origin's
    /// blob-http auth token IS configured. Proven against the new route under
    /// the IDENTICAL configuration, so this isn't "auth is globally off in
    /// this test host" — the new route genuinely 401s here and the legacy one
    /// genuinely doesn't.
    /// </summary>
    [Fact]
    public async Task Anonymous_request_bypasses_auth_even_when_blob_http_auth_token_configured()
    {
        await using var app = BuildTestApp(
            services => services.AddSingleton<IBlobUrlResolverProvider>(
                new FakeBlobUrlResolverProvider(
                    getResult: new PresignedBlobUrl("https://bucket.test/obj", DateTimeOffset.UtcNow.AddMinutes(5))
                )
            ),
            configureConfiguration: cfg => cfg.AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:BlobHttp:AuthToken"] = "super-secret"
            })
        );
        await app.StartAsync();
        var client = app.GetTestClient();

        var legacyResponse = await client.GetAsync($"/sync/blob-url?hash={CanonicalHash}&op=get");
        Assert.Equal(HttpStatusCode.OK, legacyResponse.StatusCode);

        // Control: the SAME configuration 401s the new frozen route with no
        // Bearer header.
        var newRouteResponse = await client.GetAsync($"/blobs/{CanonicalHash}/url?op=get");
        Assert.Equal(HttpStatusCode.Unauthorized, newRouteResponse.StatusCode);
    }

    /// <summary>
    /// The one part of this surface that legitimately differs from the new
    /// route and predates it: <c>room</c>/<c>namespace</c> query parameters,
    /// defaulting to <c>"default"</c>/<c>"assets"</c>. Not part of the
    /// regression, but worth pinning so nothing "fixes" it away while
    /// restoring the rest.
    /// </summary>
    [Fact]
    public async Task Room_and_namespace_default_when_absent_and_pass_through_when_present()
    {
        BlobGetUrlRequest? captured = null;
        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IBlobUrlResolverProvider>(new RecordingResolverProvider(req => captured = req));
        });
        await app.StartAsync();
        var client = app.GetTestClient();

        await client.GetAsync("/sync/blob-url?hash=whatever&op=get");
        Assert.NotNull(captured);
        Assert.Equal("default", captured!.RoomId);
        Assert.Equal("assets", captured.Namespace);

        captured = null;
        await client.GetAsync("/sync/blob-url?hash=whatever&op=get&room=my-room&namespace=my-ns");
        Assert.NotNull(captured);
        Assert.Equal("my-room", captured!.RoomId);
        Assert.Equal("my-ns", captured.Namespace);
    }

    private static WebApplication BuildTestApp(
        Action<IServiceCollection> configureServices,
        Action<ConfigurationManager>? configureConfiguration = null
    )
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: configureConfiguration
        );
    }

    private sealed class RecordingResolverProvider(Action<BlobGetUrlRequest> onGet) : IBlobUrlResolverProvider
    {
        public ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
            BlobPutUrlRequest request,
            CancellationToken cancellationToken = default
        ) => ValueTask.FromResult<PresignedBlobUrl?>(null);

        public ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
            BlobGetUrlRequest request,
            CancellationToken cancellationToken = default
        )
        {
            onGet(request);
            return ValueTask.FromResult<PresignedBlobUrl?>(
                new PresignedBlobUrl("https://bucket.test/obj", DateTimeOffset.UtcNow.AddMinutes(5))
            );
        }
    }
}
