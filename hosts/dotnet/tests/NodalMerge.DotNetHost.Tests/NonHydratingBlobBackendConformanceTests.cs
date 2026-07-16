using System.Net;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 0.2 (nodalmerge-studio/plans/blob-cas-remediation.md) — the .NET
/// half of the non-hydrating-backend conformance harness. The Rust mirror
/// is <c>server/server/tests/blob_nonhydrating_conformance.rs</c> (and
/// <c>blob_relay_nonhydrating_backend.rs</c>), backed by the shared
/// <c>NonHydratingBackend</c> fake that reproduces <c>S3BlobStore</c>'s real
/// shape: <c>get_blob</c> always <c>None</c>, <c>has_blob</c> a real
/// existence check.
///
/// .NET's <see cref="IBlobStoreProvider"/> has no existence-check method at
/// all (finding 2.1's whole point) — <c>HEAD</c> and PUT-idempotency both
/// answer "does this exist?" by calling <see
/// cref="IBlobStoreProvider.TryGetBlobAsync"/>, a full read. Under
/// <c>ChainedRemote</c>/<c>S3Direct</c> that's a full remote download + BLAKE3
/// verify + local write-back just to answer a question a cheap probe (HEAD /
/// <c>File.Exists</c>) could answer instead.
///
/// There is no existence-check API to call yet (that's exactly what slice
/// 2.1 adds), so these tests assert the observable fact instead: a counting
/// <see cref="IBlobStoreProvider"/> that already holds the bytes still sees
/// its full-read method invoked by <c>HEAD</c> / idempotent PUT. Per the
/// project's RED-test convention, both are gated (<c>Skip</c>) until slice
/// 2.1 removes the gate — see slice 0.2's final report for the real,
/// ungated failure output each one produces today.
/// </summary>
public sealed class NonHydratingBlobBackendConformanceTests
{
    /// <summary>
    /// Same shape as <c>ChainedBlobStoreProviderTests.CountingBlobStoreProvider</c>
    /// (that file's nested private fake) — promoted here so this file and that
    /// one don't drift on what "counting" means. Stores bytes for real (so a
    /// content-addressed idempotent PUT/GET actually succeeds) while counting
    /// how many times the *full read* method was invoked.
    /// </summary>
    private sealed class CountingBlobStoreProvider : IBlobStoreProvider
    {
        private readonly Dictionary<string, (byte[] Bytes, string? ContentType)> _blobs = new(StringComparer.Ordinal);

        public int GetCalls { get; private set; }

        public ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
        {
            GetCalls++;
            return ValueTask.FromResult(
                _blobs.TryGetValue(hashHex, out var entry)
                    ? BlobReadResult.Hit(entry.Bytes, entry.ContentType)
                    : BlobReadResult.Missing
            );
        }

        public ValueTask PutBlobAsync(
            string hashHex,
            byte[] bytes,
            string? contentType,
            CancellationToken cancellationToken = default
        )
        {
            _blobs[hashHex] = (bytes, contentType);
            return ValueTask.CompletedTask;
        }
    }

    private static async Task<(WebApplication App, HttpClient Client, CountingBlobStoreProvider Provider)> BuildAppAsync()
    {
        var provider = new CountingBlobStoreProvider();
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-nonhydrating-conformance", Guid.NewGuid().ToString("N"));

        var app = HostApplication.Build(
            [],
            // Registered *after* AddNodalMergeHostProviders, so this
            // singleton wins the IBlobStoreProvider resolution (last
            // registration wins) — swaps in the counting fake without
            // needing a dedicated "Counting" provider kind in
            // ServiceCollectionExtensions.
            configureServices: services => services.AddSingleton<IBlobStoreProvider>(provider),
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg =>
            {
                cfg.AddInMemoryCollection(new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = root
                });
            }
        );
        await app.StartAsync();
        return (app, app.GetTestClient(), provider);
    }

    [Fact(Skip = "RED: fails until slice 2.1 — see nodalmerge-studio/plans/blob-cas-remediation.md; HEAD must answer via a cheap existence probe (ExistsAsync), not a full TryGetBlobAsync read")]
    public async Task Head_does_not_hydrate_bytes_through_a_full_read()
    {
        var (app, client, provider) = await BuildAppAsync();
        try
        {
            var hash = Blake3.Hasher.Hash(System.Text.Encoding.UTF8.GetBytes("a blob a non-hydrating backend already holds")).ToString();
            await provider.PutBlobAsync(hash, System.Text.Encoding.UTF8.GetBytes("a blob a non-hydrating backend already holds"), null, CancellationToken.None);

            using var request = new HttpRequestMessage(HttpMethod.Head, $"/blobs/{hash}");
            using var response = await client.SendAsync(request);

            Assert.Equal(HttpStatusCode.OK, response.StatusCode);
            Assert.Equal(0, provider.GetCalls); // TODO(2.1): must go through ExistsAsync instead
        }
        finally
        {
            await app.StopAsync();
            await app.DisposeAsync();
        }
    }

    [Fact(Skip = "RED: fails until slice 2.1 — see nodalmerge-studio/plans/blob-cas-remediation.md; PUT-idempotency's already-present check must use a cheap existence probe (ExistsAsync), not a full TryGetBlobAsync read")]
    public async Task Put_of_already_present_blob_does_not_hydrate_bytes_through_a_full_read()
    {
        var (app, client, provider) = await BuildAppAsync();
        try
        {
            var bytes = System.Text.Encoding.UTF8.GetBytes("already stored, content-addressed bytes");
            var hash = Blake3.Hasher.Hash(bytes).ToString();
            await provider.PutBlobAsync(hash, bytes, null, CancellationToken.None);

            using var request = new HttpRequestMessage(HttpMethod.Put, $"/blobs/{hash}")
            {
                Content = new ByteArrayContent(bytes)
            };
            using var response = await client.SendAsync(request);

            Assert.Equal(HttpStatusCode.OK, response.StatusCode); // idempotent no-op, not 201
            Assert.Equal(0, provider.GetCalls); // TODO(2.1): must go through ExistsAsync instead
        }
        finally
        {
            await app.StopAsync();
            await app.DisposeAsync();
        }
    }
}
