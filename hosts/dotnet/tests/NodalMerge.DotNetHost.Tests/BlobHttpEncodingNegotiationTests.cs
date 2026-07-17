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

    // ─── slice 3.4 (blob-cas-remediation.md): `q=0` refusal + PUT
    // Content-Encoding -> 415 parity. Hand-written, not vector-driven — the
    // frozen vector schema (blob-http-surface-vectors.v1.json, read by
    // BlobHttpSurfaceTests.cs) has no request-header slot at all, so these
    // pair with the equivalent hand-written tests in
    // server/server/tests/blob_http_surface.rs instead of forcing the schema.

    /// RFC 9110 `q=0` means "do not send this coding" — bare `zstd;q=0`.
    [Fact]
    public async Task Get_with_accept_encoding_zstd_q0_returns_identity_bytes()
    {
        using var request = new HttpRequestMessage(HttpMethod.Get, $"/blobs/{_hash}");
        request.Headers.TryAddWithoutValidation("Accept-Encoding", "zstd;q=0");

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.DoesNotContain(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));
        var bytes = await response.Content.ReadAsByteArrayAsync();
        Assert.Equal(CompressiblePayload, bytes);
    }

    /// Every qvalue-zero spelling RFC 9110 allows, plus whitespace, must
    /// refuse zstd identically (mirrors the Rust unit tests in blob_http.rs).
    [Theory]
    [InlineData("zstd;q=0")]
    [InlineData("zstd;q=0.0")]
    [InlineData("zstd;q=0.00")]
    [InlineData("zstd;q=0.000")]
    [InlineData("zstd ; q=0")]
    [InlineData("ZSTD;Q=0")]
    public async Task Get_with_accept_encoding_zstd_q0_syntax_variants_all_refuse(string acceptEncoding)
    {
        using var request = new HttpRequestMessage(HttpMethod.Get, $"/blobs/{_hash}");
        request.Headers.TryAddWithoutValidation("Accept-Encoding", acceptEncoding);

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.DoesNotContain(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));
    }

    /// `zstd;q=0, gzip` — the comma-separated list must still be walked and
    /// zstd's own `q=0` must be honored regardless of position.
    [Fact]
    public async Task Get_with_accept_encoding_zstd_q0_then_gzip_returns_identity_bytes()
    {
        using var request = new HttpRequestMessage(HttpMethod.Get, $"/blobs/{_hash}");
        request.Headers.TryAddWithoutValidation("Accept-Encoding", "zstd;q=0, gzip");

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.DoesNotContain(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));
    }

    /// `gzip;q=0, zstd` — the opposite order: `gzip`'s `q=0` must NOT leak
    /// onto `zstd`'s (unparameterized, so accepted) token. Parameters are
    /// per-coding. This scenario was already correct before the fix (today's
    /// bug is "any q is ignored", not "q is shared across the list") — kept
    /// as a pin against a fix that scopes q globally instead of per-coding.
    [Fact]
    public async Task Get_with_gzip_q0_then_zstd_still_returns_content_encoding_zstd()
    {
        using var request = new HttpRequestMessage(HttpMethod.Get, $"/blobs/{_hash}");
        request.Headers.TryAddWithoutValidation("Accept-Encoding", "gzip;q=0, zstd");

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.Contains(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));
    }

    /// A nonzero `q` is NOT a refusal — this slice only honors `q=0`, it does
    /// not build qvalue-preference ordering. Also a pre-existing-correct pin.
    [Fact]
    public async Task Get_with_accept_encoding_zstd_nonzero_q_still_returns_content_encoding_zstd()
    {
        using var request = new HttpRequestMessage(HttpMethod.Get, $"/blobs/{_hash}");
        request.Headers.TryAddWithoutValidation("Accept-Encoding", "zstd;q=0.5");

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.Contains(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));
    }

    /// <summary>
    /// Parity with Rust (<c>blob_http.rs</c>'s <c>put_blob</c>, which rejects
    /// any PUT carrying a <c>Content-Encoding</c> header with 415 before ever
    /// hashing the body): this pins the failure mode the plan calls "worse
    /// than a 422" — the request's literal wire bytes DO hash to the path
    /// hash (a client could construct this by accident, or by mislabeling an
    /// identity PUT), so pre-fix .NET happily stored them under that hash
    /// with no indication they were ever labeled compressed. Post-fix: 415,
    /// nothing persisted.
    /// </summary>
    [Fact]
    public async Task Put_with_content_encoding_header_and_hash_matching_literal_bytes_is_rejected_415_not_stored()
    {
        var literalBytes = System.Text.Encoding.UTF8.GetBytes("arbitrary bytes labeled zstd but never actually compressed, slice 3.4");
        var hash = Hasher.Hash(literalBytes).ToString();

        using var request = new HttpRequestMessage(HttpMethod.Put, $"/blobs/{hash}") { Content = new ByteArrayContent(literalBytes) };
        request.Content.Headers.ContentEncoding.Add("zstd");

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.UnsupportedMediaType, response.StatusCode);
        Assert.False(
            File.Exists(Path.Combine(_originRoot!, "blake3", hash)),
            "a PUT carrying Content-Encoding must never persist, even when the wire bytes happen to hash correctly"
        );
    }

    /// <summary>
    /// The other pre-fix failure mode the plan names: a real content-encoded
    /// body whose path hash is the (correct, per docs/BLOB_HTTP_SURFACE.md)
    /// hash of the PLAINTEXT never matches the compressed wire bytes, so
    /// pre-fix .NET 422s ("hash mismatch") instead of the 415 the header
    /// itself should have produced. Captured here so both pre-fix failure
    /// modes the plan describes have a named, checked test.
    /// </summary>
    [Fact]
    public async Task Put_with_content_encoding_header_and_plaintext_path_hash_is_rejected_415_not_422()
    {
        var plaintext = System.Text.Encoding.UTF8.GetBytes("plaintext this PUT pretends to be a zstd frame of, slice 3.4");
        var hash = Hasher.Hash(plaintext).ToString();
        var wireBytes = System.Text.Encoding.UTF8.GetBytes("stand-in compressed bytes, deliberately different from the plaintext above");

        using var request = new HttpRequestMessage(HttpMethod.Put, $"/blobs/{hash}") { Content = new ByteArrayContent(wireBytes) };
        request.Content.Headers.ContentEncoding.Add("zstd");

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.UnsupportedMediaType, response.StatusCode);
        Assert.False(File.Exists(Path.Combine(_originRoot!, "blake3", hash)));
    }

    /// <summary>
    /// Parity check on WHICH header values trigger the 415: Rust's
    /// <c>put_blob</c> rejects on <c>headers.contains_key(CONTENT_ENCODING)</c>
    /// — ANY value, including <c>identity</c> — not just <c>zstd</c>. .NET
    /// must match exactly rather than special-casing "identity" as exempt.
    /// </summary>
    [Fact]
    public async Task Put_with_content_encoding_identity_header_is_also_rejected_415()
    {
        var literalBytes = System.Text.Encoding.UTF8.GetBytes("identity-labeled bytes, slice 3.4 parity check");
        var hash = Hasher.Hash(literalBytes).ToString();

        using var request = new HttpRequestMessage(HttpMethod.Put, $"/blobs/{hash}") { Content = new ByteArrayContent(literalBytes) };
        request.Content.Headers.ContentEncoding.Add("identity");

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.UnsupportedMediaType, response.StatusCode);
        Assert.False(File.Exists(Path.Combine(_originRoot!, "blake3", hash)));
    }

    /// <summary>
    /// Slice 2.1 (nodalmerge-studio/plans/blob-cas-remediation.md) changed
    /// what "behaves as before" means for Content-Length specifically: HEAD
    /// used to answer via a full <c>TryGetBlobAsync</c> read (so it happened
    /// to know the real decompressed length), but that is exactly the
    /// hydration cost 2.1 removes — HEAD now answers via the cheap
    /// <c>ExistsAsync</c> probe, which cannot know the length without paying
    /// for the read it exists to avoid. This is not a regression: it brings
    /// .NET into parity with the Rust reference's <c>head_blob</c>
    /// (<c>blob_http.rs:256</c>), which has never set Content-Length on HEAD
    /// (only Content-Type + ETag on 200), and matches the frozen golden
    /// vector <c>head-found</c> (blob-http-surface-vectors.v1.json), which
    /// does not require one either.
    /// </summary>
    [Fact]
    public async Task Head_ignores_accept_encoding_and_behaves_as_before()
    {
        using var request = new HttpRequestMessage(HttpMethod.Head, $"/blobs/{_hash}");
        request.Headers.AcceptEncoding.Add(new StringWithQualityHeaderValue("zstd"));

        using var response = await _originClient!.SendAsync(request);

        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.DoesNotContain(response.Content.Headers.ContentEncoding, v => string.Equals(v, "zstd", StringComparison.OrdinalIgnoreCase));
        Assert.Null(response.Content.Headers.ContentLength);
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
