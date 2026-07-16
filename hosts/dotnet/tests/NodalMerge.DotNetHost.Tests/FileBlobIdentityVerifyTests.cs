using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 3.2 (nodalmerge-studio/plans/blob-cas-remediation.md, finding #13):
/// <see cref="FileBlobStoreProvider.TryGetBlobAsync"/>'s identity path (no
/// <c>.zst</c> sibling) used to return on-disk bytes with no BLAKE3
/// verification, unlike the zstd path in the same method (which has always
/// verified after decompress, see <c>FileBlobZstdEncodingTests</c>) and unlike
/// the Rust reference <c>DirPersistence::get_blob</c> (<c>store.rs:683</c>),
/// which verifies the identity branch and returns <c>None</c> (logging a
/// warning) on a hash mismatch — it never deletes the file and never throws.
/// This file pins the .NET identity path to the same contract:
/// docs/BLOB_HTTP_SURFACE.md's "corrupt → 404, never wrong bytes."
/// </summary>
public sealed class FileBlobIdentityVerifyTests
{
    [Fact]
    public async Task Tampered_identity_blob_file_is_treated_as_missing_never_wrong_bytes()
    {
        var root = NewRoot();
        var originalBytes = System.Text.Encoding.UTF8.GetBytes("the real, untampered identity blob contents");
        var hash = Hasher.Hash(originalBytes).ToString();

        await using (var provider = BuildProvider(root))
        {
            var blobStore = provider.GetRequiredService<IBlobStoreProvider>();
            await blobStore.PutBlobAsync(hash, originalBytes, null, CancellationToken.None);
        }

        var identityPath = Path.Combine(root, "blake3", hash);
        Assert.True(File.Exists(identityPath), "seed setup check: should have landed identity, not zstd-encoded");

        // Tamper the on-disk bytes in place, at the path keyed by the
        // ORIGINAL hash — the file no longer matches its own filename.
        await File.WriteAllBytesAsync(identityPath, System.Text.Encoding.UTF8.GetBytes("attacker-controlled substituted bytes!!"));

        await using var reopened = BuildProvider(root);
        var reopenedStore = reopened.GetRequiredService<IBlobStoreProvider>();
        var read = await reopenedStore.TryGetBlobAsync(hash, CancellationToken.None);

        Assert.False(read.Found, "a tampered identity blob must never be served as a hit — it must read as Missing, matching the zstd path and Rust's get_blob");
    }

    /// <summary>
    /// The verify must not change behavior for the "file genuinely isn't
    /// there" case — still Missing, not some new error shape. Not itself a
    /// RED test for the fix (this already passed pre-fix); recorded here as
    /// an explicit regression guard alongside the tamper test above.
    /// </summary>
    [Fact]
    public async Task Absent_identity_blob_stays_missing()
    {
        var root = NewRoot();
        await using var provider = BuildProvider(root);
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        var read = await blobStore.TryGetBlobAsync(new string('a', 64), CancellationToken.None);

        Assert.False(read.Found);
    }

    /// <summary>
    /// Sanity/regression companion to the tamper test: an untampered
    /// identity-stored blob must still read back correctly once the verify
    /// is in place — the fix must not make every identity read fail.
    /// </summary>
    [Fact]
    public async Task Untampered_identity_blob_still_reads_correctly()
    {
        var root = NewRoot();
        var bytes = System.Text.Encoding.UTF8.GetBytes("perfectly ordinary, unmodified identity blob");
        var hash = Hasher.Hash(bytes).ToString();

        await using var provider = BuildProvider(root);
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();
        await blobStore.PutBlobAsync(hash, bytes, null, CancellationToken.None);

        var read = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);

        Assert.True(read.Found);
        Assert.Equal(bytes, read.Bytes);
    }

    private static string NewRoot()
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-blob-identity-verify", Guid.NewGuid().ToString("N"));
    }

    private static ServiceProvider BuildProvider(string root)
    {
        var configuration = new ConfigurationBuilder()
            .AddInMemoryCollection(new Dictionary<string, string?>
            {
                ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                ["NodalMerge:Providers:BlobStorage"] = "File",
                ["NodalMerge:Storage:FileBlobs:RootPath"] = root,
                ["NodalMerge:Storage:FileBlobs:Compression"] = "Off"
            })
            .Build();

        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(configuration);
        return services.BuildServiceProvider();
    }
}
