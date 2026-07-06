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
        Assert.True(File.Exists(Path.Combine(root, ".layout-v2")));

        var quarantined = Directory.EnumerateFiles(
            Path.Combine(root, ".migration-skipped"),
            "*not-a-hash.blob",
            SearchOption.TopDirectoryOnly
        );
        Assert.Single(quarantined);
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
