using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using System.Diagnostics.Metrics;

namespace NodalMerge.DotNetHost.Tests;

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
    public void Bridge_failure_with_deny_metadata_surfaces_diagnostics_in_error_envelope()
    {
        var bridge = new FakeRuntimeCommandBridge(
            FfiJsonBridgeResult.Failure(
                AsStatus.Protocol,
                new FfiDenyMetadata(
                    "reject.protocol_violation",
                    "client-hello",
                    "unknown",
                    "peer mismatch"
                )
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

        var result = processor.ProcessIncomingText("{\"type\":\"noop\"}", state);

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"status\":\"Protocol\"", result.OutboundMessages[0]);
        Assert.Contains("\"msg\":\"peer mismatch\"", result.OutboundMessages[0]);
        Assert.Contains("\"reason_class\":\"reject.protocol_violation\"", result.OutboundMessages[0]);
        Assert.Contains("\"command\":\"client-hello\"", result.OutboundMessages[0]);
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
    public void Set_policy_without_capability_returns_control_plane_forbidden_error_envelope()
    {
        using var metrics = new ControlPlaneDenyMeterCapture();
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var beforeDenied = metrics.GetTotalByTags(
            "dotnet-host",
            "set-policy",
            "policy.admin",
            "reject.control_plane_forbidden"
        );

        var result = processor.ProcessIncomingText("{\"type\":\"set-policy\",\"default\":\"allow\",\"rules\":[]}", state);

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains("reject.control_plane_forbidden: command=set-policy requires=policy.admin", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "set-policy", "policy.admin", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
    }

    [Fact]
    public void Start_tick_without_capability_returns_control_plane_forbidden_error_envelope()
    {
        using var metrics = new ControlPlaneDenyMeterCapture();
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var beforeDenied = metrics.GetTotalByTags(
            "dotnet-host",
            "start-tick",
            "tick.admin",
            "reject.control_plane_forbidden"
        );

        var result = processor.ProcessIncomingText("{\"type\":\"start-tick\"}", state);

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains("reject.control_plane_forbidden: command=start-tick requires=tick.admin", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "start-tick", "tick.admin", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
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

internal sealed class ControlPlaneDenyMeterCapture : IDisposable
{
    private readonly MeterListener _listener;
    private readonly Dictionary<string, long> _totalsByTags = new(StringComparer.Ordinal);

    public ControlPlaneDenyMeterCapture()
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