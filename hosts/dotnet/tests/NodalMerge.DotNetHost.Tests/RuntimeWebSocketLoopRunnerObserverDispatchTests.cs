using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using NodalMerge.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging.Abstractions;
using System.Diagnostics;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 6.5 (nodalmerge-studio/plans/blob-cas-remediation.md): the S7.1
/// <see cref="IInboundPackObserver"/> hook used to be awaited INLINE in
/// <see cref="RuntimeWebSocketLoopRunner"/>'s receive loop — the per-observer try/catch isolated
/// exceptions but not latency, so a slow/hung observer stalled every further frame on the
/// connection. These tests pin the off-loop dispatch contract:
///
/// - a slow observer no longer delays frame processing (starvation RED — pre-6.5 the frames were
///   serialized behind every observer call);
/// - a HUNG observer (never returns, ignores its token) neither stalls frames nor wedges
///   connection teardown, and is abandoned by the dispatcher's own per-call timeout (RED —
///   pre-6.5 the loop hung forever);
/// - observers see packs in per-connection receive order, sequentially (ordering PIN — this held
///   pre-6.5 too; pinned so a future "parallelize the observer plane" can't silently break
///   studio's order-sensitive live-map replay);
/// - a full queue drops the OLDEST notification and always retains the newest (backpressure
///   policy PIN — see RuntimeInboundPackObserverDispatcher's class comment for why drop-oldest).
///
/// The throw-isolation case lives in RuntimeWebSocketLoopRunnerTests
/// (Throwing_observer_does_not_break_loop_or_persistence) and was green before 6.5 — that one is
/// a pin of S7.1 behavior, not part of this slice's RED set.
/// </summary>
public class RuntimeWebSocketLoopRunnerObserverDispatchTests
{
    private const string PackFrameTemplate = "{{\"type\":\"pack\",\"room\":\"room-a\",\"nodes\":\"{0}\"}}";

    [Fact]
    public async Task Slow_observer_does_not_stall_subsequent_frame_processing()
    {
        // Four packs behind a 500ms-per-pack observer. Pre-6.5 the receive loop awaited the
        // observer between frames, so the LAST pack could not persist until ≥ 3 × 500ms after the
        // first. Post-6.5 all four persist while the observer is still chewing on pack #1 —
        // asserted with a ≥2x margin (< 750ms) against the pre-fix floor (≥ 1500ms).
        var observer = new DelayingInboundPackObserver(TimeSpan.FromMilliseconds(500));
        var runner = new RuntimeWebSocketLoopRunner(
            NullLogger<RuntimeWebSocketLoopRunner>.Instance,
            new IInboundPackObserver[] { observer }
        );
        var frameProcessor = CreateFrameProcessor(
            FfiJsonBridgeResult.Success("[]"),
            FfiJsonBridgeResult.Success("[]"),
            FfiJsonBridgeResult.Success("[]"),
            FfiJsonBridgeResult.Success("[]")
        );
        var state = InitializedState();
        var nodeStore = new TimestampingNodeStoreProvider();
        var dagPersistence = new RuntimeDagPersistenceService(
            nodeStore,
            new FakeRuntimeCommandBridge(),
            NullLogger<RuntimeDagPersistenceService>.Instance
        );
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Text(string.Format(PackFrameTemplate, "cDE=")),
            FakeReceiveFrame.Text(string.Format(PackFrameTemplate, "cDI=")),
            FakeReceiveFrame.Text(string.Format(PackFrameTemplate, "cDM=")),
            FakeReceiveFrame.Text(string.Format(PackFrameTemplate, "cDQ=")),
            FakeReceiveFrame.Close()
        ]);

        var clock = Stopwatch.StartNew();
        await runner.RunAsync(
            socket,
            frameProcessor,
            state,
            roomBroker: null,
            tokenValidationService: null,
            dagPersistenceService: dagPersistence,
            peerLocalPersistenceService: null,
            CancellationToken.None
        ).WaitAsync(TimeSpan.FromSeconds(20));
        clock.Stop();

        Assert.Equal(4, nodeStore.PersistTimestamps.Count);
        var lastPersist = nodeStore.PersistTimestamps[^1];
        Assert.True(
            lastPersist < TimeSpan.FromMilliseconds(750),
            $"last inbound pack persisted at {lastPersist.TotalMilliseconds:F0}ms — frame processing is " +
            "still serialized behind the slow observer (pre-6.5 floor is ≥1500ms)"
        );

        // The observer still saw every pack (RunAsync's teardown drains the queue within the
        // grace) — off-loop, not dropped.
        Assert.Equal(4, observer.Calls.Count);
    }

    [Fact]
    public async Task Hung_observer_frames_keep_processing_and_teardown_is_bounded()
    {
        // An observer that NEVER returns and ignores its cancellation token. Pre-6.5 the receive
        // loop awaited it inline: the noop frame after the pack was never processed and RunAsync
        // never returned. Post-6.5 the frames keep flowing, the dispatcher's own per-call timeout
        // abandons the call, and teardown completes within the drain bound.
        var observer = new HangingInboundPackObserver();
        var runner = new RuntimeWebSocketLoopRunner(
            NullLogger<RuntimeWebSocketLoopRunner>.Instance,
            new IInboundPackObserver[] { observer },
            new RuntimeInboundPackObserverDispatchOptions
            {
                ObserverTimeout = TimeSpan.FromMilliseconds(250),
                DrainGrace = TimeSpan.FromMilliseconds(500)
            }
        );
        var frameProcessor = CreateFrameProcessor(
            FfiJsonBridgeResult.Success("[]"),
            FfiJsonBridgeResult.Success("[\"NoopAck\"]")
        );
        var state = InitializedState();
        var nodeStore = new TimestampingNodeStoreProvider();
        var dagPersistence = new RuntimeDagPersistenceService(
            nodeStore,
            new FakeRuntimeCommandBridge(),
            NullLogger<RuntimeDagPersistenceService>.Instance
        );
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Text(string.Format(PackFrameTemplate, "bm9kZXM=")),
            FakeReceiveFrame.Text("{\"type\":\"noop\"}"),
            FakeReceiveFrame.Close()
        ]);

        // Pre-6.5 this WaitAsync is what failed: RunAsync never completed behind the hung
        // observer. The bound is generous (10s) against a ≤ ~1s post-fix teardown.
        await runner.RunAsync(
            socket,
            frameProcessor,
            state,
            roomBroker: null,
            tokenValidationService: null,
            dagPersistenceService: dagPersistence,
            peerLocalPersistenceService: null,
            CancellationToken.None
        ).WaitAsync(TimeSpan.FromSeconds(10));

        // The frame AFTER the pack was processed and answered while the observer hung.
        Assert.Contains(socket.SentTextMessages, m => m.Contains("\"type\":\"noop-ack\""));
        Assert.Equal("client requested close", socket.CloseDescription);

        // The observer was started once and its own token was cancelled by the dispatcher's
        // per-call timeout — independent of the connection token, which was never cancelled.
        Assert.Equal(1, observer.InvocationCount);
        var cancelled = await observer.TokenCancelled.Task.WaitAsync(TimeSpan.FromSeconds(5));
        Assert.True(cancelled);
    }

    [Fact]
    public async Task Observers_see_packs_in_per_connection_receive_order()
    {
        // Ordering PIN: one FIFO worker per connection, observers awaited sequentially — packs
        // are observed in exactly the order the connection received them, even with a small
        // random-ish per-call delay that an unordered/parallel dispatch would scramble.
        var observer = new DelayingInboundPackObserver(TimeSpan.FromMilliseconds(20));
        var runner = new RuntimeWebSocketLoopRunner(
            NullLogger<RuntimeWebSocketLoopRunner>.Instance,
            new IInboundPackObserver[] { observer }
        );
        var frameProcessor = CreateFrameProcessor(
            Enumerable.Repeat(FfiJsonBridgeResult.Success("[]"), 6).ToArray()
        );
        var state = InitializedState();
        var dagPersistence = new RuntimeDagPersistenceService(
            new TimestampingNodeStoreProvider(),
            new FakeRuntimeCommandBridge(),
            NullLogger<RuntimeDagPersistenceService>.Instance
        );
        var expected = new[] { "cDE=", "cDI=", "cDM=", "cDQ=", "cDU=", "cDY=" };
        var frames = expected
            .Select(nodes => FakeReceiveFrame.Text(string.Format(PackFrameTemplate, nodes)))
            .Append(FakeReceiveFrame.Close())
            .ToArray();
        var socket = new FakeWebSocket(frames);

        await runner.RunAsync(
            socket,
            frameProcessor,
            state,
            roomBroker: null,
            tokenValidationService: null,
            dagPersistenceService: dagPersistence,
            peerLocalPersistenceService: null,
            CancellationToken.None
        ).WaitAsync(TimeSpan.FromSeconds(20));

        Assert.Equal(expected, observer.Calls.Select(call => call.NodesB64).ToArray());
        Assert.True(observer.MaxConcurrency <= 1, "observers must be invoked sequentially, never in parallel");
    }

    [Fact]
    public async Task Full_queue_drops_oldest_and_always_retains_newest()
    {
        // Backpressure PIN: capacity 2, an observer gated shut while five packs flood in. The
        // dispatcher must (a) never block the receive loop (RunAsync's frames all process
        // immediately), (b) drop from the OLD end, (c) always deliver the NEWEST notification once
        // the observer unblocks — the property studio's idempotent full-store refresh relies on.
        var observer = new GatedInboundPackObserver();
        var runner = new RuntimeWebSocketLoopRunner(
            NullLogger<RuntimeWebSocketLoopRunner>.Instance,
            new IInboundPackObserver[] { observer },
            new RuntimeInboundPackObserverDispatchOptions
            {
                QueueCapacity = 2,
                DrainGrace = TimeSpan.FromSeconds(10)
            }
        );
        var frameProcessor = CreateFrameProcessor(
            Enumerable.Repeat(FfiJsonBridgeResult.Success("[]"), 5).ToArray()
        );
        var state = InitializedState();
        var dagPersistence = new RuntimeDagPersistenceService(
            new TimestampingNodeStoreProvider(),
            new FakeRuntimeCommandBridge(),
            NullLogger<RuntimeDagPersistenceService>.Instance
        );
        var all = new[] { "cDE=", "cDI=", "cDM=", "cDQ=", "cDU=" };
        var frames = all
            .Select(nodes => FakeReceiveFrame.Text(string.Format(PackFrameTemplate, nodes)))
            .Append(FakeReceiveFrame.Close())
            .ToArray();
        var socket = new FakeWebSocket(frames);

        var runTask = runner.RunAsync(
            socket,
            frameProcessor,
            state,
            roomBroker: null,
            tokenValidationService: null,
            dagPersistenceService: dagPersistence,
            peerLocalPersistenceService: null,
            CancellationToken.None
        );

        // Wait until the worker has taken the first notification (and is now blocked in the
        // observer), then open the gate so the drained backlog can flow.
        await observer.FirstCallStarted.Task.WaitAsync(TimeSpan.FromSeconds(5));
        observer.Gate.SetResult();

        await runTask.WaitAsync(TimeSpan.FromSeconds(20));

        // First notification was in-flight before the flood; of the rest, the queue (capacity 2)
        // dropped from the old end — the LAST pack must always survive.
        var seen = observer.Calls.Select(call => call.NodesB64).ToArray();
        Assert.Equal("cDU=", seen[^1]);
        Assert.True(
            seen.Length < all.Length,
            "expected the capacity-2 queue to drop at least one notification under a gated observer"
        );
        // Receive order is preserved among the survivors.
        var positions = seen.Select(nodes => Array.IndexOf(all, nodes)).ToArray();
        Assert.Equal(positions.OrderBy(p => p).ToArray(), positions);
    }

    private static RuntimeFrameProcessor CreateFrameProcessor(params FfiJsonBridgeResult[] bridgeResults)
    {
        var bridge = new FakeRuntimeCommandBridge(bridgeResults);
        var mapper = new RuntimeProtocolMapper();
        var messageProcessor = new RuntimeMessageProcessor(bridge, mapper);
        return new RuntimeFrameProcessor(messageProcessor);
    }

    private static RuntimeConnectionState InitializedState()
    {
        return new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
    }
}

// Slice 6.5 test doubles.

/// <summary>Observer that takes a fixed wall-clock delay per pack and records call order.</summary>
internal sealed class DelayingInboundPackObserver(TimeSpan delay) : IInboundPackObserver
{
    private readonly object _lock = new();
    private int _inFlight;

    public List<(string RoomId, string NodesB64)> Calls { get; } = [];
    public int MaxConcurrency { get; private set; }

    public async ValueTask OnInboundPackAppliedAsync(string roomId, string nodesB64, CancellationToken cancellationToken = default)
    {
        lock (_lock)
        {
            _inFlight++;
            MaxConcurrency = Math.Max(MaxConcurrency, _inFlight);
        }

        try
        {
            await Task.Delay(delay, cancellationToken);
        }
        finally
        {
            lock (_lock)
            {
                Calls.Add((roomId, nodesB64));
                _inFlight--;
            }
        }
    }
}

/// <summary>
/// Observer that never returns: it ignores cancellation for its own completion but reports when
/// its token fires, so tests can prove the dispatcher's own timeout reached it.
/// </summary>
internal sealed class HangingInboundPackObserver : IInboundPackObserver
{
    private int _invocationCount;

    public int InvocationCount => Volatile.Read(ref _invocationCount);
    public TaskCompletionSource<bool> TokenCancelled { get; } =
        new(TaskCreationOptions.RunContinuationsAsynchronously);

    public async ValueTask OnInboundPackAppliedAsync(string roomId, string nodesB64, CancellationToken cancellationToken = default)
    {
        Interlocked.Increment(ref _invocationCount);
        cancellationToken.Register(() => TokenCancelled.TrySetResult(true));
        // Hang forever, deliberately NOT observing the token.
        await new TaskCompletionSource().Task;
    }
}

/// <summary>Observer whose first call blocks on a test-controlled gate (honoring its token).</summary>
internal sealed class GatedInboundPackObserver : IInboundPackObserver
{
    private readonly object _lock = new();
    private bool _first = true;

    public List<(string RoomId, string NodesB64)> Calls { get; } = [];
    public TaskCompletionSource FirstCallStarted { get; } =
        new(TaskCreationOptions.RunContinuationsAsynchronously);
    public TaskCompletionSource Gate { get; } =
        new(TaskCreationOptions.RunContinuationsAsynchronously);

    public async ValueTask OnInboundPackAppliedAsync(string roomId, string nodesB64, CancellationToken cancellationToken = default)
    {
        bool isFirst;
        lock (_lock)
        {
            isFirst = _first;
            _first = false;
        }

        if (isFirst)
        {
            FirstCallStarted.TrySetResult();
            await Gate.Task.WaitAsync(cancellationToken);
        }

        lock (_lock)
        {
            Calls.Add((roomId, nodesB64));
        }
    }
}

/// <summary>
/// In-memory node store that timestamps every PersistAcceptedNodesAsync against a shared
/// stopwatch started at construction — the seam the starvation test reads frame-processing
/// progress from (persistence happens inline on the receive loop, BEFORE observers).
/// </summary>
internal sealed class TimestampingNodeStoreProvider : INodeStoreProvider
{
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private readonly List<AcceptedNodeRecord> _nodes = [];
    private readonly object _lock = new();

    public List<TimeSpan> PersistTimestamps { get; } = [];

    public ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
    {
        lock (_lock)
        {
            return ValueTask.FromResult<NodeSnapshot?>(new NodeSnapshot(roomId, _nodes.ToArray()));
        }
    }

    public ValueTask<CompactionSnapshot?> LoadCompactionSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
    {
        return ValueTask.FromResult<CompactionSnapshot?>(null);
    }

    public ValueTask PersistAcceptedNodesAsync(string roomId, IReadOnlyList<AcceptedNodeRecord> nodes, CancellationToken cancellationToken = default)
    {
        lock (_lock)
        {
            PersistTimestamps.Add(_clock.Elapsed);
            _nodes.AddRange(nodes);
        }
        return ValueTask.CompletedTask;
    }

    public ValueTask DeleteAcceptedNodesAsync(string roomId, IReadOnlyList<string> nodeIdHexes, CancellationToken cancellationToken = default)
    {
        lock (_lock)
        {
            _nodes.RemoveAll(node => nodeIdHexes.Contains(node.NodeIdHex, StringComparer.Ordinal));
        }
        return ValueTask.CompletedTask;
    }

    public ValueTask PersistCompactionSnapshotAsync(string roomId, CompactionSnapshot snapshot, CancellationToken cancellationToken = default)
    {
        return ValueTask.CompletedTask;
    }
}
