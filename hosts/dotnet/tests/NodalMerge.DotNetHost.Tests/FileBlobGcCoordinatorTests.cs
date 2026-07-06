using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

public sealed class FileBlobGcCoordinatorTests
{
    // Canonical layout requires exactly 64 lowercase hex chars (see
    // docs/BLOB_STORAGE_LAYOUT.md §3) — repeated single hex digits keep
    // these readable while staying valid.
    private static readonly string LiveHash = new('1', 64);
    private static readonly string OrphanHash = new('2', 64);
    private static readonly string StaleHash = new('3', 64);
    private static readonly string Phase2Hash = new('4', 64);

    [Fact]
    public async Task DryRun_reports_mark_and_delete_candidates_without_mutating_files()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));

        await SeedBlobAsync(root, LiveHash, new byte[] { 1, 2, 3 });
        await SeedBlobAsync(root, OrphanHash, new byte[] { 4, 5, 6 });
        await SeedBlobAsync(root, StaleHash, new byte[] { 7, 8, 9 });

        var liveTomb = GetTombPath(root, LiveHash);
        Directory.CreateDirectory(Path.GetDirectoryName(liveTomb)!);
        await File.WriteAllTextAsync(liveTomb, string.Empty);

        var staleTomb = GetTombPath(root, StaleHash);
        Directory.CreateDirectory(Path.GetDirectoryName(staleTomb)!);
        await File.WriteAllTextAsync(staleTomb, string.Empty);
        File.SetLastWriteTimeUtc(staleTomb, DateTime.UtcNow.AddHours(-2));

        var gc = new FileBlobGcCoordinator(
            root,
            new FileBlobGcPolicy(GraceWindow: TimeSpan.FromHours(1), MaxDeletesPerRun: 10, RequireTombstoneBeforeDelete: true)
        );

        var report = gc.DryRun(new[] { LiveHash }, DateTimeOffset.UtcNow);

        Assert.Equal(3, report.Scanned);
        Assert.Equal(1, report.Marked);
        Assert.Equal(1, report.DeleteCandidates);
        Assert.Equal(1, report.Deleted);
        Assert.Contains(report.ClearedHashes, h => string.Equals(h, LiveHash, StringComparison.Ordinal));
        Assert.Contains(report.MarkedHashes, h => string.Equals(h, OrphanHash, StringComparison.Ordinal));
        Assert.Contains(report.DeletedHashes, h => string.Equals(h, StaleHash, StringComparison.Ordinal));

        Assert.True(File.Exists(GetBlobPath(root, OrphanHash)));
        Assert.True(File.Exists(GetBlobPath(root, StaleHash)));
        Assert.True(File.Exists(staleTomb));
    }

    [Fact]
    public async Task LiveRun_applies_mark_then_delete_after_grace_window()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        await SeedBlobAsync(root, Phase2Hash, new byte[] { 10, 11, 12 });

        var gc = new FileBlobGcCoordinator(
            root,
            new FileBlobGcPolicy(GraceWindow: TimeSpan.FromMilliseconds(1), MaxDeletesPerRun: 10, RequireTombstoneBeforeDelete: true)
        );

        var first = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);
        Assert.Equal(1, first.Marked);
        Assert.Equal(0, first.Deleted);
        Assert.True(File.Exists(GetBlobPath(root, Phase2Hash)));
        Assert.True(File.Exists(GetTombPath(root, Phase2Hash)));

        File.SetLastWriteTimeUtc(GetTombPath(root, Phase2Hash), DateTime.UtcNow.AddMinutes(-1));
        var second = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);
        Assert.Equal(1, second.Deleted);
        Assert.False(File.Exists(GetBlobPath(root, Phase2Hash)));
        Assert.False(File.Exists(GetTombPath(root, Phase2Hash)));
    }

    [Fact]
    public async Task RunCore_ignores_foreign_entries_under_blake3_dir()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        var blake3Dir = Path.Combine(root, "blake3");
        Directory.CreateDirectory(blake3Dir);
        // Not 64 lowercase hex chars — must be skipped, never marked/deleted.
        await File.WriteAllTextAsync(Path.Combine(blake3Dir, "not-a-hash"), "junk");
        await File.WriteAllTextAsync(Path.Combine(blake3Dir, new string('A', 64)), "uppercase-not-canonical");

        var gc = new FileBlobGcCoordinator(root);
        var report = gc.DryRun(Array.Empty<string>(), DateTimeOffset.UtcNow);

        Assert.Equal(0, report.Scanned);
        Assert.True(File.Exists(Path.Combine(blake3Dir, "not-a-hash")));
        Assert.True(File.Exists(Path.Combine(blake3Dir, new string('A', 64))));
    }

    private static async Task SeedBlobAsync(string root, string hash, byte[] bytes)
    {
        var path = GetBlobPath(root, hash);
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        await File.WriteAllBytesAsync(path, bytes);
    }

    private static string GetBlobPath(string root, string hash)
    {
        return Path.Combine(root, "blake3", hash);
    }

    private static string GetTombPath(string root, string hash)
    {
        return Path.Combine(root, ".tombstones", "blake3", hash);
    }
}
