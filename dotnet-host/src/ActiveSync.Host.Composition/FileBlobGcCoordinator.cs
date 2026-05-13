namespace ActiveSync.Host.Composition;

public sealed record FileBlobGcPolicy(
    TimeSpan GraceWindow,
    int MaxDeletesPerRun = 100,
    bool RequireTombstoneBeforeDelete = true
)
{
    public static FileBlobGcPolicy Default { get; } = new(
        GraceWindow: TimeSpan.FromHours(24),
        MaxDeletesPerRun: 100,
        RequireTombstoneBeforeDelete: true
    );
}

public sealed record FileBlobGcRunReport(
    int Scanned,
    int Marked,
    int Cleared,
    int DeleteCandidates,
    int Deleted,
    IReadOnlyList<string> MarkedHashes,
    IReadOnlyList<string> ClearedHashes,
    IReadOnlyList<string> DeletedHashes
);

public sealed class FileBlobGcCoordinator
{
    private readonly string _rootPath;
    private readonly FileBlobGcPolicy _policy;

    public FileBlobGcCoordinator(string rootPath, FileBlobGcPolicy? policy = null)
    {
        _rootPath = Path.GetFullPath(rootPath);
        _policy = policy ?? FileBlobGcPolicy.Default;
        Directory.CreateDirectory(_rootPath);
    }

    public FileBlobGcRunReport DryRun(IReadOnlyCollection<string> liveHashes, DateTimeOffset nowUtc)
    {
        return RunCore(liveHashes, nowUtc, applyChanges: false);
    }

    public FileBlobGcRunReport LiveRun(IReadOnlyCollection<string> liveHashes, DateTimeOffset nowUtc)
    {
        return RunCore(liveHashes, nowUtc, applyChanges: true);
    }

    private FileBlobGcRunReport RunCore(IReadOnlyCollection<string> liveHashes, DateTimeOffset nowUtc, bool applyChanges)
    {
        var liveSafe = new HashSet<string>(liveHashes.Select(SanitizeHash), StringComparer.Ordinal);
        var blobFiles = Directory
            .EnumerateFiles(_rootPath, "*.blob", SearchOption.AllDirectories)
            .Where(path => !path.Contains(Path.DirectorySeparatorChar + ".gc-tombstones" + Path.DirectorySeparatorChar, StringComparison.Ordinal))
            .ToArray();

        var marked = new List<string>();
        var cleared = new List<string>();
        var deleted = new List<string>();
        var deleteCandidates = 0;

        var tombRoot = Path.Combine(_rootPath, ".gc-tombstones");
        if (applyChanges)
        {
            Directory.CreateDirectory(tombRoot);
        }

        foreach (var blobPath in blobFiles)
        {
            var safeHash = Path.GetFileNameWithoutExtension(blobPath);
            if (string.IsNullOrWhiteSpace(safeHash))
            {
                continue;
            }

            var tombPath = Path.Combine(tombRoot, safeHash + ".tomb");

            if (liveSafe.Contains(safeHash))
            {
                if (File.Exists(tombPath))
                {
                    cleared.Add(safeHash);
                    if (applyChanges)
                    {
                        File.Delete(tombPath);
                    }
                }

                continue;
            }

            if (!File.Exists(tombPath))
            {
                marked.Add(safeHash);
                if (applyChanges)
                {
                    File.WriteAllText(tombPath, string.Empty);
                }
                continue;
            }

            if (_policy.RequireTombstoneBeforeDelete)
            {
                var tombAge = nowUtc - File.GetLastWriteTimeUtc(tombPath);
                if (tombAge < _policy.GraceWindow)
                {
                    continue;
                }
            }

            deleteCandidates += 1;
            if (deleted.Count >= _policy.MaxDeletesPerRun)
            {
                continue;
            }

            deleted.Add(safeHash);
            if (applyChanges)
            {
                File.Delete(blobPath);
                if (File.Exists(tombPath))
                {
                    File.Delete(tombPath);
                }
            }
        }

        return new FileBlobGcRunReport(
            Scanned: blobFiles.Length,
            Marked: marked.Count,
            Cleared: cleared.Count,
            DeleteCandidates: deleteCandidates,
            Deleted: deleted.Count,
            MarkedHashes: marked,
            ClearedHashes: cleared,
            DeletedHashes: deleted
        );
    }

    private static string SanitizeHash(string hashHex)
    {
        return hashHex
            .Replace(':', '_')
            .Replace('/', '_')
            .Replace('\\', '_');
    }
}
