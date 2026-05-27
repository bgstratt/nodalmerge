using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using Microsoft.Extensions.Logging.Abstractions;
using System.Diagnostics.Metrics;
using System.Net.WebSockets;
using System.Text;

namespace NodalMerge.DotNetHost.Tests;

public class RuntimeWebSocketLoopRunnerTests
{
    [Fact]
    public async Task Binary_frame_emits_error_and_connection_stays_open_until_client_close()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Binary([0xAA]),
            FakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"msg\":\"text messages required\"", socket.SentTextMessages[0]);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Close_session_success_closes_with_session_closed_reason()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Success("[]"));
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Text("{\"type\":\"close-session\"}")
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("session closed", socket.CloseDescription);
    }

    [Fact]
    public async Task Fragmented_close_session_success_closes_with_session_closed_reason()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Success("[]"));
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.TextFragment("{\"type\":\"close-", endOfMessage: false),
            FakeReceiveFrame.TextFragment("session\"}", endOfMessage: true)
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("session closed", socket.CloseDescription);
    }

    [Fact]
    public async Task Close_session_bridge_failure_emits_status_error_and_waits_for_client_close()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Failure(AsStatus.Protocol));
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Text("{\"type\":\"close-session\"}"),
            FakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"status\":\"Protocol\"", socket.SentTextMessages[0]);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Fragmented_close_session_bridge_failure_error_send_failure_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Failure(AsStatus.Protocol));
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [
                FakeReceiveFrame.TextFragment("{\"type\":\"close-", endOfMessage: false),
                FakeReceiveFrame.TextFragment("session\"}", endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Oversized_message_emits_error_and_waits_for_client_close()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var chunkA = new string('x', 32_000);
        var chunkB = new string('y', 32_000);
        var chunkC = new string('z', 1_537);
        var socket = new FakeWebSocket([
            FakeReceiveFrame.TextFragment(chunkA, endOfMessage: false),
            FakeReceiveFrame.TextFragment(chunkB, endOfMessage: false),
            FakeReceiveFrame.TextFragment(chunkC, endOfMessage: true),
            FakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"msg\":\"message too large\"", socket.SentTextMessages[0]);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Fragmented_oversized_message_error_send_failure_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var chunkA = new string('x', 32_000);
        var chunkB = new string('y', 32_000);
        var chunkC = new string('z', 1_537);
        var socket = new FakeWebSocket(
            [
                FakeReceiveFrame.TextFragment(chunkA, endOfMessage: false),
                FakeReceiveFrame.TextFragment(chunkB, endOfMessage: false),
                FakeReceiveFrame.TextFragment(chunkC, endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Outbound_send_failure_is_handled_without_throwing()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Success("[\"NoopAck\"]"));
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [FakeReceiveFrame.Text("{\"type\":\"noop\"}")],
            throwOnSend: true
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Fragmented_noop_outbound_send_failure_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Success("[\"NoopAck\"]"));
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [
                FakeReceiveFrame.TextFragment("{\"type\":\"no", endOfMessage: false),
                FakeReceiveFrame.TextFragment("op\"}", endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Outbound_message_then_peer_close_frame_completes_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Success("[\"NoopAck\"]"));
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Text("{\"type\":\"noop\"}"),
            FakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"type\":\"noop-ack\"", socket.SentTextMessages[0]);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Close_failure_is_handled_without_throwing()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Success("[]"));
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [FakeReceiveFrame.Text("{\"type\":\"close-session\"}")],
            throwOnClose: true
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Cancellation_during_receive_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket([FakeReceiveFrame.Text("{\"type\":\"noop\"}")]);
        using var cts = new CancellationTokenSource();
        cts.Cancel();

        await runner.RunAsync(socket, frameProcessor, state, cts.Token);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Receive_failure_is_handled_without_throwing()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [FakeReceiveFrame.Text("{\"type\":\"noop\"}")],
            receiveFailure: ReceiveFailureMode.WebSocketException
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Partial_fragment_then_receive_failure_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [FakeReceiveFrame.TextFragment("{\"type\":\"noop\"", endOfMessage: false)],
            receiveFailure: ReceiveFailureMode.WebSocketException,
            receiveFailureOnCall: 2
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Receive_disposed_failure_is_handled_without_throwing()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [FakeReceiveFrame.Text("{\"type\":\"noop\"}")],
            receiveFailure: ReceiveFailureMode.ObjectDisposedException
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Partial_fragment_then_receive_disposed_failure_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [FakeReceiveFrame.TextFragment("{\"type\":\"noop\"", endOfMessage: false)],
            receiveFailure: ReceiveFailureMode.ObjectDisposedException,
            receiveFailureOnCall: 2
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Partial_fragment_then_close_frame_closes_without_emitting_messages()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.TextFragment("{\"type\":\"noop\"", endOfMessage: false),
            FakeReceiveFrame.Close(WebSocketCloseStatus.EndpointUnavailable, "peer closing")
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Direct_close_frame_with_non_normal_status_closes_without_emitting_messages()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Close(WebSocketCloseStatus.PolicyViolation, "peer policy close")
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Direct_close_frame_with_non_normal_status_and_null_reason_closes_without_emitting_messages()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Close(WebSocketCloseStatus.InternalServerError, null)
        ]);

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Binary_frame_error_send_failure_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var socket = new FakeWebSocket(
            [FakeReceiveFrame.Binary([0xAA])],
            throwOnSend: true
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Oversized_error_send_failure_exits_cleanly()
    {
        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor();
        var state = InitializedState();
        var chunkA = new string('x', 32_000);
        var chunkB = new string('y', 32_000);
        var chunkC = new string('z', 1_537);
        var socket = new FakeWebSocket(
            [
                FakeReceiveFrame.TextFragment(chunkA, endOfMessage: false),
                FakeReceiveFrame.TextFragment(chunkB, endOfMessage: false),
                FakeReceiveFrame.TextFragment(chunkC, endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.Empty(socket.SentTextMessages);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Runtime_ws_metrics_emit_connection_and_inbound_counts()
    {
        using var metrics = new WsMeterCapture("NodalMerge.DotNetHost.RuntimeWs");

        var runner = new RuntimeWebSocketLoopRunner();
        var frameProcessor = CreateFrameProcessor(
            FfiJsonBridgeResult.Success("[]"),
            FfiJsonBridgeResult.Success("[\"NoopAck\"]")
        );
        var state = InitializedState();
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Text("{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}"),
            FakeReceiveFrame.Text("{\"type\":\"noop\"}"),
            FakeReceiveFrame.Close()
        ]);

        var beforeOpened = metrics.GetTotal("runtime_ws_connections_opened_total");
        var beforeClosed = metrics.GetTotal("runtime_ws_connections_closed_total");
        var beforeInbound = metrics.GetTotal("runtime_ws_inbound_messages_total");

        await runner.RunAsync(socket, frameProcessor, state);

        Assert.True(metrics.GetTotal("runtime_ws_connections_opened_total") >= beforeOpened + 1);
        Assert.True(metrics.GetTotal("runtime_ws_connections_closed_total") >= beforeClosed + 1);
        Assert.True(metrics.GetTotal("runtime_ws_inbound_messages_total") >= beforeInbound + 2);
    }

    [Fact]
    public async Task Runtime_ws_metrics_emit_trace_and_pack_relay_correlation_counts()
    {
        using var metrics = new WsMeterCapture("NodalMerge.DotNetHost.RuntimeWs");

        var roomBroker = new RuntimeRoomBroker(NullLogger<RuntimeRoomBroker>.Instance);
        var senderRunner = new RuntimeWebSocketLoopRunner();
        var senderState = new RuntimeConnectionState(3)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        var senderSocket = new FakeWebSocket([
            FakeReceiveFrame.Text("{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"trace_id\":\"trace-abc\",\"frontier\":[]}"),
            FakeReceiveFrame.Text("{\"type\":\"pack\",\"nodes\":\"bm9kZXM=\",\"trace_id\":\"trace-abc\"}"),
            FakeReceiveFrame.Close()
        ]);

        var beforePackRelay = metrics.GetTotal("runtime_ws_pack_relay_total");
        var beforeInboundTrace = metrics.GetTotalByTrace("runtime_ws_inbound_messages_total", "trace-abc");

        await senderRunner.RunAsync(
            senderSocket,
            CreateFrameProcessor(
                FfiJsonBridgeResult.Success("[]"),
                FfiJsonBridgeResult.Success("[]")
            ),
            senderState,
            roomBroker,
            tokenValidationService: null,
            dagPersistenceService: null,
            CancellationToken.None
        );

        Assert.True(metrics.GetTotal("runtime_ws_pack_relay_total") >= beforePackRelay + 1);
        Assert.True(metrics.GetTotalByTrace("runtime_ws_inbound_messages_total", "trace-abc") >= beforeInboundTrace + 2);
    }

    [Fact]
    public async Task Simulated_reconnect_storm_emits_actionable_room_metrics()
    {
        using var metrics = new WsMeterCapture("NodalMerge.DotNetHost.RuntimeWs");

        const string room = "room-storm";
        const int sessionCount = 12;

        var beforeClosedByRoom = metrics.GetTotalByTag("runtime_ws_connections_closed_total", "room", room);
        var beforeInboundByRoom = metrics.GetTotalByTag("runtime_ws_inbound_messages_total", "room", room);

        for (var i = 0; i < sessionCount; i++)
        {
            var runner = new RuntimeWebSocketLoopRunner();
            var state = new RuntimeConnectionState((ulong)(1000 + i))
            {
                IsInitialized = true,
                RoomId = room,
                PeerPubkeyHex = $"peer-{i}"
            };

            var socket = new FakeWebSocket([
                FakeReceiveFrame.Text($"{{\"type\":\"hello\",\"room\":\"{room}\",\"pubkey\":\"peer-{i}\",\"trace_id\":\"storm-{i}\",\"frontier\":[]}}"),
                FakeReceiveFrame.Close()
            ]);

            await runner.RunAsync(
                socket,
                CreateFrameProcessor(FfiJsonBridgeResult.Success("[]")),
                state,
                cancellationToken: CancellationToken.None
            );
        }

        Assert.True(metrics.GetTotalByTag("runtime_ws_connections_closed_total", "room", room) >= beforeClosedByRoom + sessionCount);
        Assert.True(metrics.GetTotalByTag("runtime_ws_inbound_messages_total", "room", room) >= beforeInboundByRoom + sessionCount);
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

internal sealed record FakeReceiveFrame(
    WebSocketMessageType MessageType,
    byte[] Payload,
    bool EndOfMessage,
    WebSocketCloseStatus? CloseStatus = null,
    string? CloseStatusDescription = null
)
{
    public static FakeReceiveFrame Text(string text)
    {
        return new(WebSocketMessageType.Text, Encoding.UTF8.GetBytes(text), true);
    }

    public static FakeReceiveFrame TextFragment(string text, bool endOfMessage)
    {
        return new(WebSocketMessageType.Text, Encoding.UTF8.GetBytes(text), endOfMessage);
    }

    public static FakeReceiveFrame Binary(byte[] bytes)
    {
        return new(WebSocketMessageType.Binary, bytes, true);
    }

    public static FakeReceiveFrame Close(
        WebSocketCloseStatus closeStatus = WebSocketCloseStatus.NormalClosure,
        string? closeDescription = null
    )
    {
        return new(WebSocketMessageType.Close, [], true, closeStatus, closeDescription);
    }
}

internal sealed class FakeWebSocket : WebSocket
{
    private readonly Queue<FakeReceiveFrame> _receiveFrames;
    private readonly bool _throwOnSend;
    private readonly bool _throwOnClose;
    private readonly ReceiveFailureMode _receiveFailure;
    private readonly int _receiveFailureOnCall;
    private int _receiveCalls;
    private WebSocketCloseStatus? _closeStatus;
    private WebSocketState _state = WebSocketState.Open;

    public FakeWebSocket(
        IEnumerable<FakeReceiveFrame> receiveFrames,
        bool throwOnSend = false,
        bool throwOnClose = false,
        ReceiveFailureMode receiveFailure = ReceiveFailureMode.None,
        int receiveFailureOnCall = 1
    )
    {
        _receiveFrames = new Queue<FakeReceiveFrame>(receiveFrames);
        _throwOnSend = throwOnSend;
        _throwOnClose = throwOnClose;
        _receiveFailure = receiveFailure;
        _receiveFailureOnCall = receiveFailureOnCall;
    }

    public List<string> SentTextMessages { get; } = [];
    public override WebSocketCloseStatus? CloseStatus => _closeStatus;
    public override string? CloseStatusDescription { get; }
    public string? CloseDescription { get; private set; }
    public override WebSocketState State => _state;
    public override string? SubProtocol => null;

    public override void Abort()
    {
        _state = WebSocketState.Aborted;
    }

    public override Task CloseAsync(WebSocketCloseStatus closeStatus, string? statusDescription, CancellationToken cancellationToken)
    {
        if (_throwOnClose)
        {
            throw new WebSocketException(WebSocketError.ConnectionClosedPrematurely);
        }

        _closeStatus = closeStatus;
        CloseDescription = statusDescription;
        _state = WebSocketState.Closed;
        return Task.CompletedTask;
    }

    public override Task CloseOutputAsync(WebSocketCloseStatus closeStatus, string? statusDescription, CancellationToken cancellationToken)
    {
        _closeStatus = closeStatus;
        CloseDescription = statusDescription;
        _state = WebSocketState.CloseSent;
        return Task.CompletedTask;
    }

    public override void Dispose()
    {
        _state = WebSocketState.Closed;
    }

    public override Task<WebSocketReceiveResult> ReceiveAsync(ArraySegment<byte> buffer, CancellationToken cancellationToken)
    {
        _receiveCalls++;

        if (cancellationToken.IsCancellationRequested)
        {
            throw new OperationCanceledException(cancellationToken);
        }

        if (_receiveFailure == ReceiveFailureMode.WebSocketException && _receiveCalls == _receiveFailureOnCall)
        {
            throw new WebSocketException(WebSocketError.ConnectionClosedPrematurely);
        }

        if (_receiveFailure == ReceiveFailureMode.ObjectDisposedException && _receiveCalls == _receiveFailureOnCall)
        {
            throw new ObjectDisposedException(nameof(FakeWebSocket));
        }

        if (_receiveFrames.Count == 0)
        {
            return Task.FromResult(new WebSocketReceiveResult(0, WebSocketMessageType.Close, true));
        }

        var frame = _receiveFrames.Dequeue();
        if (frame.MessageType == WebSocketMessageType.Close)
        {
            _state = WebSocketState.CloseReceived;
        }
        var count = Math.Min(frame.Payload.Length, buffer.Count);
        if (count > 0)
        {
            frame.Payload.AsSpan(0, count).CopyTo(buffer.AsSpan(0, count));
        }

        var receiveResult = frame.MessageType == WebSocketMessageType.Close
            ? new WebSocketReceiveResult(count, frame.MessageType, frame.EndOfMessage, frame.CloseStatus, frame.CloseStatusDescription)
            : new WebSocketReceiveResult(count, frame.MessageType, frame.EndOfMessage);

        return Task.FromResult(receiveResult);
    }

    public override Task SendAsync(ArraySegment<byte> buffer, WebSocketMessageType messageType, bool endOfMessage, CancellationToken cancellationToken)
    {
        if (_throwOnSend)
        {
            throw new WebSocketException(WebSocketError.ConnectionClosedPrematurely);
        }

        if (messageType == WebSocketMessageType.Text)
        {
            SentTextMessages.Add(Encoding.UTF8.GetString(buffer.Array!, buffer.Offset, buffer.Count));
        }

        return Task.CompletedTask;
    }
}

internal enum ReceiveFailureMode
{
    None,
    WebSocketException,
    ObjectDisposedException
}

internal sealed class WsMeterCapture : IDisposable
{
    private readonly MeterListener _listener;
    private readonly Dictionary<string, long> _totals = new(StringComparer.Ordinal);
    private readonly Dictionary<string, long> _totalsByInstrumentAndTrace = new(StringComparer.Ordinal);
    private readonly Dictionary<string, long> _totalsByInstrumentAndTag = new(StringComparer.Ordinal);

    public WsMeterCapture(string meterName)
    {
        _listener = new MeterListener
        {
            InstrumentPublished = (instrument, listener) =>
            {
                if (string.Equals(instrument.Meter.Name, meterName, StringComparison.Ordinal))
                {
                    listener.EnableMeasurementEvents(instrument);
                }
            }
        };

        _listener.SetMeasurementEventCallback<long>((instrument, measurement, tags, _) =>
        {
            if (_totals.TryGetValue(instrument.Name, out var current))
            {
                _totals[instrument.Name] = current + measurement;
            }
            else
            {
                _totals[instrument.Name] = measurement;
            }

            string? trace = null;
            foreach (var tag in tags)
            {
                if (string.Equals(tag.Key, "trace", StringComparison.Ordinal)
                    && tag.Value is string traceValue
                    && !string.IsNullOrWhiteSpace(traceValue))
                {
                    trace = traceValue;
                    break;
                }
            }

            if (!string.IsNullOrWhiteSpace(trace))
            {
                var key = $"{instrument.Name}|{trace}";
                if (_totalsByInstrumentAndTrace.TryGetValue(key, out var byTraceCurrent))
                {
                    _totalsByInstrumentAndTrace[key] = byTraceCurrent + measurement;
                }
                else
                {
                    _totalsByInstrumentAndTrace[key] = measurement;
                }
            }

            foreach (var tag in tags)
            {
                if (string.IsNullOrWhiteSpace(tag.Key))
                {
                    continue;
                }

                var value = tag.Value?.ToString();
                if (string.IsNullOrWhiteSpace(value))
                {
                    continue;
                }

                var tagKey = $"{instrument.Name}|{tag.Key}|{value}";
                if (_totalsByInstrumentAndTag.TryGetValue(tagKey, out var byTagCurrent))
                {
                    _totalsByInstrumentAndTag[tagKey] = byTagCurrent + measurement;
                }
                else
                {
                    _totalsByInstrumentAndTag[tagKey] = measurement;
                }
            }
        });

        _listener.Start();
    }

    public long GetTotal(string name)
    {
        return _totals.TryGetValue(name, out var total) ? total : 0;
    }

    public long GetTotalByTrace(string instrumentName, string trace)
    {
        var key = $"{instrumentName}|{trace}";
        return _totalsByInstrumentAndTrace.TryGetValue(key, out var total) ? total : 0;
    }

    public long GetTotalByTag(string instrumentName, string tagKey, string tagValue)
    {
        var key = $"{instrumentName}|{tagKey}|{tagValue}";
        return _totalsByInstrumentAndTag.TryGetValue(key, out var total) ? total : 0;
    }

    public void Dispose()
    {
        _listener.Dispose();
    }
}