using Blake3;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Auto-migration from the legacy sharded <c>&lt;shard&gt;/&lt;hex&gt;.blob</c>
/// layout to the canonical global <c>blake3/&lt;hex&gt;</c> layout — see
/// docs/BLOB_STORAGE_LAYOUT.md §6. The Rust mirror is
/// server/server/src/store.rs's <c>migrate_legacy_blob_layout</c> tests.
/// </summary>
public sealed class FileBlobStoreProviderMigrationTests
{
    [Fact]
    public async Task Migration_moves_legacy_blob_and_quarantines_non_conforming_entries()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));

        var bytes = System.Text.Encoding.UTF8.GetBytes("legacy blob content");
        var hash = Hasher.Hash(bytes).ToString();

        // Hand-construct the legacy layout: <root>/<shard>/<hash>.blob
        var shard = hash[..2];
        var legacyDir = Path.Combine(root, shard);
        Directory.CreateDirectory(legacyDir);
        await File.WriteAllBytesAsync(Path.Combine(legacyDir, hash + ".blob"), bytes);

        // A tampered/foreign file that must be quarantined, not adopted.
        await File.WriteAllTextAsync(Path.Combine(legacyDir, "not-a-hash.blob"), "junk");

        var configuration = BuildConfig(root);

        await using (var provider = BuildProvider(configuration))
        {
            var blobStore = provider.GetRequiredService<IBlobStoreProvider>();
            var read = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);
            Assert.True(read.Found, "migrated blob must be readable at its new canonical path");
            Assert.Equal(bytes, read.Bytes);
        }

        var migratedPath = Path.Combine(root, "blake3", hash);
        Assert.True(File.Exists(migratedPath));

        // Slice A1: a pass that quarantined something is not clean, so the
        // marker is deferred; the quarantined file has left the scan scope,
        // so the next construction converges and writes it.
        Assert.False(File.Exists(Path.Combine(root, ".layout-v2")));

        var quarantined = Directory.EnumerateFiles(
            Path.Combine(root, ".migration-skipped"),
            "*not-a-hash.blob",
            SearchOption.TopDirectoryOnly
        );
        Assert.Single(quarantined);

        await using (var second = BuildProvider(configuration))
        {
            second.GetRequiredService<IBlobStoreProvider>();
        }
        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));
    }

    [Fact]
    public async Task Migration_is_idempotent_across_repeated_opens()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));
        var bytes = System.Text.Encoding.UTF8.GetBytes("idempotent content");
        var hash = Hasher.Hash(bytes).ToString();
        var shard = hash[..2];
        var legacyDir = Path.Combine(root, shard);
        Directory.CreateDirectory(legacyDir);
        await File.WriteAllBytesAsync(Path.Combine(legacyDir, hash + ".blob"), bytes);

        var configuration = BuildConfig(root);

        await using (var first = BuildProvider(configuration))
        {
            var blobStore = first.GetRequiredService<IBlobStoreProvider>();
            var read = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);
            Assert.True(read.Found);
        }

        // Re-opening must be a no-op: marker already present, no re-scan needed.
        await using (var second = BuildProvider(configuration))
        {
            var blobStore = second.GetRequiredService<IBlobStoreProvider>();
            var read = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);
            Assert.True(read.Found, "blob must still be readable after a second open");
            Assert.Equal(bytes, read.Bytes);
        }
    }

    [Fact]
    public async Task Migration_is_noop_on_fresh_store()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));
        var configuration = BuildConfig(root);

        await using var provider = BuildProvider(configuration);
        // DI registers the provider lazily — resolving it is what triggers
        // the ctor (and therefore the migration scan).
        provider.GetRequiredService<IBlobStoreProvider>();
        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));
    }

    [Fact]
    public async Task Migration_repairs_corrupt_destination_from_verified_legacy_bytes()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));

        var bytes = System.Text.Encoding.UTF8.GetBytes("good legacy bytes behind a corrupt dest");
        var hash = Hasher.Hash(bytes).ToString();

        var legacyDir = Path.Combine(root, hash[..2]);
        Directory.CreateDirectory(legacyDir);
        await File.WriteAllBytesAsync(Path.Combine(legacyDir, hash + ".blob"), bytes);

        // A partial/corrupt copy already sits at the canonical destination —
        // the shape an interrupted earlier pass leaves behind. The dedupe
        // branch must BLAKE3-verify it before trusting it (Rust slice 5.1).
        var blake3Dir = Path.Combine(root, "blake3");
        Directory.CreateDirectory(blake3Dir);
        var dest = Path.Combine(blake3Dir, hash);
        await File.WriteAllBytesAsync(dest, System.Text.Encoding.UTF8.GetBytes("partial garbage"));

        var configuration = BuildConfig(root);
        await using (var provider = BuildProvider(configuration))
        {
            var blobStore = provider.GetRequiredService<IBlobStoreProvider>();
            var read = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);
            Assert.True(read.Found, "dest must have been repaired from the verified legacy bytes, not adopted as-is");
            Assert.Equal(bytes, read.Bytes);
        }

        Assert.Equal(bytes, await File.ReadAllBytesAsync(dest));
        Assert.False(
            File.Exists(Path.Combine(legacyDir, hash + ".blob")),
            "legacy duplicate must be dropped once the dest verifiably holds the bytes"
        );
        // A successful repair is a clean disposition — the marker is earned.
        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));
    }

    [Fact]
    public async Task Migration_defers_marker_when_a_quarantine_occurs_and_converges_on_next_pass()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));

        var goodBytes = System.Text.Encoding.UTF8.GetBytes("good sibling of a corrupt file");
        var goodHash = Hasher.Hash(goodBytes).ToString();

        // A hash-named legacy file whose bytes do NOT hash to its name —
        // genuinely corrupt, quarantined (terminal). Distinct content so its
        // real hash can't collide with anything else in the store.
        var corruptName = Hasher.Hash(System.Text.Encoding.UTF8.GetBytes("name donor")).ToString();

        var legacyDir = Path.Combine(root, goodHash[..2]);
        Directory.CreateDirectory(legacyDir);
        await File.WriteAllBytesAsync(Path.Combine(legacyDir, goodHash + ".blob"), goodBytes);
        await File.WriteAllBytesAsync(
            Path.Combine(legacyDir, corruptName + ".blob"),
            System.Text.Encoding.UTF8.GetBytes("tampered payload")
        );

        var configuration = BuildConfig(root);
        await using (var first = BuildProvider(configuration))
        {
            first.GetRequiredService<IBlobStoreProvider>();
        }

        // The good file migrated; the corrupt one was quarantined; the pass
        // was NOT clean, so .layout-v2 must be deferred to the next boot.
        Assert.True(File.Exists(Path.Combine(root, "blake3", goodHash)));
        var quarantined = Directory.EnumerateFiles(Path.Combine(root, ".migration-skipped"), "*" + corruptName + ".blob");
        Assert.Single(quarantined);
        Assert.False(
            File.Exists(Path.Combine(root, ".layout-v2")),
            "a pass with a quarantine must not write the marker"
        );

        // Second construction: the corrupt file is out of scan scope now, so
        // the pass is clean and the marker lands (self-converging).
        await using (var second = BuildProvider(configuration))
        {
            var blobStore = second.GetRequiredService<IBlobStoreProvider>();
            var read = await blobStore.TryGetBlobAsync(goodHash, CancellationToken.None);
            Assert.True(read.Found);
            Assert.Equal(goodBytes, read.Bytes);
        }

        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));
    }

    [Fact]
    public async Task Migration_leaves_unreadable_legacy_file_in_place_and_retries_next_construction()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));

        var bytes = System.Text.Encoding.UTF8.GetBytes("transiently locked legacy bytes");
        var hash = Hasher.Hash(bytes).ToString();

        var legacyDir = Path.Combine(root, hash[..2]);
        Directory.CreateDirectory(legacyDir);
        var legacyPath = Path.Combine(legacyDir, hash + ".blob");
        await File.WriteAllBytesAsync(legacyPath, bytes);

        var configuration = BuildConfig(root);

        // Simulate a transient reader lock (AV scan etc.): FileShare.None
        // makes ReadAllBytes fail during the first construction.
        await using (var locked = new FileStream(legacyPath, FileMode.Open, FileAccess.Read, FileShare.None))
        {
            await using var first = BuildProvider(configuration);
            first.GetRequiredService<IBlobStoreProvider>();
        }

        // Transient failure is NOT terminal: the file must stay put (not
        // quarantined) and the marker must be deferred so it's retried.
        Assert.True(File.Exists(legacyPath), "unreadable legacy file must be left in place, not quarantined");
        var skippedDir = Path.Combine(root, ".migration-skipped");
        if (Directory.Exists(skippedDir))
        {
            Assert.Empty(Directory.EnumerateFiles(skippedDir, "*" + hash + ".blob"));
        }
        Assert.False(
            File.Exists(Path.Combine(root, ".layout-v2")),
            "a pass with a read failure must not write the marker"
        );

        // Lock released — the next construction migrates it and converges.
        await using (var second = BuildProvider(configuration))
        {
            var blobStore = second.GetRequiredService<IBlobStoreProvider>();
            var read = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);
            Assert.True(read.Found, "retry after the transient lock must migrate the file");
            Assert.Equal(bytes, read.Bytes);
        }

        Assert.True(File.Exists(Path.Combine(root, "blake3", hash)));
        Assert.False(File.Exists(legacyPath));
        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));
    }

    [Fact]
    public async Task Migration_deletes_legacy_duplicate_when_destination_already_holds_the_bytes()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));

        var bytes = System.Text.Encoding.UTF8.GetBytes("already migrated content");
        var hash = Hasher.Hash(bytes).ToString();

        var legacyDir = Path.Combine(root, hash[..2]);
        Directory.CreateDirectory(legacyDir);
        var legacyPath = Path.Combine(legacyDir, hash + ".blob");
        await File.WriteAllBytesAsync(legacyPath, bytes);

        // Dest already holds the correct bytes — the collision IS the dedup.
        var blake3Dir = Path.Combine(root, "blake3");
        Directory.CreateDirectory(blake3Dir);
        await File.WriteAllBytesAsync(Path.Combine(blake3Dir, hash), bytes);

        var configuration = BuildConfig(root);
        await using (var provider = BuildProvider(configuration))
        {
            provider.GetRequiredService<IBlobStoreProvider>();
        }

        Assert.False(File.Exists(legacyPath), "verified duplicate must be dropped");
        Assert.Equal(bytes, await File.ReadAllBytesAsync(Path.Combine(blake3Dir, hash)));
        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));
    }

    [Fact]
    public async Task Migration_does_not_rescan_once_marker_is_present()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-migration", Guid.NewGuid().ToString("N"));
        var configuration = BuildConfig(root);

        await using (var first = BuildProvider(configuration))
        {
            first.GetRequiredService<IBlobStoreProvider>();
        }
        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));

        // A stray legacy-shaped file planted AFTER the marker must be left
        // untouched: the marker short-circuits the scan entirely.
        var bytes = System.Text.Encoding.UTF8.GetBytes("stray post-marker file");
        var hash = Hasher.Hash(bytes).ToString();
        var legacyDir = Path.Combine(root, hash[..2]);
        Directory.CreateDirectory(legacyDir);
        var strayPath = Path.Combine(legacyDir, hash + ".blob");
        await File.WriteAllBytesAsync(strayPath, bytes);

        await using (var second = BuildProvider(configuration))
        {
            second.GetRequiredService<IBlobStoreProvider>();
        }

        Assert.True(File.Exists(strayPath), "marker present means no rescan; the stray file stays put");
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash)));
    }

    private static IConfiguration BuildConfig(string root)
    {
        return new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "InMemory",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = root
                }
            )
            .Build();
    }

    private static ServiceProvider BuildProvider(IConfiguration configuration)
    {
        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(configuration);
        return services.BuildServiceProvider();
    }
}
