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
        var canonicalFiles = Directory.Exists(blake3Dir)
            ? Directory
                .EnumerateFiles(blake3Dir)
                .Select(path => (Path: path, BareHash: TryGetCanonicalBareHash(Path.GetFileName(path))))
                .Where(entry => entry.BareHash is not null)
                .ToArray()
            : [];

        // Group by bare hash so a hash stored under both encodings (§8:
        // "writers never do this", but GC must still cope) is processed
        // once — the live-set / tombstone keying is always the bare hash.
        var groups = canonicalFiles
            .GroupBy(entry => entry.BareHash!, StringComparer.Ordinal)
            .ToArray();

        var marked = new List<string>();
        var cleared = new List<string>();
        var deleted = new List<string>();
        var deleteCandidates = 0;

        var tombRoot = Path.Combine(_rootPath, ".tombstones", "blake3");
        if (applyChanges)
        {
            Directory.CreateDirectory(tombRoot);
        }

        foreach (var group in groups)
        {
            var bareHash = group.Key;
            var tombPath = Path.Combine(tombRoot, bareHash);

            if (liveSafe.Contains(bareHash))
            {
                if (File.Exists(tombPath))
                {
                    cleared.Add(bareHash);
                    if (applyChanges)
                    {
                        File.Delete(tombPath);
                    }
                }

                continue;
            }

            if (!File.Exists(tombPath))
            {
                marked.Add(bareHash);
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

            deleted.Add(bareHash);
            if (applyChanges)
            {
                // Deletes whichever encoding file(s) exist for this hash.
                foreach (var entry in group)
                {
                    File.Delete(entry.Path);
                }
                if (File.Exists(tombPath))
                {
                    File.Delete(tombPath);
                }
            }
        }

        return new FileBlobGcRunReport(
            Scanned: canonicalFiles.Length,
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
    /// characters, or that (v3, §8) has exactly one <c>.zst</c> suffix over
    /// 64 lowercase hex characters, is foreign and must never be touched by
    /// GC — see docs/BLOB_STORAGE_LAYOUT.md §3 and §8's
    /// <c>name_conformance_vectors_v3</c>. Returns the bare hash (suffix
    /// stripped) for a canonical name, or <c>null</c> for a foreign one.
    /// </summary>
    private static string? TryGetCanonicalBareHash(string name)
    {
        if (IsCanonicalHashName(name))
        {
            return name;
        }

        const string zstSuffix = ".zst";
        if (name.EndsWith(zstSuffix, StringComparison.Ordinal))
        {
            var candidate = name[..^zstSuffix.Length];
            if (IsCanonicalHashName(candidate))
            {
                return candidate;
            }
        }

        return null;
    }

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

    /// <summary>
    /// Deliberately lenient where the provider's write path is strict
    /// (slice 5.2 made <see cref="FileBlobStoreProvider"/> reject
    /// non-canonical hashes): a live hash arriving in a non-canonical
    /// spelling can only ever OVER-protect here — its folded form either
    /// matches a real on-disk name or nothing at all — and over-protecting
    /// is the safe failure mode for a GC live set, while dropping the
    /// entry could sweep a blob a sloppy caller genuinely references.
    /// </summary>
    private static string SanitizeHash(string hashHex)
    {
        return hashHex
            .Replace(':', '_')
            .Replace('/', '_')
            .Replace('\\', '_')
            .ToLowerInvariant();
    }
}
