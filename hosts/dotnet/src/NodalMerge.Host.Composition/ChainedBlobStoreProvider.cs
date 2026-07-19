using Blake3;
using NodalMerge.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Chains a local blob store (fast, may be cold) in front of a remote blob
/// origin (slow, authoritative), so a cold local store transparently
/// materializes from the origin on demand while writes land locally first.
/// This is the "local cache -> remote origin" half of slice 2.2
/// (nodalmerge-studio/plans/cas-distribution-and-storage.md) — selected via
/// <c>NodalMerge:Providers:BlobStorage = ChainedRemote</c>.
///
/// This is the ONLY place BLAKE3 verification of remote bytes happens.
/// <see cref="HttpRemoteBlobStoreProvider"/> deliberately does not verify —
/// verification lives here, once, so there is exactly one gate a corrupt
/// remote payload must pass before it can ever reach the local disk.
/// </summary>
public sealed class ChainedBlobStoreProvider : IBlobStoreProvider
{
    private readonly IBlobStoreProvider _local;
    private readonly IBlobStoreProvider _remote;
    private readonly ILogger<ChainedBlobStoreProvider> _logger;

    public ChainedBlobStoreProvider(
        IBlobStoreProvider local,
        IBlobStoreProvider remote,
        ILogger<ChainedBlobStoreProvider> logger
    )
    {
        _local = local;
        _remote = remote;
        _logger = logger;
    }

    public async ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        var localResult = await _local.TryGetBlobAsync(hashHex, cancellationToken);
        if (localResult.Found)
        {
            return localResult;
        }

        BlobReadResult remoteResult;
        try
        {
            remoteResult = await _remote.TryGetBlobAsync(hashHex, cancellationToken);
        }
        catch (Exception ex)
        {
            // Local-first availability: the remote origin is degraded or
            // unreachable, but that's not fatal — it just means this blob
            // isn't available right now. Callers see a plain miss.
            _logger.LogWarning(
                ex,
                "Remote blob origin fetch failed for {Hash}; treating as miss (local-first availability)",
                hashHex
            );
            return BlobReadResult.Missing;
        }

        if (!remoteResult.Found)
        {
            return BlobReadResult.Missing;
        }

        var actualHash = Hasher.Hash(remoteResult.Bytes!).ToString();
        if (!string.Equals(actualHash, hashHex, StringComparison.Ordinal))
        {
            // Corrupt (or malicious) remote payload. Never write this to the
            // local cache — surface it as a miss, exactly like docs/BLOB_HTTP_SURFACE.md
            // requires servers to serve a corrupt stored blob as 404, never as
            // wrong bytes.
            _logger.LogError(
                "Remote blob origin returned corrupted bytes for {Hash} (actual hash {ActualHash}); rejecting, never caching",
                hashHex,
                actualHash
            );
            return BlobReadResult.Missing;
        }

        // Write-through: the local cache now has it, so the next read is
        // local-only.
        await _local.PutBlobAsync(hashHex, remoteResult.Bytes!, remoteResult.ContentType, cancellationToken);
        return BlobReadResult.Hit(remoteResult.Bytes!, remoteResult.ContentType);
    }

    /// <summary>
    /// Cheap existence probe (slice 2.1): local first (typically
    /// <see cref="FileBlobStoreProvider"/>'s <see cref="File.Exists"/>
    /// check), falling through to the remote link's own cheap probe
    /// (<see cref="HttpRemoteBlobStoreProvider"/>'s HEAD,
    /// <see cref="S3DirectBlobStoreProvider"/>'s bucket HEAD, or — if the
    /// remote is a <c>RemoteBlobLinkAggregator</c> composing more than one
    /// link — whatever that aggregator's own <see cref="IBlobStoreProvider.ExistsAsync"/>
    /// resolves to, currently its default (<see cref="IBlobStoreProvider.TryGetBlobAsync"/>)
    /// since the aggregator does not itself override this member yet).
    /// A degraded/unreachable remote is treated as "not found" here, exactly
    /// like <see cref="TryGetBlobAsync"/>'s local-first-availability stance —
    /// never propagated as a fault, and — critically — never falls back to
    /// a full <see cref="TryGetBlobAsync"/> read, which would defeat the
    /// entire point of this method.
    /// </summary>
    public async ValueTask<bool> ExistsAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        if (await _local.ExistsAsync(hashHex, cancellationToken))
        {
            return true;
        }

        try
        {
            return await _remote.ExistsAsync(hashHex, cancellationToken);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(
                ex,
                "Remote blob origin existence probe failed for {Hash}; treating as absent (local-first availability)",
                hashHex
            );
            return false;
        }
    }

    public async ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        // Local write always happens first and its exceptions propagate —
        // callers need to know when a write didn't durably land anywhere.
        await _local.PutBlobAsync(hashHex, bytes, contentType, cancellationToken);

        try
        {
            await _remote.PutBlobAsync(hashHex, bytes, contentType, cancellationToken);
        }
        catch (Exception ex)
        {
            // The local write already succeeded, so this blob is not lost —
            // it's just not yet replicated to the origin. The reconcile
            // sweep (IRemoteBlobPushTarget) is the healing path for this
            // gap; failing the whole PutBlobAsync call here would be
            // misleading since the data IS safe locally.
            _logger.LogWarning(
                ex,
                "Remote blob origin push failed for {Hash}; local write succeeded, reconcile sweep will heal",
                hashHex
            );
        }
    }
}
