using Blake3;
using Microsoft.Extensions.Logging;
using NodalMerge.Host.Abstractions;
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
        // Slice 5.2: a non-canonical hash definitionally cannot exist in
        // the store (writes reject it, see GetPath), so reads answer
        // Missing rather than throw — matching Rust readers, which treat
        // foreign names as absent (docs/BLOB_STORAGE_LAYOUT.md §3).
        if (!BlobHash.IsCanonical(hashHex))
        {
            return BlobReadResult.Missing;
        }

        // Identity always wins when both encodings exist (contract:
        // writers never do this, but readers must prefer identity).
        var path = GetPath(hashHex);
        if (File.Exists(path))
        {
            var identityBytes = await File.ReadAllBytesAsync(path, cancellationToken);

            // Slice 3.2 (finding #13): verify-on-read, matching the zstd
            // branch below and Rust's DirPersistence::get_blob (store.rs:683)
            // — on a hash mismatch, log and treat as missing. Never delete
            // the file, never throw: a corrupt blob is Missing, the same
            // outcome an absent file would produce, so the HTTP origin's
            // "corrupt → 404, never wrong bytes" contract holds without any
            // extra plumbing at the call site.
            var actualHash = Hasher.Hash(identityBytes).ToString();
            if (!string.Equals(actualHash, hashHex, StringComparison.Ordinal))
            {
                _logger?.LogWarning(
                    "Corrupt identity blob file for {Hash} at {Path} hashes to {ActualHash} instead; treating as missing",
                    hashHex,
                    path,
                    actualHash
                );
                return BlobReadResult.Missing;
            }

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
    /// Cheap existence probe (slice 2.1): a plain <see cref="File.Exists"/>
    /// check against both possible on-disk locations, no read/decompress/
    /// verify — mirrors the Rust reference's
    /// <c>DirPersistence::has_blob</c> (<c>store.rs:719</c>), which is
    /// exactly this same two-path <c>is_file</c> check with no
    /// verification either. Note the resulting parity: a blob whose on-disk
    /// bytes are corrupt still answers "exists" here (same as Rust) — HEAD
    /// and PUT-idempotency are existence checks, not integrity re-checks;
    /// <see cref="TryGetBlobAsync"/> is what looks at the actual bytes and
    /// filters out corruption.
    /// </summary>
    public ValueTask<bool> ExistsAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        // Slice 5.2: non-canonical → false, same posture as TryGetBlobAsync.
        if (!BlobHash.IsCanonical(hashHex))
        {
            return ValueTask.FromResult(false);
        }

        var exists = File.Exists(GetPath(hashHex)) || File.Exists(GetEncodedPath(hashHex));
        return ValueTask.FromResult(exists);
    }

    /// <summary>
    /// Serves the stored encoding as-is (docs/BLOB_HTTP_SURFACE.md
    /// "Content encoding"): identity bytes when that's what's on disk, the
    /// raw zstd frame (never decompressed) when only the encoded form
    /// exists. Identity still wins when both exist.
    /// </summary>
    public async ValueTask<EncodedBlobResult> TryGetEncodedBlobAsync(string hashHex, CancellationToken ct = default)
    {
        // Slice 5.2: non-canonical → Missing, same posture as TryGetBlobAsync.
        if (!BlobHash.IsCanonical(hashHex))
        {
            return EncodedBlobResult.Missing;
        }

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
        try
        {
            await File.WriteAllBytesAsync(temp, payload, cancellationToken);
            try
            {
                File.Move(temp, targetPath);
            }
            catch (IOException) when (File.Exists(targetPath))
            {
                // Lost the race to an identical write — that collision IS
                // the CAS dedup. The finally below drops our temp copy.
            }
        }
        finally
        {
            // Slice 5.2: a cancelled or failed write must not leak its
            // temp file under blake3/ — GC classifies foreign names as
            // untouchable, so a leaked .tmp would sit there forever. Best
            // effort only: the write error itself still propagates (4.2's
            // truthful PUT); only the cleanup failure is swallowed.
            if (File.Exists(temp))
            {
                try
                {
                    File.Delete(temp);
                }
                catch (Exception cleanupEx)
                {
                    _logger?.LogWarning(
                        cleanupEx,
                        "Failed to clean up temp blob file {TempPath} after an unsuccessful write",
                        temp
                    );
                }
            }
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
        // Slice 5.2: reject-don't-sanitize. This used to mangle whatever it
        // was handed into a storable filename (':'/'/'/'\\' → '_', then
        // lowercase), which diverges from Rust's strict hash_from_hex
        // (store.rs) and creates entries GC classifies as foreign and the
        // Rust host can't read. The provider boundary now matches Rust:
        // exactly 64 lowercase hex, uppercase deliberately rejected rather
        // than normalized.
        if (!BlobHash.IsCanonical(hashHex))
        {
            throw new ArgumentException(
                $"Blob hash must be exactly 64 lowercase hex characters, got '{hashHex}'.",
                nameof(hashHex)
            );
        }

        return Path.Combine(_rootPath, "blake3", hashHex);
    }

    /// <summary>The v3 zstd-encoded sibling of <see cref="GetPath"/> (docs/BLOB_STORAGE_LAYOUT.md §8).</summary>
    private string GetEncodedPath(string hashHex)
    {
        return GetPath(hashHex) + ".zst";
    }

    /// <summary>
    /// One-time migration from the legacy sharded <c>&lt;shard&gt;/&lt;hex&gt;.blob</c>
    /// layout to the canonical global <c>blake3/&lt;hex&gt;</c> layout,
    /// guarded by a <c>.layout-v2</c> marker written last — and, mirroring
    /// Rust slice 5.1 (<c>migrate_legacy_blob_layout</c>, store.rs), written
    /// ONLY when the pass was fully clean. Idempotent and crash-safe: a
    /// re-run finds already-migrated files at their destination and drops
    /// the duplicate only after BLAKE3-verifying the destination really
    /// holds those bytes (a partial copy left by an interrupted pass is
    /// repaired from the verified legacy source instead of adopted).
    /// Read failures are treated as transient (AV lock etc.): the file
    /// stays in place and the deferred marker makes the next construction
    /// retry it. Only a genuine content-hash mismatch is terminal and
    /// quarantined into <c>.migration-skipped/</c> rather than deleted —
    /// and even then the marker is deferred once; the quarantined file has
    /// left the scan scope, so the next pass is clean and converges. See
    /// docs/BLOB_STORAGE_LAYOUT.md §6.
    /// </summary>
    private void MigrateLegacyLayoutIfNeeded(string rootPath)
    {
        var marker = Path.Combine(rootPath, ".layout-v2");
        if (File.Exists(marker))
        {
            return;
        }

        var blake3Dir = Path.Combine(rootPath, "blake3");
        var skippedDir = Path.Combine(rootPath, ".migration-skipped");

        // Slice A1 (dotnet-host-readiness): any deferral — read failure,
        // quarantine, failed repair/move — leaves the marker absent so the
        // next construction rescans. A dirty pass only costs a cheap
        // TopDirectoryOnly re-scan per boot.
        var dirty = false;

        // Slice 5.2: enumerate legacy shard files only — never the canonical
        // blake3/ tree or any dot-directory (.migration-skipped,
        // .tombstones). The old AllDirectories scan re-discovered files
        // already parked under .migration-skipped on every
        // crash-before-marker restart and re-quarantined them under
        // ever-growing GUID names. Mirrors the Rust migration's
        // reserved-subtree skip (store.rs, migrate_legacy_blob_layout).
        var legacyFiles = Directory
            .EnumerateFiles(rootPath, "*.blob", SearchOption.TopDirectoryOnly)
            .Concat(
                Directory
                    .EnumerateDirectories(rootPath)
                    .Where(dir =>
                    {
                        var name = Path.GetFileName(dir);
                        return name != "blake3" && !name.StartsWith('.');
                    })
                    .SelectMany(dir => Directory.EnumerateFiles(dir, "*.blob", SearchOption.AllDirectories))
            );

        foreach (var legacyFile in legacyFiles)
        {
            var fileName = Path.GetFileNameWithoutExtension(legacyFile);

            byte[] bytes;
            try
            {
                bytes = File.ReadAllBytes(legacyFile);
            }
            catch (Exception ex)
            {
                // A read failure may be transient (AV scanner holding the
                // file, etc.) — quarantining here would be terminal for a
                // perfectly good blob. Leave it in place and defer the
                // marker so the next construction retries it.
                dirty = true;
                _logger?.LogWarning(
                    ex,
                    "Legacy blob migration: cannot read {LegacyFile}; leaving it in place and deferring .layout-v2 for a retry next construction",
                    legacyFile
                );
                continue;
            }

            var actualHash = Hasher.Hash(bytes).ToString();
            if (!string.Equals(actualHash, fileName, StringComparison.OrdinalIgnoreCase))
            {
                // Genuinely corrupt (bytes don't hash to the name) —
                // terminal, quarantine rather than delete. Still defer the
                // marker this pass; the quarantined file leaves the scan
                // scope, so the next pass converges cleanly.
                dirty = true;
                _logger?.LogWarning(
                    "Legacy blob migration: {LegacyFile} hashes to {ActualHash} instead of its name; quarantining to {SkippedDir}",
                    legacyFile,
                    actualHash,
                    skippedDir
                );
                Quarantine(legacyFile, skippedDir);
                continue;
            }

            Directory.CreateDirectory(blake3Dir);
            var dest = Path.Combine(blake3Dir, actualHash);
            if (File.Exists(dest))
            {
                // Already present — this collision IS the cross-room dedup
                // the v2 layout is for — but verify the dest really holds
                // these bytes before dropping the duplicate: an interrupted
                // earlier marker-less pass can leave a partial file here,
                // and deleting the legacy source on the strength of a
                // partial copy would destroy the only good copy (Rust 5.1).
                bool destOk;
                try
                {
                    var destBytes = File.ReadAllBytes(dest);
                    destOk = string.Equals(
                        Hasher.Hash(destBytes).ToString(),
                        actualHash,
                        StringComparison.Ordinal
                    );
                }
                catch
                {
                    destOk = false;
                }

                if (!destOk)
                {
                    // Repair from the already-verified legacy bytes via
                    // write-tmp-then-atomic-replace: at no point is there
                    // no good copy on disk (the legacy file stays until
                    // the replace has landed).
                    var temp = Path.Combine(
                        blake3Dir,
                        "." + actualHash + "." + Guid.NewGuid().ToString("N") + ".tmp"
                    );
                    try
                    {
                        File.WriteAllBytes(temp, bytes);
                        File.Move(temp, dest, overwrite: true);
                        _logger?.LogWarning(
                            "Legacy blob migration: repaired partial/corrupt destination {Dest} from verified legacy source {LegacyFile}",
                            dest,
                            legacyFile
                        );
                    }
                    catch (Exception ex)
                    {
                        // Repair failed — the legacy file is the only good
                        // copy; leave it in place and defer the marker.
                        dirty = true;
                        _logger?.LogWarning(
                            ex,
                            "Legacy blob migration: cannot repair partial destination {Dest}; leaving legacy source {LegacyFile} in place and deferring",
                            dest,
                            legacyFile
                        );
                        TryDeleteTempFile(temp);
                        continue;
                    }
                }

                if (!TryDeleteLegacyDuplicate(legacyFile))
                {
                    dirty = true;
                }
            }
            else
            {
                try
                {
                    File.Move(legacyFile, dest);
                }
                catch (Exception ex)
                {
                    dirty = true;
                    _logger?.LogWarning(
                        ex,
                        "Legacy blob migration: cannot move {LegacyFile} to {Dest}; leaving it in place and deferring",
                        legacyFile,
                        dest
                    );
                    continue;
                }
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

        if (dirty)
        {
            // Not a clean pass — leave .layout-v2 absent so the next
            // construction rescans and retries the deferred entries
            // (mirrors Rust 5.1's deferred-count gate on the marker).
            _logger?.LogWarning(
                "Legacy blob migration: incomplete pass; leaving .layout-v2 absent so the next construction retries"
            );
            return;
        }

        File.WriteAllText(marker, string.Empty);
    }

    private bool TryDeleteLegacyDuplicate(string legacyFile)
    {
        try
        {
            File.Delete(legacyFile);
            return true;
        }
        catch (Exception ex)
        {
            // The dest verifiably holds the bytes, so nothing is lost —
            // but the leftover duplicate must defer the marker so a later
            // pass can finish the dedup.
            _logger?.LogWarning(
                ex,
                "Legacy blob migration: destination verified but could not delete legacy duplicate {LegacyFile}; deferring",
                legacyFile
            );
            return false;
        }
    }

    private void TryDeleteTempFile(string temp)
    {
        try
        {
            if (File.Exists(temp))
            {
                File.Delete(temp);
            }
        }
        catch (Exception ex)
        {
            _logger?.LogWarning(
                ex,
                "Legacy blob migration: failed to clean up temp repair file {TempPath}",
                temp
            );
        }
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
