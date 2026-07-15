using System.Net;
using System.Net.Http.Headers;
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
/// Content encoding (reserved v1.1, docs/BLOB_HTTP_SURFACE.md
/// "Content encoding"): an in-proc origin with zstd-at-rest compression on
/// negotiates <c>Accept-Encoding: zstd</c> / <c>Content-Encoding: zstd</c>
/// over the real blob GET route, and the <c>HttpRemoteBlobStoreProvider</c>
/// client decompresses before the chain's BLAKE3 verification — mirrors
/// the harness in <c>ChainedBlobStoreProviderHttpIntegrationTests</c>.
/// </summary>
public sealed class BlobHttpEncodingNegotiationTests : IAsyncLifetime
{
    // 100 KB of repetitive JSON — comfortably compressible and over the
    // default 4096-byte floor.
    private static readonly byte[] CompressiblePayload = System.Text.Encoding.UTF8.GetBytes(
        string.Concat(Enumerable.Repeat("{\"room\":\"room-a\",\"kind\":\"node\",\"value\":12345},", 2500))
    );

    private string? _originRoot;
    private WebApplication? _originApp;
    private HttpClient? _originClient;
    private string _hash = string.Empty;

    public async Task InitializeAsync()
    {
        _originRoot = NewTempRoot();
        _originApp = HostApplication.Build(
            [],
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg =>
            {
                cfg.AddInMemoryCollection(new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = _originRoot,
                    ["NodalMerge:Storage:FileBlobs:Compression"] = "Zstd"
                });
            }
        );
        await _originApp.StartAsync();
        _originClient = _originApp.GetTestClient();

        _hash = Hasher.Hash(CompressiblePayload).ToString();
        using var putResponse = await _originClient.PutAsync($"/blobs/{_hash}", new ByteArrayContent(CompressiblePayload));
        Assert.Equal(HttpStatusCode.Created, putResponse.StatusCode);

        // Confirm the seeded blob really landed zstd-encoded, otherwise
        // this test would pass for the wrong reason.
        Assert.True(File.Exists(Path.Combine(_originRoot, "blake3", _hash + ".zst")));
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
    public async Task Get_with_accept_encoding_zstd_returns_content_encoding_zstd_and_decompresses_correctly()
    {
        using var request = new HttpRequestMessage(HttpMethod.Get, $"/blobs/{_hash}");
        request.Headers.AcceptEncoding.Add(new StringWithQualityHeaderValue("zstd"));

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.Contains(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));

        var wireBytes = await response.Content.ReadAsByteArrayAsync();
        Assert.True(wireBytes.Length < CompressiblePayload.Length, "the wire bytes should be the compressed form, smaller than the original");

        var decompressed = DecompressZstdFrame(wireBytes);
        Assert.Equal(CompressiblePayload, decompressed);
    }

    [Fact]
    public async Task Get_without_accept_encoding_header_returns_identity_bytes()
    {
        using var response = await _originClient!.GetAsync($"/blobs/{_hash}");

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.DoesNotContain(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));

        var bytes = await response.Content.ReadAsByteArrayAsync();
        Assert.Equal(CompressiblePayload, bytes);
    }

    [Fact]
    public async Task Head_ignores_accept_encoding_and_behaves_as_before()
    {
        using var request = new HttpRequestMessage(HttpMethod.Head, $"/blobs/{_hash}");
        request.Headers.AcceptEncoding.Add(new StringWithQualityHeaderValue("zstd"));

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.DoesNotContain(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));
        Assert.Equal(CompressiblePayload.Length, response.Content.Headers.ContentLength);
    }

    [Fact]
    public async Task Chained_http_remote_provider_decompresses_and_verifies_end_to_end()
    {
        var localRoot = NewTempRoot();
        try
        {
            var configuration = new ConfigurationBuilder()
                .AddInMemoryCollection(new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:BlobStorage"] = "ChainedRemote",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = localRoot,
                    ["NodalMerge:Storage:RemoteOrigin:BaseUrl"] = "http://localhost"
                })
                .Build();

            var services = new ServiceCollection();
            services.AddSingleton<IHttpClientFactory>(new FakeOriginHttpClientFactory(_originClient!));
            services.AddNodalMergeHostProviders(configuration);

            await using var provider = services.BuildServiceProvider();
            var chain = provider.GetRequiredService<IBlobStoreProvider>();

            var result = await chain.TryGetBlobAsync(_hash);

            Assert.True(result.Found);
            Assert.Equal(CompressiblePayload, result.Bytes);
            Assert.True(
                File.Exists(Path.Combine(localRoot, "blake3", _hash)),
                "the chain writes through identity bytes to the local cache regardless of the origin's encoding"
            );
        }
        finally
        {
            TryDeleteDirectory(localRoot);
        }
    }

    private static byte[] DecompressZstdFrame(byte[] compressed)
    {
        var size = ZstdSharp.Decompressor.GetDecompressedSize(compressed);
        var buffer = new byte[(int)size];
        using var decompressor = new ZstdSharp.Decompressor();
        var written = decompressor.Unwrap((ReadOnlySpan<byte>)compressed, (Span<byte>)buffer);
        Assert.Equal(buffer.Length, written);
        return buffer;
    }

    private static string NewTempRoot()
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-blob-encoding-negotiation", Guid.NewGuid().ToString("N"));
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
