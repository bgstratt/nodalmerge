using System.Net;
using System.Text;
using Blake3;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Acceptance coverage for slice 2.2 (nodalmerge-studio/plans/cas-distribution-and-storage.md):
/// "a cold local store materializes entirely from the origin; a corrupted
/// remote payload is rejected and never written to the local cache."
///
/// Drives a real in-proc origin host (same harness as <c>BlobHttpSurfaceTests</c>,
/// speaking the frozen docs/BLOB_HTTP_SURFACE.md surface over TestServer) and a
/// full DI-composed <c>ChainedRemote</c> provider graph whose HttpClient is
/// swapped for the origin's TestServer client — no real network involved.
/// </summary>
public sealed class ChainedBlobStoreProviderHttpIntegrationTests : IAsyncLifetime
{
    private string? _originRoot;
    private WebApplication? _originApp;
    private HttpClient? _originClient;

    public async Task InitializeAsync()
    {
        _originRoot = NewTempRoot("origin");
        _originApp = HostApplication.Build(
            [],
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg =>
            {
                cfg.AddInMemoryCollection(new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = _originRoot
                });
            }
        );
        await _originApp.StartAsync();
        _originClient = _originApp.GetTestClient();
    }

    public async Task DisposeAsync()
    {
        _originClient?.Dispose();

        if (_originApp is not null)
        {
            await _originApp.StopAsync();
            await _originApp.DisposeAsync();
        }

        TryDeleteDirectory(_originRoot);
    }

    [Fact]
    public async Task Cold_local_store_materializes_entirely_from_origin()
    {
        var bytes = Encoding.UTF8.GetBytes("slice 2.2 acceptance payload");
        var hash = Hasher.Hash(bytes).ToString();

        using var putResponse = await _originClient!.PutAsync($"/blobs/{hash}", new ByteArrayContent(bytes));
        Assert.Equal(HttpStatusCode.Created, putResponse.StatusCode);

        var localRoot = NewTempRoot("local-cold");
        try
        {
            var chain = BuildChainedProvider(localRoot);

            var result = await chain.TryGetBlobAsync(hash);

            Assert.True(result.Found);
            Assert.Equal(bytes, result.Bytes);
            Assert.True(
                File.Exists(Path.Combine(localRoot, "blake3", hash)),
                "expected the local cache to now hold the blob after a cold materialize"
            );
        }
        finally
        {
            TryDeleteDirectory(localRoot);
        }
    }

    [Fact]
    public async Task Corrupted_remote_payload_is_rejected_and_never_cached_locally()
    {
        var correctBytes = Encoding.UTF8.GetBytes("this is the real content");
        var hash = Hasher.Hash(correctBytes).ToString();
        var wrongBytes = Encoding.UTF8.GetBytes("this is NOT the real content");

        // Seed the origin's on-disk store directly, bypassing the HTTP
        // API's own hash check on PUT. This simulates a blob that was
        // corrupted at rest (bit rot, disk fault) — the .NET File store
        // does not verify on read (unlike the Rust mirror per
        // docs/BLOB_HTTP_SURFACE.md), so the origin will happily serve the
        // wrong bytes back as a 200. Verification is the chain's job.
        var blake3Dir = Path.Combine(_originRoot!, "blake3");
        Directory.CreateDirectory(blake3Dir);
        await File.WriteAllBytesAsync(Path.Combine(blake3Dir, hash), wrongBytes);

        var localRoot = NewTempRoot("local-corrupt");
        try
        {
            var chain = BuildChainedProvider(localRoot);

            var result = await chain.TryGetBlobAsync(hash);

            Assert.False(result.Found);
            Assert.False(
                File.Exists(Path.Combine(localRoot, "blake3", hash)),
                "a corrupted remote payload must never be written to the local cache"
            );
        }
        finally
        {
            TryDeleteDirectory(localRoot);
        }
    }

    private IBlobStoreProvider BuildChainedProvider(string localRoot)
    {
        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = localRoot,
                // Unused by the fake factory below (which always returns
                // the origin TestServer client), but must be a valid
                // absolute URI to pass RemoteBlobOriginOptions.Validate().
                ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = "http://localhost"
            })
            .Build();

        var services = new ServiceCollection();
        services.AddSingleton<IHttpClientFactory>(new FakeOriginHttpClientFactory(_originClient!));
        services.AddNodalMergeHostProviders(configuration);

        var provider = services.BuildServiceProvider();
        return provider.GetRequiredService<IBlobStoreProvider>();
    }

    private static string NewTempRoot(string label)
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-chained-blob-integration", label, Guid.NewGuid().ToString("N"));
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

    private sealed class FakeOriginHttpClientFactory(HttpClient client) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => client;
    }
}
