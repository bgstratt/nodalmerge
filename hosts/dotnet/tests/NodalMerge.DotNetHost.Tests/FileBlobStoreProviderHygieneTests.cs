using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 5.2 (nodalmerge-studio/plans/blob-cas-remediation.md) — migration
/// and write-path hygiene for <see cref="FileBlobStoreProvider"/>:
/// <list type="number">
/// <item>a crash-before-marker restart must not re-discover already
/// quarantined files under <c>.migration-skipped/</c> and re-quarantine
/// them under ever-growing GUID names (mirrors the Rust migration's
/// reserved-subtree skip in <c>store.rs</c>'s
/// <c>migrate_legacy_blob_layout</c>);</item>
/// <item>a cancelled or failed <c>PutBlobAsync</c> must not leak its
/// <c>.{hash}.{guid}.tmp</c> file under <c>blake3/</c> — GC classifies
/// foreign names as untouchable, so a leaked temp file would sit there
/// forever;</item>
/// <item>a non-canonical hash (anything but 64 lowercase hex) is rejected
/// at the provider layer, never sanitized into a storable filename —
/// sanitized names diverge from Rust's strict <c>hash_from_hex</c>
/// (<c>store.rs</c>) and create GC-invisible, Rust-unreadable entries.
/// Writes throw <see cref="ArgumentException"/>; reads answer Missing /
/// false, matching Rust readers, which treat foreign names as absent.</item>
/// </list>
/// </summary>
public sealed class FileBlobStoreProviderHygieneTests
{
    // ---- 5.2.1: quarantined files must not be re-quarantined ------------

    [Fact]
    public async Task Crash_before_marker_restart_keeps_quarantine_names_stable()
    {
        var root = NewRoot();

        // A prior run quarantined a non-conforming legacy file and crashed
        // before writing .layout-v2 — so the next opens re-run the scan.
        var skippedDir = Path.Combine(root, ".migration-skipped");
        Directory.CreateDirectory(skippedDir);
        const string quarantinedName = "deadbeef00000000000000000000000f__junk.blob";
        await File.WriteAllTextAsync(Path.Combine(skippedDir, quarantinedName), "junk");

        // Plus an ordinary legacy blob so the scan has real work to do.
        var bytes = System.Text.Encoding.UTF8.GetBytes("legacy blob beside a quarantine");
        var hash = Hasher.Hash(bytes).ToString();
        var legacyDir = Path.Combine(root, hash[..2]);
        Directory.CreateDirectory(legacyDir);
        await File.WriteAllBytesAsync(Path.Combine(legacyDir, hash + ".blob"), bytes);

        var marker = Path.Combine(root, ".layout-v2");

        // Two crash-before-marker restarts: each open re-runs the migration.
        for (var run = 1; run <= 2; run++)
        {
            await using (var provider = BuildProvider(root))
            {
                provider.GetRequiredService<IBlobStoreProvider>();
            }

            var names = Directory
                .EnumerateFiles(skippedDir)
                .Select(Path.GetFileName)
                .OrderBy(n => n, StringComparer.Ordinal)
                .ToArray();
            Assert.True(
                names.Length == 1 && names[0] == quarantinedName,
                $"run {run}: quarantine must be left alone by the migration scan, but "
                    + $".migration-skipped now contains: [{string.Join(", ", names)}]"
            );

            // Simulate the next crash-before-marker restart.
            File.Delete(marker);
        }

        // The ordinary legacy blob still migrated normally.
        Assert.True(File.Exists(Path.Combine(root, "blake3", hash)));
    }

    // ---- 5.2.2: no .tmp leak under blake3/ on a failed/cancelled put ----

    /// <summary>
    /// Genuine mid-write cancellation isn't deterministically forcible —
    /// <c>File.WriteAllBytesAsync</c> observes an already-cancelled token
    /// before it even creates the file — so this injects a failure at the
    /// move step instead: the destination path is occupied by a directory,
    /// which makes <c>File.Move</c> throw a real <see cref="IOException"/>
    /// after the temp file was fully written (the same post-write failure
    /// shape a cancellation or rename error produces).
    /// </summary>
    [Fact]
    public async Task Failed_move_does_not_leak_tmp_and_still_propagates_the_error()
    {
        var root = NewRoot();
        var bytes = System.Text.Encoding.UTF8.GetBytes("blob whose landing spot is blocked");
        var hash = Hasher.Hash(bytes).ToString();

        var blake3Dir = Path.Combine(root, "blake3");
        // Occupy the destination with a DIRECTORY: File.Exists() says false
        // (so the put proceeds and writes its temp file), then File.Move
        // fails against it.
        Directory.CreateDirectory(Path.Combine(blake3Dir, hash));

        await using var provider = BuildProvider(root);
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        // Per 4.2's now-truthful PUT, the write error itself must still
        // propagate — cleanup must never swallow it.
        await Assert.ThrowsAsync<IOException>(
            () => blobStore.PutBlobAsync(hash, bytes, null, CancellationToken.None).AsTask()
        );

        var leaked = Directory
            .EnumerateFiles(blake3Dir, "*.tmp")
            .Select(Path.GetFileName)
            .ToArray();
        Assert.True(
            leaked.Length == 0,
            $"a failed put must clean up its temp file, but blake3/ now contains: [{string.Join(", ", leaked)}]"
        );
    }

    /// <summary>
    /// Companion pin, not itself RED: a token cancelled before the write
    /// starts surfaces as a cancellation and touches nothing on disk (this
    /// already held pre-fix, because WriteAllBytesAsync checks the token
    /// before creating the file).
    /// </summary>
    [Fact]
    public async Task Precancelled_put_propagates_cancellation_and_leaves_no_tmp()
    {
        var root = NewRoot();
        var bytes = System.Text.Encoding.UTF8.GetBytes("never written");
        var hash = Hasher.Hash(bytes).ToString();

        await using var provider = BuildProvider(root);
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        using var cts = new CancellationTokenSource();
        cts.Cancel();
        await Assert.ThrowsAnyAsync<OperationCanceledException>(
            () => blobStore.PutBlobAsync(hash, bytes, null, cts.Token).AsTask()
        );

        var blake3Dir = Path.Combine(root, "blake3");
        var leftovers = Directory.Exists(blake3Dir)
            ? Directory.EnumerateFiles(blake3Dir).Select(Path.GetFileName).ToArray()
            : [];
        Assert.True(
            leftovers.Length == 0,
            $"a cancelled put must leave nothing behind, but blake3/ contains: [{string.Join(", ", leftovers)}]"
        );
    }

    // ---- 5.2.3: reject-don't-sanitize non-canonical hashes --------------

    [Theory]
    [InlineData("sha256:abc")]
    [InlineData("ABC123")]
    [InlineData("../escape")]
    public async Task Non_canonical_hash_put_is_rejected_and_stores_nothing(string badHash)
    {
        var root = NewRoot();
        await using var provider = BuildProvider(root);
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        var ex = await Record.ExceptionAsync(
            () => blobStore.PutBlobAsync(badHash, [1, 2, 3], null, CancellationToken.None).AsTask()
        );

        var blake3Dir = Path.Combine(root, "blake3");
        var stored = Directory.Exists(blake3Dir)
            ? Directory.EnumerateFiles(blake3Dir).Select(Path.GetFileName).ToArray()
            : [];
        Assert.True(
            ex is ArgumentException,
            $"put of '{badHash}' must throw ArgumentException, got "
                + $"{ex?.GetType().Name ?? "no exception"}; blake3/ now contains: [{string.Join(", ", stored)}]"
        );
        Assert.Empty(stored);
    }

    /// <summary>
    /// An uppercase spelling of a REAL blob's hash is still non-canonical:
    /// pre-fix it was silently lowercased into the canonical path (a write
    /// that "worked" by accident); post-fix the caller must send the
    /// canonical form, exactly like Rust's <c>hash_from_hex</c>, which
    /// deliberately rejects uppercase rather than normalizing it.
    /// </summary>
    [Fact]
    public async Task Uppercase_spelling_of_a_real_hash_is_rejected_not_normalized()
    {
        var root = NewRoot();
        var bytes = System.Text.Encoding.UTF8.GetBytes("case matters at the provider boundary");
        var upperHash = Hasher.Hash(bytes).ToString().ToUpperInvariant();

        await using var provider = BuildProvider(root);
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        var ex = await Record.ExceptionAsync(
            () => blobStore.PutBlobAsync(upperHash, bytes, null, CancellationToken.None).AsTask()
        );

        var blake3Dir = Path.Combine(root, "blake3");
        var stored = Directory.Exists(blake3Dir)
            ? Directory.EnumerateFiles(blake3Dir).Select(Path.GetFileName).ToArray()
            : [];
        Assert.True(
            ex is ArgumentException,
            $"uppercase hash must be rejected, got {ex?.GetType().Name ?? "no exception"}; "
                + $"blake3/ now contains: [{string.Join(", ", stored)}]"
        );
        Assert.Empty(stored);
    }

    /// <summary>
    /// Reads mirror Rust readers, which treat foreign names as absent: a
    /// non-canonical hash definitionally cannot exist in the store, so it
    /// answers Missing / false — no sanitized path probe, no throw.
    /// </summary>
    [Fact]
    public async Task Non_canonical_hash_reads_answer_missing_not_sanitized()
    {
        var root = NewRoot();
        await using var provider = BuildProvider(root);
        var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

        var read = await blobStore.TryGetBlobAsync("sha256:abc", CancellationToken.None);
        Assert.False(read.Found);
        Assert.False(await blobStore.ExistsAsync("ABC123", CancellationToken.None));
    }

    // ---- helpers ---------------------------------------------------------

    private static string NewRoot()
    {
        return Path.Combine(Path.GetTempPath(), "nodalmerge-blob-hygiene", Guid.NewGuid().ToString("N"));
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
