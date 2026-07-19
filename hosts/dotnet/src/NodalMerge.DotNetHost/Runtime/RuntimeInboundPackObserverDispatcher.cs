using System.Diagnostics.Metrics;
using System.Threading.Channels;
using Microsoft.Extensions.Logging;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.DotNetHost.Runtime;

/// <summary>
/// Tuning knobs for how <see cref="RuntimeWebSocketLoopRunner"/> dispatches
/// <see cref="IInboundPackObserver"/> notifications off the WebSocket receive loop — slice 6.5
/// (nodalmerge-studio/plans/blob-cas-remediation.md). Additive: hosts that never touch this get
/// the defaults below; it is also resolvable through DI (optional constructor parameter on
/// <c>RuntimeWebSocketLoopRunner</c>) for hosts that want to tune it.
/// </summary>
public sealed class RuntimeInboundPackObserverDispatchOptions
{
    /// <summary>
    /// Per-connection bound on queued observer notifications. When the queue is full the OLDEST
    /// queued notification is dropped (never the newest — see the class comment on
    /// <see cref="RuntimeInboundPackObserverDispatcher"/> for why drop-oldest is the right
    /// policy for this hook) and the drop is counted + logged. The receive loop itself never
    /// blocks on this queue.
    /// </summary>
    public int QueueCapacity { get; init; } = 512;

    /// <summary>
    /// Per-observer, per-notification budget. Independent of connection teardown: the token an
    /// observer receives is cancelled by THIS timeout (or by dispatcher shutdown), never by the
    /// connection's own receive-loop token — a closing connection does not yank an observer
    /// mid-flight, and a slow observer does not keep a connection alive.
    /// </summary>
    public TimeSpan ObserverTimeout { get; init; } = TimeSpan.FromSeconds(30);

    /// <summary>
    /// How long connection teardown waits for already-queued notifications to finish before
    /// hard-cancelling the worker. Bounds the teardown cost a slow observer can impose on
    /// <c>RunAsync</c>'s finally block.
    /// </summary>
    public TimeSpan DrainGrace { get; init; } = TimeSpan.FromSeconds(5);
}

/// <summary>
/// Slice 6.5 (nodalmerge-studio/plans/blob-cas-remediation.md): the S7.1 observer hook used to be
/// awaited INLINE in the WS receive loop — the per-observer try/catch isolated exceptions but not
/// latency, so a slow/hung observer stalled every further frame on that connection. This
/// dispatcher moves observer invocation onto its own per-connection worker:
///
/// - One bounded channel + one consumer task per CONNECTION (created lazily on the first inbound
///   pack, torn down in <c>RunAsync</c>'s finally). Per-connection rather than per-runner so one
///   connection's observer backlog can never delay another connection's notifications, and so
///   the worker's lifetime is exactly the connection's — the singleton runner needs no shutdown
///   hook of its own.
/// - Per-connection FIFO is preserved: a single reader drains the channel in receive order and
///   awaits observers sequentially (registration order per pack). The one shipped observer
///   (studio's live-map replay + cache refresh) is invoked per room off this stream, and a future
///   observer may well be order-sensitive — do not parallelize this without a contract change;
///   <c>RuntimeWebSocketLoopRunnerObserverDispatchTests</c> pins the guarantee.
/// - Backpressure is DROP-OLDEST, counted and logged, never block-the-receive-loop: the hook's
///   contract is advisory (persistence has already completed before observers ever run), and the
///   shipped observer ignores the pack bytes entirely — it re-reads the full persisted store per
///   notification, idempotently (see StudioInboundPackObserver / ServiceContracts' "safe to re-run
///   at any time"). Under drop-oldest the NEWEST notification always survives, so a drop costs at
///   most temporary cache staleness that the surviving notification (or the next pack) heals.
///   A WAIT policy was rejected because it re-couples receive-loop latency to observer latency —
///   exactly the defect this slice removes — merely with a deeper buffer in front of it.
/// - Each observer call gets its own timeout (see
///   <see cref="RuntimeInboundPackObserverDispatchOptions.ObserverTimeout"/>). The call is made
///   via <c>Task.Run(...).WaitAsync(token)</c> so even an observer that BLOCKS synchronously and
///   ignores its token cannot wedge the worker — the worker abandons the call at the deadline
///   (the abandoned call keeps its thread until it returns; that leak is bounded per notification
///   and only ever paid for a genuinely misbehaving observer) and moves on.
/// - Worker faults never surface anywhere that matters: per-observer try/catch (same isolation
///   the inline path had), plus a belt-and-suspenders catch around the whole loop so a bug here
///   can never produce an unobserved task exception, let alone touch frame processing or relay.
/// </summary>
internal sealed class RuntimeInboundPackObserverDispatcher : IAsyncDisposable
{
    // Deliberately the same meter name RuntimeWebSocketLoopRunner uses so these counters land in
    // the runtime-WS instrument family listeners already subscribe to.
    private static readonly Meter RuntimeWsMeter = new("NodalMerge.DotNetHost.RuntimeWs", "1.0.0");
    private static readonly Counter<long> ObserverDroppedCounter = RuntimeWsMeter.CreateCounter<long>(
        "runtime_ws_inbound_pack_observer_dropped_total"
    );
    private static readonly Counter<long> ObserverTimeoutCounter = RuntimeWsMeter.CreateCounter<long>(
        "runtime_ws_inbound_pack_observer_timeout_total"
    );

    private readonly IReadOnlyList<IInboundPackObserver> _observers;
    private readonly RuntimeInboundPackObserverDispatchOptions _options;
    private readonly ILogger _logger;
    private readonly Channel<(string RoomId, string NodesB64)> _channel;
    private readonly CancellationTokenSource _shutdownCts = new();
    private readonly Task _worker;

    public RuntimeInboundPackObserverDispatcher(
        IReadOnlyList<IInboundPackObserver> observers,
        RuntimeInboundPackObserverDispatchOptions options,
        ILogger logger
    )
    {
        _observers = observers;
        _options = options;
        _logger = logger;
        _channel = Channel.CreateBounded<(string RoomId, string NodesB64)>(
            new BoundedChannelOptions(Math.Max(1, options.QueueCapacity))
            {
                FullMode = BoundedChannelFullMode.DropOldest,
                SingleReader = true,
                SingleWriter = true
            },
            dropped =>
            {
                ObserverDroppedCounter.Add(1, KeyValuePair.Create<string, object?>("room", dropped.RoomId));
                _logger.LogWarning(
                    "runtime ws inbound pack observer queue full — dropped oldest queued notification room={Room} capacity={Capacity} (observers are lagging; the newest notification is retained, so caches heal on the surviving one)",
                    dropped.RoomId,
                    options.QueueCapacity
                );
            }
        );
        _worker = Task.Run(RunWorkerAsync);
    }

    /// <summary>
    /// Enqueue a notification from the receive loop. Never blocks, never throws: on a full queue
    /// the channel's drop-oldest callback fires instead; after shutdown the write is a no-op.
    /// </summary>
    public void Post(string roomId, string nodesB64)
    {
        _channel.Writer.TryWrite((roomId, nodesB64));
    }

    private async Task RunWorkerAsync()
    {
        try
        {
            await foreach (var notification in _channel.Reader.ReadAllAsync(_shutdownCts.Token))
            {
                await NotifyAllAsync(notification.RoomId, notification.NodesB64);
            }
        }
        catch (OperationCanceledException) when (_shutdownCts.IsCancellationRequested)
        {
            // Hard-cancelled teardown (drain grace expired) — anything still queued is dropped.
        }
        catch (Exception ex)
        {
            // Should be unreachable (NotifyAllAsync isolates every observer call) — kept so a bug
            // here can never become an unobserved task exception.
            _logger.LogError(ex, "runtime ws inbound pack observer worker faulted unexpectedly");
        }
    }

    private async Task NotifyAllAsync(string roomId, string nodesB64)
    {
        foreach (var observer in _observers)
        {
            // Own timeout, independent of connection teardown: linked to dispatcher shutdown (so a
            // hard-cancelled teardown reaches in-flight observers) but never to the connection's
            // receive-loop token.
            using var callCts = CancellationTokenSource.CreateLinkedTokenSource(_shutdownCts.Token);
            callCts.CancelAfter(_options.ObserverTimeout);
            try
            {
                // Task.Run + WaitAsync: the timeout holds even against an observer that blocks
                // synchronously or ignores its token — the worker abandons the call and moves on.
                await Task.Run(
                    () => observer.OnInboundPackAppliedAsync(roomId, nodesB64, callCts.Token).AsTask(),
                    CancellationToken.None
                ).WaitAsync(callCts.Token);
            }
            catch (OperationCanceledException) when (callCts.IsCancellationRequested && !_shutdownCts.IsCancellationRequested)
            {
                ObserverTimeoutCounter.Add(1, KeyValuePair.Create<string, object?>("room", roomId));
                _logger.LogWarning(
                    "runtime ws inbound pack observer timed out after {Timeout} room={Room} observer={Observer}",
                    _options.ObserverTimeout,
                    roomId,
                    observer.GetType().Name
                );
            }
            catch (OperationCanceledException) when (_shutdownCts.IsCancellationRequested)
            {
                return;
            }
            catch (Exception ex)
            {
                // Same per-observer isolation the old inline path had.
                _logger.LogWarning(
                    ex,
                    "runtime ws inbound pack observer failed room={Room} observer={Observer}",
                    roomId,
                    observer.GetType().Name
                );
            }
        }
    }

    /// <summary>
    /// Connection teardown: stop accepting, let queued notifications drain within
    /// <see cref="RuntimeInboundPackObserverDispatchOptions.DrainGrace"/>, then hard-cancel.
    /// Never throws.
    /// </summary>
    public async ValueTask DisposeAsync()
    {
        _channel.Writer.TryComplete();
        var drained = false;
        try
        {
            await _worker.WaitAsync(_options.DrainGrace);
            drained = true;
        }
        catch (TimeoutException)
        {
            _shutdownCts.Cancel();
            try
            {
                // Every await in the worker observes _shutdownCts, so this bound is generous;
                // it exists only so teardown can never hang behind a bug here.
                await _worker.WaitAsync(TimeSpan.FromSeconds(2));
                drained = true;
            }
            catch (TimeoutException)
            {
                _logger.LogWarning("runtime ws inbound pack observer worker did not stop within the teardown bound — abandoned");
            }
            catch (Exception ex)
            {
                _logger.LogWarning(ex, "runtime ws inbound pack observer worker teardown failed");
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime ws inbound pack observer worker teardown failed");
        }

        if (drained)
        {
            _shutdownCts.Dispose();
        }
        // else: leave the CTS for the abandoned worker; the finalizer reclaims it.
    }
}
