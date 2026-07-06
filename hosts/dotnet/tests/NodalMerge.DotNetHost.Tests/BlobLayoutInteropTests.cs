using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Cross-runtime interop test — .NET reads the same golden blob-store
/// fixture (<c>engine/commands/fixtures/blob-layout-v1/</c>, linked into
/// this project) that the Rust mirror
/// (<c>server/server/tests/blob_layout_interop.rs</c>) reads. Neither
/// runtime writes the fixture; each just proves it can interpret the
/// other's canonical on-disk shape. See docs/BLOB_STORAGE_LAYOUT.md.
///
/// Unlike Rust's <c>DirPersistence</c> (which nests the blob root one
/// level under <c>&lt;store_root&gt;/blobs/</c>), .NET's
/// <see cref="FileBlobStorageOptions.RootPath"/> *is* the blob root
/// directly — so this test points straight at a copy of the fixture, no
/// extra nesting.
/// </summary>
public sealed class BlobLayoutInteropTests
{
    private const string BlobOneHash = "30db28cf6e7c0df6cf6226f68f364649236cd3a6470823ea78d759f95da49da4";
    private const string BlobOneContent = "fixture blob one";
    private const string BlobTwoHash = "70dd491e791ac7b073c5ac5e343b9bc803f3d1e1b7038c1233531070a07b0b31";
    private const string BlobTwoContent = "fixture blob two";

    [Fact]
    public async Task DotNet_reads_the_shared_golden_fixture()
    {
        var fixture = Path.Combine(AppContext.BaseDirectory, "blob-layout-fixture-v1");
        Assert.True(Directory.Exists(fixture), $"fixture not found at {fixture} — check the csproj Link entries");

        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-blob-interop", Guid.NewGuid().ToString("N"));
        CopyDirectory(fixture, root);

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

        var blobOne = await blobStore.TryGetBlobAsync(BlobOneHash, CancellationToken.None);
        Assert.True(blobOne.Found, "blob one must be readable from the fixture");
        Assert.Equal(BlobOneContent, System.Text.Encoding.UTF8.GetString(blobOne.Bytes!));

        var blobTwo = await blobStore.TryGetBlobAsync(BlobTwoHash, CancellationToken.None);
        Assert.True(blobTwo.Found, "blob two must be readable from the fixture even though it is also tombstoned");
        Assert.Equal(BlobTwoContent, System.Text.Encoding.UTF8.GetString(blobTwo.Bytes!));

        // Migration must be a no-op: the fixture already carries `.layout-v2`.
        Assert.False(
            Directory.Exists(Path.Combine(root, ".migration-skipped")),
            "a canonical fixture must never produce migration quarantine output"
        );
    }

    private static void CopyDirectory(string sourceDir, string destDir)
    {
        Directory.CreateDirectory(destDir);
        foreach (var file in Directory.EnumerateFiles(sourceDir))
        {
            File.Copy(file, Path.Combine(destDir, Path.GetFileName(file)));
        }
        foreach (var dir in Directory.EnumerateDirectories(sourceDir))
        {
            CopyDirectory(dir, Path.Combine(destDir, Path.GetFileName(dir)));
        }
    }
}
