namespace NodalMerge.Host.Composition;

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

/// <summary>
/// Two-phase mark-and-sweep GC over the global content-addressed pool at
/// <c>&lt;root&gt;/blake3/&lt;hex&gt;</c>. See docs/BLOB_STORAGE_LAYOUT.md
/// §4. <c>liveHashes</c> must already be the union across every room the
/// caller knows about — this coordinator has no room concept at all.
/// </summary>
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

        var blake3Dir = Path.Combine(_rootPath, "blake3");
        var blobFiles = Directory.Exists(blake3Dir)
            ? Directory
                .EnumerateFiles(blake3Dir)
                .Where(path => IsCanonicalHashName(Path.GetFileName(path)))
                .ToArray()
            : [];

        var marked = new List<string>();
        var cleared = new List<string>();
        var deleted = new List<string>();
        var deleteCandidates = 0;

        var tombRoot = Path.Combine(_rootPath, ".tombstones", "blake3");
        if (applyChanges)
        {
            Directory.CreateDirectory(tombRoot);
        }

        foreach (var blobPath in blobFiles)
        {
            var safeHash = Path.GetFileName(blobPath);

            var tombPath = Path.Combine(tombRoot, safeHash);

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

    /// <summary>
    /// Anything under <c>blake3/</c> that is not exactly 64 lowercase hex
    /// characters is foreign and must never be touched by GC — see
    /// docs/BLOB_STORAGE_LAYOUT.md §3.
    /// </summary>
    private static bool IsCanonicalHashName(string name)
    {
        if (name.Length != 64)
        {
            return false;
        }

        foreach (var c in name)
        {
            var isLowerHex = (c is >= '0' and <= '9') || (c is >= 'a' and <= 'f');
            if (!isLowerHex)
            {
                return false;
            }
        }

        return true;
    }

    private static string SanitizeHash(string hashHex)
    {
        return FileBlobStoreProvider.SanitizeHash(hashHex);
    }
}
