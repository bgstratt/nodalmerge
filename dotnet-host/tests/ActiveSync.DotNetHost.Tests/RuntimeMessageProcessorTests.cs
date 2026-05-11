using ActiveSync.DotNetHost.Ffi;
using ActiveSync.DotNetHost.Runtime;

namespace ActiveSync.DotNetHost.Tests;

public class RuntimeMessageProcessorTests
{
    [Fact]
    public void Invalid_json_returns_error_envelope_and_no_close()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1);

        var result = processor.ProcessIncomingText("{not-json", state);

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains("\"msg\":\"invalid JSON\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Noop_success_emits_noop_ack()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Success("[\"NoopAck\"]")
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText("{\"type\":\"noop\"}", state);

        Assert.True(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"noop-ack\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Bridge_failure_emits_status_error_and_no_close_for_noop()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Failure(AsStatus.InvalidArg)
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText("{\"type\":\"noop\"}", state);

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"status\":\"InvalidArg\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Event_map_failure_emits_error_and_no_close()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Success("{not-json")
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText("{\"type\":\"noop\"}", state);

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"msg\":\"host event decode failed\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Close_session_success_closes_connection()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Success("[]")
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText("{\"type\":\"close-session\"}", state);

        Assert.True(result.DispatchSucceeded);
        Assert.True(result.ShouldCloseConnection);
    }

    [Fact]
    public void Close_session_bridge_failure_does_not_close_connection()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Failure(AsStatus.Protocol)
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText("{\"type\":\"close-session\"}", state);

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"status\":\"Protocol\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Text_insert_at_resolves_anchor_and_emits_insert_ack()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Success(
                "[{\"TextValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"value\":\"ab\",\"entries\":[{\"id\":\"text-1\",\"ch\":\"a\"},{\"id\":\"text-2\",\"ch\":\"b\"}]}}]"
            ),
            FfiJsonBridgeResult.Success(
                "[{\"TextValueInserted\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"id\":\"text-3\",\"ch\":\"x\"}}]"
            )
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText(
            "{\"type\":\"text-insert-at\",\"namespace\":\"doc\",\"key\":\"title\",\"index\":1,\"ch\":\"x\"}",
            state
        );

        Assert.True(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"text-insert-ack\"", result.OutboundMessages[0]);
        Assert.Equal(2, bridge.Commands.Count);
        Assert.Contains("\"TextGet\"", bridge.Commands[0]);
        Assert.Contains("\"TextInsert\"", bridge.Commands[1]);
        Assert.Contains("\"after_id\":\"text-1\"", bridge.Commands[1]);
    }

    [Fact]
    public void Text_delete_at_resolves_target_and_emits_delete_ack()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Success(
                "[{\"TextValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"value\":\"ab\",\"entries\":[{\"id\":\"text-1\",\"ch\":\"a\"},{\"id\":\"text-2\",\"ch\":\"b\"}]}}]"
            ),
            FfiJsonBridgeResult.Success(
                "[{\"TextValueDeleted\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"target_id\":\"text-2\",\"found\":true}}]"
            )
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText(
            "{\"type\":\"text-delete-at\",\"namespace\":\"doc\",\"key\":\"title\",\"index\":1}",
            state
        );

        Assert.True(result.DispatchSucceeded);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"text-delete-ack\"", result.OutboundMessages[0]);
        Assert.Equal(2, bridge.Commands.Count);
        Assert.Contains("\"TextDelete\"", bridge.Commands[1]);
        Assert.Contains("\"target_id\":\"text-2\"", bridge.Commands[1]);
    }

    [Fact]
    public void Text_insert_at_out_of_range_returns_error()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Success(
                "[{\"TextValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"value\":\"a\",\"entries\":[{\"id\":\"text-1\",\"ch\":\"a\"}]}}]"
            )
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText(
            "{\"type\":\"text-insert-at\",\"namespace\":\"doc\",\"key\":\"title\",\"index\":5,\"ch\":\"x\"}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"msg\":\"text-insert-at.index is out of range\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Text_delete_at_out_of_range_returns_error()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Success(
                "[{\"TextValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"value\":\"a\",\"entries\":[{\"id\":\"text-1\",\"ch\":\"a\"}]}}]"
            )
        );
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText(
            "{\"type\":\"text-delete-at\",\"namespace\":\"doc\",\"key\":\"title\",\"index\":2}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"msg\":\"text-delete-at.index is out of range\"", result.OutboundMessages[0]);
    }
}

internal sealed class FakeRuntimeCommandBridge : IRuntimeCommandBridge
{
    private readonly Queue<FfiJsonBridgeResult> _results;
    public List<string> Commands { get; } = [];

    public FakeRuntimeCommandBridge(params FfiJsonBridgeResult[] results)
    {
        _results = new Queue<FfiJsonBridgeResult>(results);
    }

    public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
    {
        Commands.Add(commandJson);
        if (_results.Count == 0)
        {
            return FfiJsonBridgeResult.Success("[]");
        }

        return _results.Dequeue();
    }
}