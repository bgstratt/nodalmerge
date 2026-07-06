using Blake3;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Global content-addressed file blob store: <c>&lt;root&gt;/blake3/&lt;hex&gt;</c>,
/// flat, no sharding, no extension, no room segment — see
/// docs/BLOB_STORAGE_LAYOUT.md. Auto-migrates a legacy sharded
/// <c>&lt;shard&gt;/&lt;hex&gt;.blob</c> layout on construction.
/// </summary>
internal sealed class FileBlobStoreProvider : IBlobStoreProvider, IBlobUrlResolverProvider
{
    private readonly string _rootPath;

    public FileBlobStoreProvider(FileBlobStorageOptions options)
    {
        _rootPath = Path.GetFullPath(options.RootPath);
        Directory.CreateDirectory(_rootPath);
        MigrateLegacyLayoutIfNeeded(_rootPath);
    }

    public async ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        var path = GetPath(hashHex);
        if (!File.Exists(path))
        {
            return BlobReadResult.Missing;
        }

        var bytes = await File.ReadAllBytesAsync(path, cancellationToken);
        return BlobReadResult.Hit(bytes, contentType: null);
    }

    public async ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        var path = GetPath(hashHex);
        if (File.Exists(path))
        {
            // Content-addressed: an existing file with this hash already
            // holds these bytes.
            return;
        }

        var parent = Path.GetDirectoryName(path);
        if (!string.IsNullOrWhiteSpace(parent))
        {
            Directory.CreateDirectory(parent);
        }

        // Write via temp + rename so concurrent writers of the same blob
        // don't collide on the destination and readers never observe a
        // partially written file.
        var temp = Path.Combine(parent!, "." + Path.GetFileName(path) + "." + Guid.NewGuid().ToString("N") + ".tmp");
        await File.WriteAllBytesAsync(temp, bytes, cancellationToken);
        try
        {
            File.Move(temp, path);
        }
        catch (IOException) when (File.Exists(path))
        {
            // Lost the race to an identical write — that collision IS the
            // CAS dedup. Drop our temp copy.
            File.Delete(temp);
        }
    }

    public ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
        BlobPutUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult<PresignedBlobUrl?>(null);
    }

    public ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
        BlobGetUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult<PresignedBlobUrl?>(null);
    }

    private string GetPath(string hashHex)
    {
        return Path.Combine(_rootPath, "blake3", SanitizeHash(hashHex));
    }

    internal static string SanitizeHash(string hashHex)
    {
        return hashHex
            .Replace(':', '_')
            .Replace('/', '_')
            .Replace('\\', '_')
            .ToLowerInvariant();
    }

    /// <summary>
    /// One-time migration from the legacy sharded <c>&lt;shard&gt;/&lt;hex&gt;.blob</c>
    /// layout to the canonical global <c>blake3/&lt;hex&gt;</c> layout,
    /// guarded by a <c>.layout-v2</c> marker written last. Idempotent and
    /// crash-safe: a re-run finds already-migrated files at their
    /// destination and skips them (that collision IS the cross-room CAS
    /// dedup), and anything that fails hash verification is quarantined
    /// into <c>.migration-skipped/</c> rather than deleted. See
    /// docs/BLOB_STORAGE_LAYOUT.md §6.
    /// </summary>
    private static void MigrateLegacyLayoutIfNeeded(string rootPath)
    {
        var marker = Path.Combine(rootPath, ".layout-v2");
        if (File.Exists(marker))
        {
            return;
        }

        var blake3Dir = Path.Combine(rootPath, "blake3");
        var skippedDir = Path.Combine(rootPath, ".migration-skipped");

        foreach (var legacyFile in Directory.EnumerateFiles(rootPath, "*.blob", SearchOption.AllDirectories))
        {
            var fileName = Path.GetFileNameWithoutExtension(legacyFile);

            byte[] bytes;
            try
            {
                bytes = File.ReadAllBytes(legacyFile);
            }
            catch
            {
                Quarantine(legacyFile, skippedDir);
                continue;
            }

            var actualHash = Hasher.Hash(bytes).ToString();
            if (!string.Equals(actualHash, fileName, StringComparison.OrdinalIgnoreCase))
            {
                Quarantine(legacyFile, skippedDir);
                continue;
            }

            Directory.CreateDirectory(blake3Dir);
            var dest = Path.Combine(blake3Dir, actualHash);
            if (File.Exists(dest))
            {
                // Already present — this collision IS the cross-room
                // dedup the v2 layout is for. Drop the duplicate.
                File.Delete(legacyFile);
            }
            else
            {
                File.Move(legacyFile, dest);
            }
        }

        // Best-effort cleanup of now-empty legacy shard directories.
        foreach (var dir in Directory.EnumerateDirectories(rootPath))
        {
            var name = Path.GetFileName(dir);
            if (name == "blake3" || name.StartsWith('.'))
            {
                continue;
            }
            try
            {
                if (!Directory.EnumerateFileSystemEntries(dir).Any())
                {
                    Directory.Delete(dir);
                }
            }
            catch
            {
                // Non-empty or in use — leave it; harmless leftover.
            }
        }

        File.WriteAllText(marker, string.Empty);
    }

    private static void Quarantine(string legacyFile, string skippedDir)
    {
        try
        {
            Directory.CreateDirectory(skippedDir);
            var dest = Path.Combine(skippedDir, Guid.NewGuid().ToString("N") + "__" + Path.GetFileName(legacyFile));
            File.Move(legacyFile, dest);
        }
        catch
        {
            // Best effort — leave the file in place rather than lose it.
        }
    }
}
