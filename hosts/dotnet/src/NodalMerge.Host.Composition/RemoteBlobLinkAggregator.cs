using NodalMerge.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Fallback (GET) + fan-out (PUT) aggregate over multiple unverified remote
/// blob links — e.g. server-relay (<see cref="HttpRemoteBlobStoreProvider"/>)
/// and s3-direct (<see cref="S3DirectBlobStoreProvider"/>) — added in slice
/// 4.3 (nodalmerge-studio/plans/cas-distribution-and-storage.md Phase 4) so
/// the chain can grow from two links to three (<c>local -> server-relay
/// -> s3-direct</c>) while <see cref="ChainedBlobStoreProvider"/> still only
/// ever talks to ONE "remote" argument. That is the whole point of this
/// class: it keeps exactly one BLAKE3 verification gate in the composed
/// chain, regardless of how many remote links are configured — this
/// aggregator, like each of its members, does NOT verify.
///
/// <b>GET (fallback, ordered):</b> tries each link in the order given to the
/// constructor and returns the first hit. A link that misses (not found) or
/// throws (degraded/unreachable) just falls through to the next one — link
/// order is therefore a pure availability/cost decision (try the cheaper
/// server-relay hop before the bucket), never a correctness one, since every
/// link's bytes are verified identically once they reach the chain. Only
/// when every link has missed or failed does this return
/// <see cref="BlobReadResult.Missing"/>.
///
/// <b>PUT (fan-out, parallel):</b> pushes to every configured link
/// independently. Each link's failure is logged and does not fail the
/// others or the overall call — generalizing
/// <see cref="ChainedBlobStoreProvider"/>'s own "local write already
/// succeeded, so a remote push failure just means the reconcile sweep has
/// healing to do" stance from one remote to N. Keeping every remote link
/// warm on every put (rather than e.g. "s3-direct first, relay only on
/// s3-direct failure") also means the existing reconcile-sweep target
/// (<see cref="IRemoteBlobPushTarget"/>, still wired to the relay link only
/// — see <c>ServiceCollectionExtensions</c>) never develops a permanent gap
/// just because s3-direct happened to be healthy.
/// </summary>
public sealed class RemoteBlobLinkAggregator : IBlobStoreProvider
{
    private readonly IReadOnlyList<IBlobStoreProvider> _links;
    private readonly ILogger<RemoteBlobLinkAggregator> _logger;

    public RemoteBlobLinkAggregator(IReadOnlyList<IBlobStoreProvider> links, ILogger<RemoteBlobLinkAggregator> logger)
    {
        if (links.Count == 0)
        {
            throw new ArgumentException("RemoteBlobLinkAggregator requires at least one remote link", nameof(links));
        }

        _links = links;
        _logger = logger;
    }

    public async ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        foreach (var link in _links)
        {
            BlobReadResult result;
            try
            {
                result = await link.TryGetBlobAsync(hashHex, cancellationToken);
            }
            catch (Exception ex)
            {
                _logger.LogWarning(
                    ex,
                    "Remote blob link {Link} fetch failed for {Hash}; falling through to the next configured link",
                    link.GetType().Name,
                    hashHex
                );
                continue;
            }

            if (result.Found)
            {
                return result;
            }
        }

        return BlobReadResult.Missing;
    }

    /// <summary>
    /// Slice 2.1 — existence probe, ordered-fallback like
    /// <see cref="TryGetBlobAsync"/> but never hydrating bytes. Overriding this is not
    /// optional: this aggregator IS the single "remote" argument
    /// <see cref="ChainedBlobStoreProvider"/> sees whenever more than one remote link is
    /// configured, so without an override it would inherit
    /// <see cref="IBlobStoreProvider"/>'s compat default — which answers existence by
    /// calling <see cref="TryGetBlobAsync"/>, putting a full remote download (+ verify +
    /// local write-back) back on the HEAD/PUT-idempotency path for precisely the
    /// three-link <c>local -&gt; server-relay -&gt; s3-direct</c> deployment that most
    /// needs the cheap probe.
    ///
    /// A throwing link is treated as "ask the next one", exactly as in
    /// <see cref="TryGetBlobAsync"/>: a degraded link is an availability event, not
    /// evidence about the blob. Consequently a <c>false</c> here means "no reachable link
    /// has it", which is the same guarantee <see cref="TryGetBlobAsync"/>'s
    /// <see cref="BlobReadResult.Missing"/> already carries.
    /// </summary>
    public async ValueTask<bool> ExistsAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        foreach (var link in _links)
        {
            try
            {
                if (await link.ExistsAsync(hashHex, cancellationToken))
                {
                    return true;
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(
                    ex,
                    "Remote blob link {Link} existence probe failed for {Hash}; falling through to the next configured link",
                    link.GetType().Name,
                    hashHex
                );
            }
        }

        return false;
    }

    public async ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        var pushes = _links.Select(link => PushOneAsync(link, hashHex, bytes, contentType, cancellationToken));
        await Task.WhenAll(pushes);
    }

    private async Task PushOneAsync(
        IBlobStoreProvider link,
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken
    )
    {
        try
        {
            await link.PutBlobAsync(hashHex, bytes, contentType, cancellationToken);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(
                ex,
                "Remote blob link {Link} push failed for {Hash}; other configured links are unaffected and the reconcile sweep will heal this one",
                link.GetType().Name,
                hashHex
            );
        }
    }
}
