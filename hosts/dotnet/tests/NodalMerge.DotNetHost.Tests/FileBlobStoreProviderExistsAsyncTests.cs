using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 2.1 (nodalmerge-studio/plans/blob-cas-remediation.md):
/// <see cref="FileBlobStoreProvider.ExistsAsync"/> is a cheap
/// <see cref="File.Exists"/> probe — no read, no decompress, no BLAKE3
/// verify — mirroring the Rust reference's <c>DirPersistence::has_blob</c>
/// (<c>store.rs:719</c>). Covers both on-disk shapes (identity, zstd-encoded)
/// plus the corrupt-blob-still-reports-present parity note documented on the
/// method itself.
/// </summary>
public sealed class FileBlobStoreProviderExistsAsyncTests
{
    private static readonly byte[] CompressiblePayload = System.Text.Encoding.UTF8.GetBytes(
        string.Concat(Enumerable.Repeat("{\"room\":\"room-a\",\"kind\":\"node\",\"value\":12345},", 2500))
    );

    [Fact]
    public async Task Identity_stored_blob_exists()
    {
        var root = NewRoot();
        await using var provider = BuildProvider(root, compression: "Off");
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        var bytes = System.Text.Encoding.UTF8.GetBytes("small, incompressible-by-policy blob");
        var hash = Hasher.Hash(bytes).ToString();
        await blobStore.PutBlobAsync(hash, bytes, null, CancellationToken.None);

        Assert.True(await blobStore.ExistsAsync(hash));
    }

    [Fact]
    public async Task Zstd_encoded_blob_exists_without_decompressing()
    {
        var root = NewRoot();
        await using var provider = BuildProvider(root, compression: "Zstd");
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        var hash = Hasher.Hash(CompressiblePayload).ToString();
        await blobStore.PutBlobAsync(hash, CompressiblePayload, null, CancellationToken.None);
        Assert.True(File.Exists(Path.Combine(root, "blake3", hash + ".zst")), "seed setup check: should have landed zstd-encoded");

        Assert.True(await blobStore.ExistsAsync(hash));
    }

    [Fact]
    public async Task Absent_blob_does_not_exist()
    {
        var root = NewRoot();
        await using var provider = BuildProvider(root, compression: "Off");
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        Assert.False(await blobStore.ExistsAsync(new string('a', 64)));
    }

    /// <summary>
    /// Documents the parity note on <see cref="FileBlobStoreProvider.ExistsAsync"/>:
    /// a corrupt on-disk zstd frame still answers "exists" (matches Rust's
    /// <c>has_blob</c>, which also does not verify) even though
    /// <see cref="IBlobStoreProvider.TryGetBlobAsync"/> would treat the same
    /// blob as Missing once actually read.
    /// </summary>
    [Fact]
    public async Task Corrupt_encoded_blob_still_reports_present_via_ExistsAsync_even_though_a_real_read_would_treat_it_as_missing()
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
        onDisk[onDisk.Length / 2] ^= 0xFF;
        await File.WriteAllBytesAsync(encodedPath, onDisk);

        await using var reopened = BuildProvider(root, compression: "Zstd");
        var reopenedStore = reopened.GetRequiredService<IBlobStoreProvider>();

        Assert.True(await reopenedStore.ExistsAsync(hash), "ExistsAsync is a presence check, not an integrity check");
        var read = await reopenedStore.TryGetBlobAsync(hash, CancellationToken.None);
        Assert.False(read.Found, "a real read must still reject the corrupt frame");
    }

    private static string NewRoot()
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-blob-exists-async", Guid.NewGuid().ToString("N"));
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
