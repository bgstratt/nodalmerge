using ActiveSync.DotNetHost.Ffi;
using ActiveSync.DotNetHost.Runtime;
using System.Net.WebSockets;
using System.Text;

namespace ActiveSync.DotNetHost.Tests;

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