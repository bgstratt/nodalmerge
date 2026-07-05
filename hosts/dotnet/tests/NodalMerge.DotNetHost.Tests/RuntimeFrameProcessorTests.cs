using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using System.Net.WebSockets;
using System.Text;

namespace NodalMerge.DotNetHost.Tests;

public class RuntimeFrameProcessorTests
{
    [Fact]
    public void Binary_frame_returns_text_required_error()
    {
        var frameProcessor = CreateProcessor();
        var state = InitializedState();

        var result = frameProcessor.ProcessFrame(
            WebSocketMessageType.Binary,
            [0x01, 0x02],
            state
        );

        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains("\"msg\":\"text messages required\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Text_frame_noop_success_emits_noop_ack()
    {
        var frameProcessor = CreateProcessor(
            FfiJsonBridgeResult.Success("[\"NoopAck\"]")
        );
        var state = InitializedState();

        var result = frameProcessor.ProcessFrame(
            WebSocketMessageType.Text,
            Encoding.UTF8.GetBytes("{\"type\":\"noop\"}"),
            state
        );

        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"noop-ack\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Text_frame_close_session_success_requests_close()
    {
        var frameProcessor = CreateProcessor(
            FfiJsonBridgeResult.Success("[]")
        );
        var state = InitializedState();

        var result = frameProcessor.ProcessFrame(
            WebSocketMessageType.Text,
            Encoding.UTF8.GetBytes("{\"type\":\"close-session\"}"),
            state
        );

        Assert.True(result.ShouldCloseConnection);
    }

    [Fact]
    public void Text_frame_close_session_bridge_failure_does_not_close()
    {
        var frameProcessor = CreateProcessor(
            FfiJsonBridgeResult.Failure(AsStatus.Protocol)
        );
        var state = InitializedState();

        var result = frameProcessor.ProcessFrame(
            WebSocketMessageType.Text,
            Encoding.UTF8.GetBytes("{\"type\":\"close-session\"}"),
            state
        );

        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"status\":\"Protocol\"", result.OutboundMessages[0]);
    }

    private static RuntimeFrameProcessor CreateProcessor(params FfiJsonBridgeResult[] bridgeResults)
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