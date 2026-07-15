using Blake3;
using Microsoft.Extensions.Logging;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Global content-addressed file blob store: <c>&lt;root&gt;/blake3/&lt;hex&gt;</c>,
/// flat, no sharding, no extension, no room segment — see
/// docs/BLOB_STORAGE_LAYOUT.md. Auto-migrates a legacy sharded
/// <c>&lt;shard&gt;/&lt;hex&gt;.blob</c> layout on construction. Optionally
/// stores blobs zstd-encoded at rest (§8) when
/// <c>NodalMerge:Storage:FileBlobs:Compression = Zstd</c>.
/// </summary>
internal sealed class FileBlobStoreProvider : IBlobStoreProvider, IBlobUrlResolverProvider, IEncodedBlobSource
{
    private readonly string _rootPath;
    private readonly FileBlobStorageOptions _options;
    private readonly ILogger<FileBlobStoreProvider>? _logger;

    public FileBlobStoreProvider(FileBlobStorageOptions options, ILogger<FileBlobStoreProvider>? logger = null)
    {
        _options = options;
        _rootPath = Path.GetFullPath(options.RootPath);
        _logger = logger;
        Directory.CreateDirectory(_rootPath);
        MigrateLegacyLayoutIfNeeded(_rootPath);
    }

    public async ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        // Identity always wins when both encodings exist (contract:
        // writers never do this, but readers must prefer identity).
        var path = GetPath(hashHex);
        if (File.Exists(path))
        {
            var identityBytes = await File.ReadAllBytesAsync(path, cancellationToken);
            return BlobReadResult.Hit(identityBytes, contentType: null);
        }

        var encodedPath = GetEncodedPath(hashHex);
        if (File.Exists(encodedPath))
        {
            var compressed = await File.ReadAllBytesAsync(encodedPath, cancellationToken);
            var decompressed = BlobCompression.TryDecompress(compressed);
            if (decompressed is null)
            {
                _logger?.LogWarning(
                    "Corrupt zstd frame for blob {Hash} at {Path}; treating as missing",
                    hashHex,
                    encodedPath
                );
                return BlobReadResult.Missing;
            }

            // Invariant (§8): the hash is always Blake3 of the
            // uncompressed bytes. Never serve wrong bytes.
            var actualHash = Hasher.Hash(decompressed).ToString();
            if (!string.Equals(actualHash, hashHex, StringComparison.Ordinal))
            {
                _logger?.LogWarning(
                    "Decompressed bytes for blob {Hash} at {Path} hash to {ActualHash} instead; treating as missing",
                    hashHex,
                    encodedPath,
                    actualHash
                );
                return BlobReadResult.Missing;
            }

            return BlobReadResult.Hit(decompressed, contentType: null);
        }

        return BlobReadResult.Missing;
    }

    /// <summary>
    /// Serves the stored encoding as-is (docs/BLOB_HTTP_SURFACE.md
    /// "Content encoding"): identity bytes when that's what's on disk, the
    /// raw zstd frame (never decompressed) when only the encoded form
    /// exists. Identity still wins when both exist.
    /// </summary>
    public async ValueTask<EncodedBlobResult> TryGetEncodedBlobAsync(string hashHex, CancellationToken ct = default)
    {
        var path = GetPath(hashHex);
        if (File.Exists(path))
        {
            var identityBytes = await File.ReadAllBytesAsync(path, ct);
            return EncodedBlobResult.Identity(identityBytes);
        }

        var encodedPath = GetEncodedPath(hashHex);
        if (File.Exists(encodedPath))
        {
            var compressed = await File.ReadAllBytesAsync(encodedPath, ct);
            return EncodedBlobResult.Zstd(compressed);
        }

        return EncodedBlobResult.Missing;
    }

    public async ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        var path = GetPath(hashHex);
        var encodedPath = GetEncodedPath(hashHex);
        if (File.Exists(path) || File.Exists(encodedPath))
        {
            // Content-addressed: an existing file with this hash already
            // holds these bytes, under either encoding.
            return;
        }

        var compress = string.Equals(_options.Compression, "Zstd", StringComparison.Ordinal)
            && BlobCompression.ShouldCompress(bytes, contentType, _options.CompressionMinBytes, _options.CompressionLevel);

        var targetPath = compress ? encodedPath : path;
        var payload = compress ? BlobCompression.Compress(bytes, _options.CompressionLevel) : bytes;

        var parent = Path.GetDirectoryName(targetPath);
        if (!string.IsNullOrWhiteSpace(parent))
        {
            Directory.CreateDirectory(parent);
        }

        // Write via temp + rename so concurrent writers of the same blob
        // don't collide on the destination and readers never observe a
        // partially written file.
        var temp = Path.Combine(parent!, "." + Path.GetFileName(targetPath) + "." + Guid.NewGuid().ToString("N") + ".tmp");
        await File.WriteAllBytesAsync(temp, payload, cancellationToken);
        try
        {
            File.Move(temp, targetPath);
        }
        catch (IOException) when (File.Exists(targetPath))
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

    /// <summary>The v3 zstd-encoded sibling of <see cref="GetPath"/> (docs/BLOB_STORAGE_LAYOUT.md §8).</summary>
    private string GetEncodedPath(string hashHex)
    {
        return GetPath(hashHex) + ".zst";
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
