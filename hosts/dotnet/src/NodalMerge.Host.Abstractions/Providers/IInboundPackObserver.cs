namespace NodalMerge.Host.Abstractions.Providers;

/// <summary>
/// Optional, additive seam: notified after a genuinely peer-authored inbound "pack" message has
/// been imported into the engine and persisted on the server-side WebSocket path (a peer connected
/// to this host's <c>/ws/{room}</c> endpoint, not this host acting as a client of someone else's
/// room). Never invoked for this host's own outbound/rebroadcast pack traffic.
///
/// Resolved via DI as <c>IEnumerable&lt;IInboundPackObserver&gt;</c> — zero registered observers
/// (the default) reproduces today's behavior exactly. A throwing observer must never break the
/// WebSocket loop or the pack's persistence, which has already completed by the time observers run;
/// see <c>RuntimeWebSocketLoopRunner</c>'s per-observer try/catch.
/// </summary>
public interface IInboundPackObserver
{
    ValueTask OnInboundPackAppliedAsync(string roomId, string nodesB64, CancellationToken cancellationToken = default);
}
