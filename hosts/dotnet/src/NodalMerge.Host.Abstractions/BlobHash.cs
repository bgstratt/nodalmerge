namespace NodalMerge.Host.Abstractions;

/// <summary>
/// Canonical on-disk/URL blob hash shape (docs/BLOB_STORAGE_LAYOUT.md §3):
/// exactly 64 lowercase hex characters (BLAKE3, hex-encoded). Anything else
/// is foreign — reject it on the write path, treat it as absent on the read
/// path, never adopt it. The .NET twin of Rust's
/// <c>is_canonical_blob_name</c> (<c>server/server/src/store.rs</c>).
/// </summary>
/// <remarks>
/// Slice 7.3 (blob-cas-remediation.md): this predicate was independently
/// duplicated three times —
/// <c>NodalMerge.DotNetHost.WebApplicationExtensions.IsCanonicalBlobHash</c>,
/// <c>NodalMerge.Host.Composition.FileBlobGcCoordinator.IsCanonicalHashName</c>,
/// and <c>NodalMerge.Host.Composition.FileBlobStoreProvider.IsCanonicalHash</c>
/// (added by slice 5.2). All three had IDENTICAL semantics (length 64,
/// lowercase hex only, uppercase rejected) modulo one detail: the
/// <c>WebApplicationExtensions</c> and <c>FileBlobGcCoordinator</c> copies
/// indexed <c>hash.Length</c> directly (throws on a null input); the 5.2
/// copy used a null-safe pattern (<c>hash is not { Length: 64 }</c>). No
/// call site ever passes a null hash (all three feed a route-bound or
/// on-disk-derived string), so the null-safe form was adopted everywhere
/// with no observed behavior change for any real input — see the slice
/// report for the audit trail.
///
/// This is NOT the same thing as
/// <c>FileBlobGcCoordinator.SanitizeHash</c>, a *deliberately* lenient
/// case/separator fold used only to match a live-set hash spelling against
/// on-disk names before GC decides what's live (over-protecting is the
/// safe failure mode there). That helper stays local and untouched —
/// folding it into this strict predicate would relax exactly the check its
/// own doc comment says must stay strict on the write path.
/// </remarks>
public static class BlobHash
{
    /// <summary>
    /// True iff <paramref name="hash"/> is exactly 64 lowercase hex
    /// characters (<c>0-9</c>, <c>a-f</c>). <see langword="null"/> and any
    /// other length or character return <see langword="false"/>.
    /// </summary>
    public static bool IsCanonical(string? hash)
    {
        if (hash is not { Length: 64 })
        {
            return false;
        }

        foreach (var c in hash)
        {
            if (c is (>= '0' and <= '9') or (>= 'a' and <= 'f'))
            {
                continue;
            }

            return false;
        }

        return true;
    }
}
