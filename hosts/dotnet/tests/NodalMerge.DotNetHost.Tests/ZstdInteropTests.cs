using System.Text.Json;
using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Cross-runtime zstd interop — Phase 0 slice 0.1 / Phase 3 slice 3.1 of
/// nodalmerge-studio/plans/blob-cas-remediation.md (finding #4). Reads the
/// golden fixtures in <c>engine/commands/fixtures/zstd-interop-v1/</c>
/// (manifest <c>engine/commands/zstd-interop-vectors.v1.json</c>, both
/// linked into this project) that were produced by the **other** runtime's
/// real at-rest encoder. Every prior <c>.zst</c> fixture/test was
/// same-runtime, which is exactly why finding #4 shipped: every frame from
/// Rust <c>zstd::stream::encode_all</c> carries no content-size header, and
/// <see cref="Decompressor.GetDecompressedSize"/> returns only an upper
/// bound for those (verified empirically against ZstdSharp.Port 0.8.1: it
/// returns the zstd default window size, 131072, never zero/error) — so
/// <see cref="BlobCompression.TryDecompress"/>'s old
/// <c>written == buffer.Length</c> check never held and every cross-runtime
/// compressed fetch was rejected as "corrupt zstd frame" until slice 3.1
/// relaxed it to <c>written &lt;= buffer.Length</c> (with the existing
/// Blake3 verify-after-decompress still catching real corruption).
///
/// The Rust mirror (which decodes the <c>.NET</c>-produced fixture instead,
/// and passes today without any production change) is
/// <c>server/server/tests/zstd_interop.rs</c>.
///
/// Goes through <see cref="FileBlobStoreProvider"/> via DI — the real
/// production GET path that internally calls
/// <see cref="BlobCompression.TryDecompress"/> — rather than calling
/// <c>BlobCompression</c> directly, since it is <c>internal</c> to
/// <c>NodalMerge.Host.Composition</c> and this test project has no
/// <c>InternalsVisibleTo</c> grant into that assembly (nor should one be
/// added just for this test — DI is how every other interop test in this
/// project reaches provider internals; see
/// <see cref="BlobLayoutInteropTests"/>).
/// </summary>
public sealed class ZstdInteropTests
{
    private sealed record FixtureVector(
        string Id,
        string ProducerRuntime,
        string ConsumerRuntime,
        string FixturePath,
        bool HasContentSizeHeader,
        int DecodedPlaintextLen,
        string DecodedPlaintextBlake3
    );

    private static readonly IReadOnlyList<FixtureVector> Fixtures;

    static ZstdInteropTests()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "zstd-interop-vectors.v1.json");
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var root = doc.RootElement;

        var fixtures = new List<FixtureVector>();
        foreach (var el in root.GetProperty("fixtures").EnumerateArray())
        {
            fixtures.Add(new FixtureVector(
                el.GetProperty("id").GetString()!,
                el.GetProperty("producer_runtime").GetString()!,
                el.GetProperty("consumer_runtime").GetString()!,
                el.GetProperty("fixture_path").GetString()!,
                el.GetProperty("has_content_size_header").GetBoolean(),
                el.GetProperty("decoded_plaintext_len").GetInt32(),
                el.GetProperty("decoded_plaintext_blake3").GetString()!
            ));
        }
        Fixtures = fixtures;
    }

    private static FixtureVector RequireFixture(string id)
    {
        return Fixtures.SingleOrDefault(f => f.Id == id)
            ?? throw new InvalidOperationException($"vector `{id}` not found in zstd-interop-vectors.v1.json");
    }

    /// <summary>
    /// Copies the fixture's raw <c>.zst</c> bytes into a fresh
    /// <see cref="FileBlobStoreProvider"/> root at the canonical encoded
    /// path (<c>&lt;root&gt;/blake3/&lt;hash&gt;.zst</c>) and reads it back
    /// through the DI-composed <see cref="IBlobStoreProvider"/> — the same
    /// GET path production traffic uses.
    /// </summary>
    private static async Task<BlobReadResult> ReadFixtureThroughProductionPathAsync(FixtureVector vector)
    {
        var fixtureFile = Path.Combine(
            AppContext.BaseDirectory,
            "zstd-interop-fixture-v1",
            Path.GetFileName(vector.FixturePath)
        );
        Assert.True(File.Exists(fixtureFile), $"fixture not found at {fixtureFile} — check the csproj Link entries");

        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-zstd-interop", Guid.NewGuid().ToString("N"));
        var blake3Dir = Path.Combine(root, "blake3");
        Directory.CreateDirectory(blake3Dir);
        File.Copy(fixtureFile, Path.Combine(blake3Dir, $"{vector.DecodedPlaintextBlake3}.zst"));

        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                ["NodalMerge:Providers:BlobStorage"] = "File",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = root
            })
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(configuration);
        await using var provider = services.BuildServiceProvider();
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        return await blobStore.TryGetBlobAsync(vector.DecodedPlaintextBlake3, CancellationToken.None);
    }

    /// <summary>
    /// Was RED (finding #4): a Rust <c>zstd::stream::encode_all</c> frame
    /// carries no content-size header, so
    /// <see cref="BlobCompression.TryDecompress"/> used to mis-size its
    /// output buffer from <see cref="Decompressor.GetDecompressedSize"/>'s
    /// upper bound and reject the frame as corrupt (confirmed failing with
    /// <c>Assert.True() Failure</c> on <c>result.Found</c> before slice 3.1 —
    /// see the slice 0.1 report for the captured output). Fixed by slice 3.1:
    /// <c>TryDecompress</c> now accepts <c>written &lt;= buffer.Length</c>
    /// and slices to the actual decoded length, relying on the caller's
    /// Blake3 verify-after-decompress to catch a genuinely wrong/corrupt
    /// frame instead of the buffer-length equality check.
    /// </summary>
    [Fact]
    public async Task DotNet_reads_rust_encoded_frame_without_content_size_header()
    {
        var vector = RequireFixture("rust-encode-all-no-size-header");
        Assert.Equal("rust", vector.ProducerRuntime);
        Assert.False(vector.HasContentSizeHeader);

        var result = await ReadFixtureThroughProductionPathAsync(vector);

        Assert.True(result.Found, "BlobCompression.TryDecompress should accept a header-less Rust zstd frame");
        Assert.Equal(vector.DecodedPlaintextLen, result.Bytes!.Length);
        Assert.Equal(vector.DecodedPlaintextBlake3, Hasher.Hash(result.Bytes!).ToString());
    }

    /// <summary>
    /// Distinct edge, NOT red today: an empty-payload Rust frame is also
    /// header-less, but <c>GetDecompressedSize</c>'s upper-bound guess
    /// coincidentally equals the true size (0 either way), so
    /// <c>written == buffer.Length</c> holds by accident and this one
    /// round-trips even with finding #4 present. Pins that "no
    /// content-size header" and "reproduces the bug" are not the same
    /// condition.
    /// </summary>
    [Fact]
    public async Task DotNet_reads_rust_encoded_empty_payload_frame()
    {
        var vector = RequireFixture("rust-encode-all-empty-payload");
        Assert.Equal("rust", vector.ProducerRuntime);
        Assert.False(vector.HasContentSizeHeader);
        Assert.Equal(0, vector.DecodedPlaintextLen);

        var result = await ReadFixtureThroughProductionPathAsync(vector);

        Assert.True(result.Found, "an empty-payload zstd frame should still decode as a hit, not missing");
        Assert.Empty(result.Bytes!);
        Assert.Equal(vector.DecodedPlaintextBlake3, Hasher.Hash(result.Bytes!).ToString());
    }
}
