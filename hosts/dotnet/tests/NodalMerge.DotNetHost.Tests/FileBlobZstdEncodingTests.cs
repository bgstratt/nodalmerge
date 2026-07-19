using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// v3 at-rest zstd encoding (docs/BLOB_STORAGE_LAYOUT.md §8): round-trip
/// correctness, the skip heuristic, corrupt-frame handling, and v2/v3
/// store compatibility. Layout-shape assertions (encoded path, tombstone
/// keying, both-files-exist preference) live in the vector-driven
/// <c>BlobLayoutParityTests</c>; this file covers the byte-level behavior
/// those vectors don't.
/// </summary>
public sealed class FileBlobZstdEncodingTests
{
    // 100 KB of highly repetitive JSON — comfortably clears the default
    // 4096-byte floor and the 0.98 ratio guard.
    private static readonly byte[] CompressiblePayload = System.Text.Encoding.UTF8.GetBytes(
        string.Concat(Enumerable.Repeat("{\"room\":\"room-a\",\"kind\":\"node\",\"value\":12345},", 2500))
    );

    [Fact]
    public async Task Compressible_payload_is_stored_as_smaller_zst_file_and_reads_back_identical()
    {
        var root = NewRoot();
        var hash = Hasher.Hash(CompressiblePayload).ToString();

        await using var provider = BuildProvider(root, compression: "Zstd");
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        await blobStore.PutBlobAsync(hash, CompressiblePayload, null, CancellationToken.None);

        var identityPath = Path.Combine(root, "blake3", hash);
        var encodedPath = identityPath + ".zst";
        Assert.False(File.Exists(identityPath), "compressible payload should not be stored identity when compression is on");
        Assert.True(File.Exists(encodedPath), "compressible payload should be stored as <hex>.zst");
        Assert.True(new FileInfo(encodedPath).Length < CompressiblePayload.Length, "the .zst file should be smaller than the input");

        var read = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);
        Assert.True(read.Found);
        Assert.Equal(CompressiblePayload, read.Bytes);
    }

    [Fact]
    public async Task Corrupted_zst_file_on_disk_is_treated_as_missing_never_wrong_bytes()
    {
        var root = NewRoot();
        var hash = Hasher.Hash(CompressiblePayload).ToString();

        await using (var provider = BuildProvider(root, compression: "Zstd"))
        {
            var blobStore = provider.GetRequiredService<IBlobStoreProvider>();
            await blobStore.PutBlobAsync(hash, CompressiblePayload, null, CancellationToken.None);
        }

        var encodedPath = Path.Combine(root, "blake3", hash + ".zst");
        var onDisk = await File.ReadAllBytesAsync(encodedPath);
        // Flip a byte in the middle of the frame — keeps the magic header
        // intact (so the corruption isn't rejected at the "not a zstd frame
        // at all" stage) but breaks the checksum/decompressed content.
        onDisk[onDisk.Length / 2] ^= 0xFF;
        await File.WriteAllBytesAsync(encodedPath, onDisk);

        await using var reopened = BuildProvider(root, compression: "Zstd");
        var reopenedStore = reopened.GetRequiredService<IBlobStoreProvider>();
        var read = await reopenedStore.TryGetBlobAsync(hash, CancellationToken.None);

        Assert.False(read.Found, "a corrupt zstd frame must never be served as a hit");
    }

    [Fact]
    public async Task Payload_below_min_bytes_stays_identity_even_when_compression_is_on()
    {
        var root = NewRoot();
        var bytes = System.Text.Encoding.UTF8.GetBytes("tiny"); // well under the 4096 floor
        var hash = Hasher.Hash(bytes).ToString();

        await using var provider = BuildProvider(root, compression: "Zstd");
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        await blobStore.PutBlobAsync(hash, bytes, null, CancellationToken.None);

        Assert.True(File.Exists(Path.Combine(root, "blake3", hash)));
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash + ".zst")));
    }

    [Fact]
    public async Task Incompressible_random_bytes_stay_identity()
    {
        var root = NewRoot();
        var bytes = new byte[8192];
        new Random(1234).NextBytes(bytes);
        var hash = Hasher.Hash(bytes).ToString();

        await using var provider = BuildProvider(root, compression: "Zstd");
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        await blobStore.PutBlobAsync(hash, bytes, null, CancellationToken.None);

        Assert.True(File.Exists(Path.Combine(root, "blake3", hash)), "incompressible bytes must fail the ratio guard and stay identity");
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash + ".zst")));
    }

    [Fact]
    public async Task Compressed_media_content_type_stays_identity_even_when_compressible()
    {
        var root = NewRoot();
        // Compressible content, but declared as image/png — the skip
        // heuristic (docs/BLOB_STORAGE_LAYOUT.md §8) trusts the declared
        // content type over sampling.
        var hash = Hasher.Hash(CompressiblePayload).ToString();

        await using var provider = BuildProvider(root, compression: "Zstd");
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        await blobStore.PutBlobAsync(hash, CompressiblePayload, "image/png", CancellationToken.None);

        Assert.True(File.Exists(Path.Combine(root, "blake3", hash)));
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash + ".zst")));
    }

    [Fact]
    public async Task V2_store_written_with_compression_off_reads_identically_after_enabling_zstd()
    {
        var root = NewRoot();
        var hash = Hasher.Hash(CompressiblePayload).ToString();

        await using (var offProvider = BuildProvider(root, compression: "Off"))
        {
            var blobStore = offProvider.GetRequiredService<IBlobStoreProvider>();
            await blobStore.PutBlobAsync(hash, CompressiblePayload, null, CancellationToken.None);
        }

        Assert.True(File.Exists(Path.Combine(root, "blake3", hash)));
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash + ".zst")));

        // Re-open the same store root with Zstd now enabled — the old
        // identity blob must still be found first, unchanged.
        await using var zstdProvider = BuildProvider(root, compression: "Zstd");
        var reopenedStore = zstdProvider.GetRequiredService<IBlobStoreProvider>();
        var read = await reopenedStore.TryGetBlobAsync(hash, CancellationToken.None);

        Assert.True(read.Found);
        Assert.Equal(CompressiblePayload, read.Bytes);
    }

    [Fact]
    public void Invalid_compression_value_throws_with_clear_message()
    {
        var root = NewRoot();
        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                ["NodalMerge:Providers:BlobStorage"] = "File",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = root,
                ["NodalMerge:Storage:FileBlobs:Compression"] = "Lz4"
            })
            .Build();

        var services = new ServiceCollection();
        var ex = Assert.Throws<InvalidOperationException>(() => services.AddNodalMergeHostProviders(configuration));
        Assert.Contains("Compression", ex.Message, StringComparison.Ordinal);
    }

    private static string NewRoot()
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-blob-zstd-encoding", Guid.NewGuid().ToString("N"));
    }

    private static ServiceProvider BuildProvider(string root, string compression)
    {
        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                ["NodalMerge:Providers:BlobStorage"] = "File",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = root,
                ["NodalMerge:Storage:FileBlobs:Compression"] = compression
            })
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(configuration);
        return services.BuildServiceProvider();
    }
}
