using System.Net;
using System.Net.Http.Headers;
using System.Text;
using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 4.3 MinIO-gated end-to-end test — env-gated (skipped, not failed,
/// when the env var isn't set), mirroring the gating *posture* of the Rust
/// suite's Docker-gated MinIO tests (<c>server/s3-blobs/tests/minio_round_trip.rs</c>,
/// <c>server/server/tests/blob_url_resolution_minio.rs</c>: both return
/// early with a message when the container/Docker isn't available, rather
/// than failing the run). This harness is gated on an already-running
/// origin process instead of orchestrating Docker/a Rust binary from xunit,
/// since that orchestration lives naturally in the Rust test suite (which
/// already has it) — duplicating a Testcontainers + child-process harness
/// here would not exercise anything the Rust tests don't already prove
/// about the origin itself, only the .NET client wired to it, which is
/// exactly what this test drives against the real stack.
///
/// ## Running this test locally
///
/// 1. Start MinIO:
///    <code>
///    docker run --rm -p 9000:9000 -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin minio/minio server /data
///    </code>
/// 2. Create the bucket (once): `nodalmerge-s3direct-dotnet-test`, e.g. via the MinIO console at
///    http://127.0.0.1:9000 or `mc mb local/nodalmerge-s3direct-dotnet-test`.
/// 3. Start the real Rust origin against that MinIO, from the repo root:
///    <code>
///    NODALMERGE_S3_BUCKET=nodalmerge-s3direct-dotnet-test \
///    NODALMERGE_S3_ENDPOINT=http://127.0.0.1:9000 \
///    NODALMERGE_S3_REQUIRE_HTTPS=false \
///    NODALMERGE_S3_ACCESS_KEY_ID=minioadmin \
///    NODALMERGE_S3_SECRET_ACCESS_KEY=minioadmin \
///    NODALMERGE_S3_DIRECT_UPLOAD_THRESHOLD_BYTES=0 \
///    NODALMERGE_BIND_ADDR=127.0.0.1:7979 \
///    cargo run -p nodalmerge-server-s3 -- --blob-backend s3
///    </code>
///    (`DIRECT_UPLOAD_THRESHOLD_BYTES=0` so a small test payload still gets
///    presigned instead of declining under the reference threshold.)
/// 4. Run this test with the origin's base URL in the environment:
///    <code>
///    NODALMERGE_TEST_S3DIRECT_ORIGIN_BASEURL=http://127.0.0.1:7979 dotnet test --filter S3DirectMinioEndToEndTests
///    </code>
///
/// Unset (or any other absence of the env var) => the test returns early
/// with a console note and reports as passed, exactly like the Rust
/// Docker-gated tests do when the container can't start.
/// </summary>
public sealed class S3DirectMinioEndToEndTests
{
    private const string OriginBaseUrlEnvVar = "NODALMERGE_TEST_S3DIRECT_ORIGIN_BASEURL";

    [Fact]
    public async Task Cold_peer_round_trip_put_then_get_flows_bytes_only_peer_to_minio()
    {
        var originBaseUrl = Environment.GetEnvironmentVariable(OriginBaseUrlEnvVar);
        if (string.IsNullOrWhiteSpace(originBaseUrl))
        {
            Console.WriteLine(
                $"skipping: {OriginBaseUrlEnvVar} not set (no running nodalmerge-server-s3 + MinIO stack) — see this file's doc comment for the run recipe"
            );
            return;
        }

        using var httpClient = new HttpClient { BaseAddress = new Uri(originBaseUrl, UriKind.Absolute) };

        var payload = Encoding.UTF8.GetBytes(
            "slice 4.3 MinIO end-to-end acceptance payload — " + Guid.NewGuid().ToString("N")
        );
        var hash = Hasher.Hash(payload).ToString();

        // 0. Sanity: the reference S3-backed origin's relay GET never
        // hydrates bytes (S3BlobStore::get_blob always None, pinned by
        // blob_url_resolution_minio.rs) — confirm that up front so the rest
        // of this test's "bytes flowed peer<->MinIO, not through the
        // origin" claim is meaningful rather than assumed.
        using (var relayProbe = await httpClient.GetAsync($"/blobs/{hash}"))
        {
            Assert.Equal(
                HttpStatusCode.NotFound,
                relayProbe.StatusCode
            );
        }

        var localRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-s3direct-minio-e2e", Guid.NewGuid().ToString("N"));
        try
        {
            var chain = BuildChain(localRoot, originBaseUrl);

            // 1. Put via the chain: local write, then relay (miss/degrade,
            // logged) + s3-direct fan-out. s3-direct's PutBlobAsync
            // compresses (payload clears the default 4096-byte minimum? —
            // no, this payload is small, so ShouldCompress will say no;
            // that's fine, this test is about the URL-resolution + bucket
            // round-trip, not compression specifically — S3DirectBlobStoreProviderTests
            // already covers compress vs. skip-heuristic in isolation).
            await chain.PutBlobAsync(hash, payload, "text/plain");

            // 2. Confirm the relay never got asked to serve these bytes by
            // hitting it directly — it must still 404 (nothing changed
            // about the S3 backend's non-hydrating get_blob).
            using (var relayAfterPut = await httpClient.GetAsync($"/blobs/{hash}"))
            {
                Assert.Equal(HttpStatusCode.NotFound, relayAfterPut.StatusCode);
            }

            // 3. Cold peer: a brand-new local cache, fetching through the
            // same chain. The only way this can succeed is via s3-direct's
            // presigned GET hitting MinIO directly.
            var coldRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-s3direct-minio-e2e-cold", Guid.NewGuid().ToString("N"));
            try
            {
                var coldChain = BuildChain(coldRoot, originBaseUrl);
                var result = await coldChain.TryGetBlobAsync(hash);

                Assert.True(result.Found, "expected the cold peer to materialize the blob via s3-direct");
                Assert.Equal(payload, result.Bytes);
                Assert.True(
                    File.Exists(Path.Combine(coldRoot, "blake3", hash)),
                    "expected the cold peer's local cache to hold the blob after materializing"
                );
            }
            finally
            {
                TryDeleteDirectory(coldRoot);
            }
        }
        finally
        {
            TryDeleteDirectory(localRoot);
        }
    }

    private static IBlobStoreProvider BuildChain(string localRoot, string originBaseUrl)
    {
        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = localRoot,
                ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = originBaseUrl,
                ["NodalMerge:Storage:S3Direct:Enabled"] = "true"
            })
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(configuration);
        var provider = services.BuildServiceProvider();
        return provider.GetRequiredService<IBlobStoreProvider>();
    }

    private static void TryDeleteDirectory(string? path)
    {
        if (path is null)
        {
            return;
        }
        try
        {
            if (Directory.Exists(path))
            {
                Directory.Delete(path, recursive: true);
            }
        }
        catch
        {
            // Best-effort cleanup; leftover temp dirs are harmless.
        }
    }
}
