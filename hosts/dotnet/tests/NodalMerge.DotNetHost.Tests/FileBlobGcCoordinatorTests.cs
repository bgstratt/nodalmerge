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

    [Fact]
    public async Task Unreferenced_zst_blob_is_tombstoned_then_deleted_after_grace()
    {
        // v3 (docs/BLOB_STORAGE_LAYOUT.md §8): a <hex>.zst entry is
        // canonical and must go through the same mark-and-sweep as a bare
        // identity blob, keyed by the bare hash.
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        var hash = new string('5', 64);
        await SeedBlobAsync(root, hash + ".zst", new byte[] { 1, 2, 3 });

        var gc = new FileBlobGcCoordinator(
            root,
            new FileBlobGcPolicy(GraceWindow: TimeSpan.FromMilliseconds(1), MaxDeletesPerRun: 10, RequireTombstoneBeforeDelete: true)
        );

        var first = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);
        Assert.Equal(1, first.Marked);
        Assert.Contains(first.MarkedHashes, h => string.Equals(h, hash, StringComparison.Ordinal));
        Assert.True(File.Exists(Path.Combine(root, "blake3", hash + ".zst")));
        // Tombstones are keyed by bare hex regardless of stored encoding.
        Assert.True(File.Exists(GetTombPath(root, hash)));

        File.SetLastWriteTimeUtc(GetTombPath(root, hash), DateTime.UtcNow.AddMinutes(-1));
        var second = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);

        Assert.Equal(1, second.Deleted);
        Assert.Contains(second.DeletedHashes, h => string.Equals(h, hash, StringComparison.Ordinal));
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash + ".zst")));
        Assert.False(File.Exists(GetTombPath(root, hash)));
    }

    [Fact]
    public async Task Referenced_zst_blob_clears_leftover_tombstone_without_deleting_it()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        var hash = new string('6', 64);
        await SeedBlobAsync(root, hash + ".zst", new byte[] { 4, 5, 6 });

        var tomb = GetTombPath(root, hash);
        Directory.CreateDirectory(Path.GetDirectoryName(tomb)!);
        await File.WriteAllTextAsync(tomb, string.Empty);

        var gc = new FileBlobGcCoordinator(root);
        var report = gc.LiveRun(new[] { hash }, DateTimeOffset.UtcNow);

        Assert.Equal(1, report.Cleared);
        Assert.True(File.Exists(Path.Combine(root, "blake3", hash + ".zst")), "live blob must survive");
        Assert.False(File.Exists(tomb), "stale tombstone for a live hash must be cleared");
    }

    [Fact]
    public async Task Gz_file_under_blake3_dir_is_foreign_and_never_touched()
    {
        // Only .zst is a recognized v3 encoding suffix (name_conformance_vectors_v3
        // in engine/commands/blob-layout-vectors.v1.json) — .gz stays foreign.
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        var hash = new string('7', 64);
        await SeedBlobAsync(root, hash + ".gz", new byte[] { 7, 8, 9 });

        var gc = new FileBlobGcCoordinator(
            root,
            new FileBlobGcPolicy(GraceWindow: TimeSpan.Zero, MaxDeletesPerRun: 10, RequireTombstoneBeforeDelete: false)
        );

        var report = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);

        Assert.Equal(0, report.Scanned);
        Assert.Equal(0, report.Marked);
        Assert.Equal(0, report.Deleted);
        Assert.True(File.Exists(Path.Combine(root, "blake3", hash + ".gz")), "a .gz sibling must never be touched by GC");
    }

    [Fact]
    public async Task Both_encodings_present_are_grouped_and_deleted_together_once_unreferenced()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        var hash = new string('8', 64);
        await SeedBlobAsync(root, hash, new byte[] { 1 });
        await SeedBlobAsync(root, hash + ".zst", new byte[] { 2 });

        var gc = new FileBlobGcCoordinator(
            root,
            new FileBlobGcPolicy(GraceWindow: TimeSpan.Zero, MaxDeletesPerRun: 10, RequireTombstoneBeforeDelete: false)
        );

        // First sweep marks (writes the one bare-hex tombstone covering
        // both encodings); second sweep deletes — same two-phase shape as
        // every other unreferenced blob (docs/BLOB_STORAGE_LAYOUT.md §4).
        var first = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);
        Assert.True(first.Scanned == 2, "both raw canonical entries should be scanned");
        Assert.True(first.Marked == 1, "both encodings for one hash count as a single logical blob to mark");

        var second = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);
        Assert.True(second.Deleted == 1, "both encodings for one hash count as a single logical blob to delete");
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash)));
        Assert.False(File.Exists(Path.Combine(root, "blake3", hash + ".zst")));
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
