using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

public sealed class FileBlobGcCoordinatorTests
{
    [Fact]
    public async Task DryRun_reports_mark_and_delete_candidates_without_mutating_files()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        var liveHash = "sha256:live";
        var orphanHash = "sha256:orphan";
        var staleHash = "sha256:stale";

        await SeedBlobAsync(root, liveHash, new byte[] { 1, 2, 3 });
        await SeedBlobAsync(root, orphanHash, new byte[] { 4, 5, 6 });
        await SeedBlobAsync(root, staleHash, new byte[] { 7, 8, 9 });

        var liveTomb = GetTombPath(root, liveHash);
        Directory.CreateDirectory(Path.GetDirectoryName(liveTomb)!);
        await File.WriteAllTextAsync(liveTomb, string.Empty);

        var staleTomb = GetTombPath(root, staleHash);
        Directory.CreateDirectory(Path.GetDirectoryName(staleTomb)!);
        await File.WriteAllTextAsync(staleTomb, string.Empty);
        File.SetLastWriteTimeUtc(staleTomb, DateTime.UtcNow.AddHours(-2));

        var gc = new FileBlobGcCoordinator(
            root,
            new FileBlobGcPolicy(GraceWindow: TimeSpan.FromHours(1), MaxDeletesPerRun: 10, RequireTombstoneBeforeDelete: true)
        );

        var report = gc.DryRun(new[] { liveHash }, DateTimeOffset.UtcNow);

        Assert.Equal(3, report.Scanned);
        Assert.Equal(1, report.Marked);
        Assert.Equal(1, report.DeleteCandidates);
        Assert.Equal(1, report.Deleted);
        Assert.Contains(report.ClearedHashes, h => string.Equals(h, Sanitize(liveHash), StringComparison.Ordinal));
        Assert.Contains(report.MarkedHashes, h => string.Equals(h, Sanitize(orphanHash), StringComparison.Ordinal));
        Assert.Contains(report.DeletedHashes, h => string.Equals(h, Sanitize(staleHash), StringComparison.Ordinal));

        Assert.True(File.Exists(GetBlobPath(root, orphanHash)));
        Assert.True(File.Exists(GetBlobPath(root, staleHash)));
        Assert.True(File.Exists(staleTomb));
    }

    [Fact]
    public async Task LiveRun_applies_mark_then_delete_after_grace_window()
    {
        var root = Path.Combine(Path.GetTempPath(), "nodalmerge-file-gc-tests", Guid.NewGuid().ToString("N"));
        var orphanHash = "sha256:phase2";
        await SeedBlobAsync(root, orphanHash, new byte[] { 10, 11, 12 });

        var gc = new FileBlobGcCoordinator(
            root,
            new FileBlobGcPolicy(GraceWindow: TimeSpan.FromMilliseconds(1), MaxDeletesPerRun: 10, RequireTombstoneBeforeDelete: true)
        );

        var first = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);
        Assert.Equal(1, first.Marked);
        Assert.Equal(0, first.Deleted);
        Assert.True(File.Exists(GetBlobPath(root, orphanHash)));
        Assert.True(File.Exists(GetTombPath(root, orphanHash)));

        File.SetLastWriteTimeUtc(GetTombPath(root, orphanHash), DateTime.UtcNow.AddMinutes(-1));
        var second = gc.LiveRun(Array.Empty<string>(), DateTimeOffset.UtcNow);
        Assert.Equal(1, second.Deleted);
        Assert.False(File.Exists(GetBlobPath(root, orphanHash)));
        Assert.False(File.Exists(GetTombPath(root, orphanHash)));
    }

    private static async Task SeedBlobAsync(string root, string hash, byte[] bytes)
    {
        var safe = Sanitize(hash);
        var shard = safe.Length >= 2 ? safe[..2] : "00";
        var path = Path.Combine(root, shard, safe + ".blob");
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        await File.WriteAllBytesAsync(path, bytes);
    }

    private static string GetBlobPath(string root, string hash)
    {
        var safe = Sanitize(hash);
        var shard = safe.Length >= 2 ? safe[..2] : "00";
        return Path.Combine(root, shard, safe + ".blob");
    }

    private static string GetTombPath(string root, string hash)
    {
        return Path.Combine(root, ".gc-tombstones", Sanitize(hash) + ".tomb");
    }

    private static string Sanitize(string hash)
    {
        return hash.Replace(':', '_').Replace('/', '_').Replace('\\', '_');
    }
}
