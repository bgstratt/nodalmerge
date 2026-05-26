using NodalMerge.DotNetHost.Ffi;
using System.Diagnostics.Metrics;
using System.Net.WebSockets;
using System.Text;

namespace NodalMerge.DotNetHost.Tests;

public class FfiWebSocketLoopRunnerTests
{
    [Fact]
    public async Task Text_frame_emits_binary_required_error_and_waits_for_client_close()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.Text("not-binary"),
            FfiFakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"msg\":\"binary messages required\"", socket.SentTextMessages[0]);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Fragmented_binary_failure_emits_status_error_and_waits_for_client_close()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge(FfiBridgeResult.Failure(AsStatus.Protocol));
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.BinaryFragment([0xA1], endOfMessage: false),
            FfiFakeReceiveFrame.BinaryFragment([0xA2], endOfMessage: true),
            FfiFakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"status\":\"Protocol\"", socket.SentTextMessages[0]);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Policy_status_failure_emits_control_plane_deny_metric_with_fixed_labels()
    {
        using var metrics = new FfiControlPlaneDenyMeterCapture();
        var beforeDenied = metrics.GetTotalByTags(
            "dotnet-host",
            "ffi",
            "unknown",
            "reject.control_plane_forbidden"
        );

        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge(FfiBridgeResult.Failure(AsStatus.Policy));
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.Binary([0xA1]),
            FfiFakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"status\":\"Policy\"", socket.SentTextMessages[0]);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "ffi", "unknown", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
    }

    [Fact]
    public async Task Policy_status_failure_with_deny_metadata_emits_concrete_metric_labels()
    {
        using var metrics = new FfiControlPlaneDenyMeterCapture();
        var beforeDenied = metrics.GetTotalByTags(
            "dotnet-host",
            "set-policy",
            "policy.admin",
            "reject.control_plane_forbidden"
        );

        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge(
            FfiBridgeResult.Failure(
                AsStatus.Policy,
                new FfiDenyMetadata(
                    "reject.control_plane_forbidden",
                    "set-policy",
                    "policy.admin",
                    "reject.control_plane_forbidden: set-policy requires policy.admin"
                )
            )
        );
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.Binary([0xA1]),
            FfiFakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"status\":\"Policy\"", socket.SentTextMessages[0]);
        Assert.True(
            metrics.GetTotalByTags(
                "dotnet-host",
                "set-policy",
                "policy.admin",
                "reject.control_plane_forbidden"
            ) >= beforeDenied + 1
        );
    }

    [Fact]
    public async Task Fragmented_binary_failure_error_send_failure_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge(FfiBridgeResult.Failure(AsStatus.Protocol));
        var socket = new FfiFakeWebSocket(
            [
                FfiFakeReceiveFrame.BinaryFragment([0xA1], endOfMessage: false),
                FfiFakeReceiveFrame.BinaryFragment([0xA2], endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Oversized_binary_frame_emits_error_and_waits_for_client_close()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var chunkA = new byte[32_000];
        var chunkB = new byte[32_000];
        var chunkC = new byte[1_537];
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.BinaryFragment(chunkA, endOfMessage: false),
            FfiFakeReceiveFrame.BinaryFragment(chunkB, endOfMessage: false),
            FfiFakeReceiveFrame.BinaryFragment(chunkC, endOfMessage: true),
            FfiFakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Single(socket.SentTextMessages);
        Assert.Contains("\"msg\":\"message too large\"", socket.SentTextMessages[0]);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Fragmented_oversized_binary_error_send_failure_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var chunkA = new byte[32_000];
        var chunkB = new byte[32_000];
        var chunkC = new byte[1_537];
        var socket = new FfiFakeWebSocket(
            [
                FfiFakeReceiveFrame.BinaryFragment(chunkA, endOfMessage: false),
                FfiFakeReceiveFrame.BinaryFragment(chunkB, endOfMessage: false),
                FfiFakeReceiveFrame.BinaryFragment(chunkC, endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Outbound_send_failure_is_handled_without_throwing()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge(FfiBridgeResult.Success([0x01]));
        var socket = new FfiFakeWebSocket(
            [FfiFakeReceiveFrame.Binary([0xAA])],
            throwOnSend: true
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Fragmented_binary_outbound_send_failure_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge(FfiBridgeResult.Success([0x01]));
        var socket = new FfiFakeWebSocket(
            [
                FfiFakeReceiveFrame.BinaryFragment([0xA1], endOfMessage: false),
                FfiFakeReceiveFrame.BinaryFragment([0xA2], endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Outbound_binary_then_peer_close_frame_completes_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge(FfiBridgeResult.Success([0x01, 0x02]));
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.Binary([0xAA]),
            FfiFakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Single(socket.SentBinaryPayloads);
        Assert.Equal(new byte[] { 0x01, 0x02 }, socket.SentBinaryPayloads[0]);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Close_failure_is_handled_without_throwing()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket(
            [FfiFakeReceiveFrame.Close()],
            throwOnClose: true
        );

        await runner.RunAsync(socket, bridge);

        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Cancellation_during_receive_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket([FfiFakeReceiveFrame.Binary([0xAA])]);
        using var cts = new CancellationTokenSource();
        cts.Cancel();

        await runner.RunAsync(socket, bridge, cts.Token);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Receive_failure_is_handled_without_throwing()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket(
            [FfiFakeReceiveFrame.Binary([0xAA])],
            receiveFailure: FfiReceiveFailureMode.WebSocketException
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Partial_fragment_then_receive_failure_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket(
            [FfiFakeReceiveFrame.BinaryFragment([0xA1], endOfMessage: false)],
            receiveFailure: FfiReceiveFailureMode.WebSocketException,
            receiveFailureOnCall: 2
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Receive_disposed_failure_is_handled_without_throwing()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket(
            [FfiFakeReceiveFrame.Binary([0xAA])],
            receiveFailure: FfiReceiveFailureMode.ObjectDisposedException
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Partial_fragment_then_receive_disposed_failure_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket(
            [FfiFakeReceiveFrame.BinaryFragment([0xA1], endOfMessage: false)],
            receiveFailure: FfiReceiveFailureMode.ObjectDisposedException,
            receiveFailureOnCall: 2
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Partial_fragment_then_close_frame_closes_without_emitting_messages()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.BinaryFragment([0xAA, 0xBB], endOfMessage: false),
            FfiFakeReceiveFrame.Close(WebSocketCloseStatus.EndpointUnavailable, "peer closing")
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Direct_close_frame_with_non_normal_status_closes_without_emitting_messages()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.Close(WebSocketCloseStatus.PolicyViolation, "peer policy close")
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Direct_close_frame_with_non_normal_status_and_null_reason_closes_without_emitting_messages()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket([
            FfiFakeReceiveFrame.Close(WebSocketCloseStatus.InternalServerError, null)
        ]);

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Equal(WebSocketCloseStatus.NormalClosure, socket.CloseStatus);
        Assert.Equal("client requested close", socket.CloseDescription);
    }

    [Fact]
    public async Task Text_frame_error_send_failure_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var socket = new FfiFakeWebSocket(
            [FfiFakeReceiveFrame.Text("invalid")],
            throwOnSend: true
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }

    [Fact]
    public async Task Oversized_error_send_failure_exits_cleanly()
    {
        var runner = new FfiWebSocketLoopRunner();
        var bridge = new TestFfiBinaryBridge();
        var chunkA = new byte[32_000];
        var chunkB = new byte[32_000];
        var chunkC = new byte[1_537];
        var socket = new FfiFakeWebSocket(
            [
                FfiFakeReceiveFrame.BinaryFragment(chunkA, endOfMessage: false),
                FfiFakeReceiveFrame.BinaryFragment(chunkB, endOfMessage: false),
                FfiFakeReceiveFrame.BinaryFragment(chunkC, endOfMessage: true)
            ],
            throwOnSend: true
        );

        await runner.RunAsync(socket, bridge);

        Assert.Empty(socket.SentTextMessages);
        Assert.Empty(socket.SentBinaryPayloads);
        Assert.Null(socket.CloseDescription);
    }
}

internal sealed class TestFfiBinaryBridge : IFfiBinaryBridge
{
    private readonly Queue<FfiBridgeResult> _results;

    public TestFfiBinaryBridge(params FfiBridgeResult[] results)
    {
        _results = new Queue<FfiBridgeResult>(results);
    }

    public FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload)
    {
        if (_results.Count == 0)
        {
            return FfiBridgeResult.Success([]);
        }

        return _results.Dequeue();
    }
}

internal sealed record FfiFakeReceiveFrame(
    WebSocketMessageType MessageType,
    byte[] Payload,
    bool EndOfMessage,
    WebSocketCloseStatus? CloseStatus = null,
    string? CloseStatusDescription = null
)
{
    public static FfiFakeReceiveFrame Text(string text)
    {
        return new(WebSocketMessageType.Text, Encoding.UTF8.GetBytes(text), true);
    }

    public static FfiFakeReceiveFrame Binary(byte[] payload)
    {
        return new(WebSocketMessageType.Binary, payload, true);
    }

    public static FfiFakeReceiveFrame BinaryFragment(byte[] payload, bool endOfMessage)
    {
        return new(WebSocketMessageType.Binary, payload, endOfMessage);
    }

    public static FfiFakeReceiveFrame Close(
        WebSocketCloseStatus closeStatus = WebSocketCloseStatus.NormalClosure,
        string? closeDescription = null
    )
    {
        return new(WebSocketMessageType.Close, [], true, closeStatus, closeDescription);
    }
}

internal sealed class FfiFakeWebSocket : WebSocket
{
    private readonly Queue<FfiFakeReceiveFrame> _receiveFrames;
    private readonly bool _throwOnSend;
    private readonly bool _throwOnClose;
    private readonly FfiReceiveFailureMode _receiveFailure;
    private readonly int _receiveFailureOnCall;
    private int _receiveCalls;
    private WebSocketCloseStatus? _closeStatus;
    private WebSocketState _state = WebSocketState.Open;

    public FfiFakeWebSocket(
        IEnumerable<FfiFakeReceiveFrame> receiveFrames,
        bool throwOnSend = false,
        bool throwOnClose = false,
        FfiReceiveFailureMode receiveFailure = FfiReceiveFailureMode.None,
        int receiveFailureOnCall = 1
    )
    {
        _receiveFrames = new Queue<FfiFakeReceiveFrame>(receiveFrames);
        _throwOnSend = throwOnSend;
        _throwOnClose = throwOnClose;
        _receiveFailure = receiveFailure;
        _receiveFailureOnCall = receiveFailureOnCall;
    }

    public List<string> SentTextMessages { get; } = [];
    public List<byte[]> SentBinaryPayloads { get; } = [];
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

        if (_receiveFailure == FfiReceiveFailureMode.WebSocketException && _receiveCalls == _receiveFailureOnCall)
        {
            throw new WebSocketException(WebSocketError.ConnectionClosedPrematurely);
        }

        if (_receiveFailure == FfiReceiveFailureMode.ObjectDisposedException && _receiveCalls == _receiveFailureOnCall)
        {
            throw new ObjectDisposedException(nameof(FfiFakeWebSocket));
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
        else if (messageType == WebSocketMessageType.Binary)
        {
            SentBinaryPayloads.Add(buffer.ToArray());
        }

        return Task.CompletedTask;
    }
}

internal enum FfiReceiveFailureMode
{
    None,
    WebSocketException,
    ObjectDisposedException
}

internal sealed class FfiControlPlaneDenyMeterCapture : IDisposable
{
    private readonly MeterListener _listener;
    private readonly Dictionary<string, long> _totalsByTags = new(StringComparer.Ordinal);

    public FfiControlPlaneDenyMeterCapture()
    {
        _listener = new MeterListener
        {
            InstrumentPublished = (instrument, listener) =>
            {
                if (string.Equals(instrument.Meter.Name, "NodalMerge.DotNetHost.RuntimeControlPlane", StringComparison.Ordinal)
                    && string.Equals(instrument.Name, "runtime_control_plane_denied_total", StringComparison.Ordinal))
                {
                    listener.EnableMeasurementEvents(instrument);
                }
            }
        };

        _listener.SetMeasurementEventCallback<long>((_, measurement, tags, _) =>
        {
            var host = "<unknown>";
            var command = "<unknown>";
            var requiredCapability = "<unknown>";
            var reasonClass = "<unknown>";

            foreach (var tag in tags)
            {
                if (string.Equals(tag.Key, "host", StringComparison.Ordinal) && tag.Value is string hostValue)
                {
                    host = hostValue;
                }
                else if (string.Equals(tag.Key, "command", StringComparison.Ordinal) && tag.Value is string commandValue)
                {
                    command = commandValue;
                }
                else if (string.Equals(tag.Key, "required_capability", StringComparison.Ordinal) && tag.Value is string capabilityValue)
                {
                    requiredCapability = capabilityValue;
                }
                else if (string.Equals(tag.Key, "reason_class", StringComparison.Ordinal) && tag.Value is string reasonValue)
                {
                    reasonClass = reasonValue;
                }
            }

            var key = BuildKey(host, command, requiredCapability, reasonClass);
            if (_totalsByTags.TryGetValue(key, out var current))
            {
                _totalsByTags[key] = current + measurement;
            }
            else
            {
                _totalsByTags[key] = measurement;
            }
        });

        _listener.Start();
    }

    public long GetTotalByTags(string host, string command, string requiredCapability, string reasonClass)
    {
        var key = BuildKey(host, command, requiredCapability, reasonClass);
        return _totalsByTags.TryGetValue(key, out var total) ? total : 0;
    }

    public void Dispose()
    {
        _listener.Dispose();
    }

    private static string BuildKey(string host, string command, string requiredCapability, string reasonClass)
    {
        return $"{host}|{command}|{requiredCapability}|{reasonClass}";
    }
}
