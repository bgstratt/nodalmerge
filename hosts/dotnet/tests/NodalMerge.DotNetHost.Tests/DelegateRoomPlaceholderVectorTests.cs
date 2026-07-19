using System.Text.Json;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 7.4 (nodalmerge-studio/plans/blob-cas-remediation.md) — freezes the
/// delegate room-id placeholder. The delegate presign protocol v1
/// (docs/BLOB_STORAGE_LAYOUT.md §7) makes <c>room</c>/<c>namespace</c>
/// metadata-only (never key inputs), but a delegate that logs/quotas/audits
/// per room previously saw DIFFERENT per-host values on the room-agnostic
/// routes: Rust sent <c>room="_global"</c>, this host sent
/// <c>room="default", namespace="blobs"</c>. The frozen winner
/// (<c>_global</c>/<c>blobs</c>) lives in the
/// <c>delegate_room_id_placeholder</c> slot of
/// <c>engine/commands/work-unit-status-vectors.v1.json</c> — the slot slice
/// 0.4 reserved for exactly this decision. The Rust mirror is
/// <c>server/server/tests/blob_url_resolution_vectors.rs</c>
/// (<c>delegate_room_placeholder_matches_frozen_vector</c>).
///
/// These tests drive the REAL <c>GET /blobs/{hash}/url</c> routes with a
/// capturing resolver, so they pin what actually goes into the resolver's
/// <see cref="BlobGetUrlRequest"/>/<see cref="BlobPutUrlRequest"/> — not
/// just the constant's value. The legacy <c>/sync/blob-url</c> route is
/// deliberately NOT covered: its caller-supplied room/namespace (defaults
/// <c>"default"</c>/<c>"assets"</c>) are frozen legacy behavior
/// (LegacySyncBlobUrlCompatTests) and unchanged by 7.4.
/// </summary>
public sealed class DelegateRoomPlaceholderVectorTests
{
    private const string CanonicalHash =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    private static readonly string VectorRoom;
    private static readonly string VectorNamespace;

    static DelegateRoomPlaceholderVectorTests()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "work-unit-status-vectors.v1.json");
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var slot = doc.RootElement.GetProperty("delegate_room_id_placeholder");
        VectorRoom = slot.GetProperty("room").GetString()
            ?? throw new InvalidOperationException("delegate_room_id_placeholder.room is null — slice 7.4 must have decided it");
        VectorNamespace = slot.GetProperty("namespace").GetString()
            ?? throw new InvalidOperationException("delegate_room_id_placeholder.namespace is null — slice 7.4 must have decided it");
    }

    [Fact]
    public void Constants_match_the_frozen_vector_slot()
    {
        Assert.Equal(DelegatePresignProtocol.GlobalRoomPlaceholder, VectorRoom);
        Assert.Equal(DelegatePresignProtocol.GlobalRoomNamespace, VectorNamespace);
    }

    [Fact]
    public async Task Url_resolve_get_route_sends_the_frozen_room_and_namespace()
    {
        var resolver = new CapturingResolver();
        await using var app = BuildTestApp(resolver);
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync($"/blobs/{CanonicalHash}/url?op=get");
        Assert.Equal(System.Net.HttpStatusCode.OK, response.StatusCode);

        var request = Assert.IsType<BlobGetUrlRequest>(resolver.LastRequest);
        Assert.Equal(VectorRoom, request.RoomId);
        Assert.Equal(VectorNamespace, request.Namespace);
    }

    [Fact]
    public async Task Url_resolve_put_route_sends_the_frozen_room_and_namespace()
    {
        var resolver = new CapturingResolver();
        await using var app = BuildTestApp(resolver);
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync($"/blobs/{CanonicalHash}/url?op=put&size=1024");
        Assert.Equal(System.Net.HttpStatusCode.OK, response.StatusCode);

        var request = Assert.IsType<BlobPutUrlRequest>(resolver.LastRequest);
        Assert.Equal(VectorRoom, request.RoomId);
        Assert.Equal(VectorNamespace, request.Namespace);
    }

    [Fact]
    public async Task Api_mirror_route_sends_the_same_frozen_placeholder()
    {
        var resolver = new CapturingResolver();
        await using var app = BuildTestApp(resolver);
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync($"/api/blobs/{CanonicalHash}/url?op=get");
        Assert.Equal(System.Net.HttpStatusCode.OK, response.StatusCode);

        var request = Assert.IsType<BlobGetUrlRequest>(resolver.LastRequest);
        Assert.Equal(VectorRoom, request.RoomId);
        Assert.Equal(VectorNamespace, request.Namespace);
    }

    private static WebApplication BuildTestApp(IBlobUrlResolverProvider resolver)
    {
        return HostApplication.Build(
            [],
            configureServices: services => services.AddSingleton(resolver),
            configureWebHost: webHost => webHost.UseTestServer()
        );
    }

    private sealed class CapturingResolver : IBlobUrlResolverProvider
    {
        public object? LastRequest { get; private set; }

        public ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
            BlobPutUrlRequest request,
            CancellationToken cancellationToken = default
        )
        {
            LastRequest = request;
            return ValueTask.FromResult<PresignedBlobUrl?>(
                new PresignedBlobUrl("https://bucket.test/presigned", DateTimeOffset.UtcNow.AddSeconds(900))
            );
        }

        public ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
            BlobGetUrlRequest request,
            CancellationToken cancellationToken = default
        )
        {
            LastRequest = request;
            return ValueTask.FromResult<PresignedBlobUrl?>(
                new PresignedBlobUrl("https://bucket.test/presigned", DateTimeOffset.UtcNow.AddSeconds(900))
            );
        }
    }
}
