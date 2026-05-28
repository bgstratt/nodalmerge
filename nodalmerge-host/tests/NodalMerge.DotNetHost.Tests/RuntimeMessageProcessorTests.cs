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
    public void Query_register_without_capability_returns_control_plane_forbidden_error_envelope_with_query_metric_tags()
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
            "query.register",
            "query.admin",
            "reject.control_plane_forbidden"
        );

        var result = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains("reject.control_plane_forbidden: command=query.register requires=query.admin", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "query.register", "query.admin", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
    }

    [Fact]
    public void Projection_read_without_capability_returns_control_plane_forbidden_error_envelope_with_query_metric_tags()
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
            "projection.read",
            "query.read",
            "reject.control_plane_forbidden"
        );

        var result = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains("reject.control_plane_forbidden: command=projection.read requires=query.read", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "projection.read", "query.read", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
    }

    [Fact]
    public void Archive_describe_without_capability_returns_control_plane_forbidden_error_envelope_with_archive_metric_tags()
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
            "archive.describe",
            "archive.read",
            "reject.control_plane_forbidden"
        );

        var result = processor.ProcessIncomingText(
            "{\"type\":\"archive.describe\",\"archive_ref\":\"s3://bucket/room-a.nmar\"}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains("reject.control_plane_forbidden: command=archive.describe requires=archive.read", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "archive.describe", "archive.read", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
    }

    [Fact]
    public void Query_register_executes_local_stub_and_emits_query_registered_message_without_bridge_call()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var result = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );

        Assert.True(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"query.registered\"", result.OutboundMessages[0]);
        Assert.Contains("\"query_spec_id\":\"q.rooms\"", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_read_executes_local_stub_and_emits_projection_read_result_without_bridge_call()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.read");

        var result = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );

        Assert.True(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"projection.read.result\"", result.OutboundMessages[0]);
        Assert.Contains("\"projection_id\":\"p.rooms\"", result.OutboundMessages[0]);
        Assert.Contains("\"rows\":[]", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Archive_validate_executes_local_stub_and_emits_deterministic_reject_reason_class_without_bridge_call()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("archive.admin");

        var result = processor.ProcessIncomingText(
            "{\"type\":\"archive.validate\",\"archive_ref\":\"s3://bucket/invalid-manifest.nmar\",\"mode\":\"full_integrity\"}",
            state
        );

        Assert.True(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"archive.validate.rejected\"", result.OutboundMessages[0]);
        Assert.Contains("\"reason_class\":\"reject.archive_manifest_invalid\"", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Topology_propose_without_capability_returns_control_plane_forbidden_error_envelope()
    {
        using var metrics = new ControlPlaneDenyMeterCapture();
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "parent-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = processor.ProcessIncomingText(
            "{\"type\":\"topology.propose-promotion\",\"parent_room_id\":\"parent-a\",\"child_room_id\":\"child-a\",\"child_checkpoint_hash\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"payload_ref\":\"artifact://x\"}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.Contains("reject.control_plane_forbidden: command=topology.propose-promotion requires=topology.admin", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Topology_promotion_stub_executes_propose_validate_apply_without_bridge_call()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "parent-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("topology.admin");

        var propose = processor.ProcessIncomingText(
            "{\"type\":\"topology.propose-promotion\",\"parent_room_id\":\"parent-a\",\"child_room_id\":\"child-a\",\"child_checkpoint_hash\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"payload_ref\":\"artifact://dotnet\",\"idempotency_key\":\"prop-dotnet\"}",
            state
        );
        Assert.True(propose.DispatchSucceeded);
        Assert.Contains("\"type\":\"topology.propose-promotion.completed\"", propose.OutboundMessages[0]);

        var validate = processor.ProcessIncomingText(
            "{\"type\":\"topology.validate-promotion\",\"proposal_id\":\"prop-dotnet\"}",
            state
        );
        Assert.True(validate.DispatchSucceeded);
        Assert.Contains("\"type\":\"topology.validate-promotion.completed\"", validate.OutboundMessages[0]);

        var apply = processor.ProcessIncomingText(
            "{\"type\":\"topology.apply-promotion\",\"proposal_id\":\"prop-dotnet\"}",
            state
        );
        Assert.True(apply.DispatchSucceeded);
        Assert.Contains("\"type\":\"topology.apply-promotion.completed\"", apply.OutboundMessages[0]);
        Assert.Contains("\"audit_key\":\"_topology/promotion/prop-dotnet\"", apply.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Archive_import_executes_local_stub_and_emits_import_completed_with_checkpoint_hash_without_bridge_call()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("archive.admin");

        var result = processor.ProcessIncomingText(
            "{\"type\":\"archive.import\",\"archive_ref\":\"s3://bucket/room-a.nmar\",\"import_mode\":\"full_clone\"}",
            state
        );

        Assert.True(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"archive.import.completed\"", result.OutboundMessages[0]);
        Assert.Contains("\"checkpoint\":", result.OutboundMessages[0]);
        Assert.Contains("\"canonical_hash\":\"1111111111111111111111111111111111111111111111111111111111111111\"", result.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Query_projection_stub_state_persists_across_register_build_read_list_and_invalidate()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");
        state.SessionCapabilities.Add("query.read");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"query.registered\"", registerResult.OutboundMessages[0]);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"latest\"}}",
            state
        );
        Assert.True(buildResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.build.completed\"", buildResult.OutboundMessages[0]);
        Assert.Contains("\"digest\":\"", buildResult.OutboundMessages[0]);

        var readResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );
        Assert.True(readResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.read.result\"", readResult.OutboundMessages[0]);
        Assert.Contains("\"query_spec_id\":\"q.rooms\"", readResult.OutboundMessages[0]);
        Assert.Contains("\"digest\":\"", readResult.OutboundMessages[0]);

        var listActiveResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.list\",\"query_spec_id\":\"q.rooms\",\"state_filter\":\"active\"}",
            state
        );
        Assert.True(listActiveResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.list.result\"", listActiveResult.OutboundMessages[0]);
        Assert.Contains("\"projection_id\":\"p.rooms\"", listActiveResult.OutboundMessages[0]);
        Assert.Contains("\"state\":\"active\"", listActiveResult.OutboundMessages[0]);

        var invalidateResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.invalidate\",\"projection_id\":\"p.rooms\",\"reason\":\"manual\"}",
            state
        );
        Assert.True(invalidateResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.invalidated\"", invalidateResult.OutboundMessages[0]);

        var listInvalidatedResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.list\",\"query_spec_id\":\"q.rooms\",\"state_filter\":\"invalidated\"}",
            state
        );
        Assert.True(listInvalidatedResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.list.result\"", listInvalidatedResult.OutboundMessages[0]);
        Assert.Contains("\"projection_id\":\"p.rooms\"", listInvalidatedResult.OutboundMessages[0]);
        Assert.Contains("\"state\":\"invalidated\"", listInvalidatedResult.OutboundMessages[0]);

        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_read_stub_paginates_with_offset_page_tokens_and_limit_windows_deterministically()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");
        state.SessionCapabilities.Add("query.read");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"latest\"}}",
            state
        );
        Assert.True(buildResult.DispatchSucceeded);

        var page1 = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":1}",
            state
        );
        Assert.True(page1.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.read.result\"", page1.OutboundMessages[0]);
        Assert.Contains("\"k\":\"world/a\"", page1.OutboundMessages[0]);
        Assert.Contains("\"next_page_token\":\"offset:1\"", page1.OutboundMessages[0]);

        var page2 = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":1,\"page_token\":\"offset:1\"}",
            state
        );
        Assert.True(page2.DispatchSucceeded);
        Assert.Contains("\"k\":\"world/b\"", page2.OutboundMessages[0]);
        Assert.Contains("\"next_page_token\":\"offset:2\"", page2.OutboundMessages[0]);

        var page3 = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":2,\"page_token\":\"offset:2\"}",
            state
        );
        Assert.True(page3.DispatchSucceeded);
        Assert.Contains("\"k\":\"world/c\"", page3.OutboundMessages[0]);
        Assert.Contains("\"next_page_token\":null", page3.OutboundMessages[0]);

        // Determinism guardrail: first page is stable across repeated reads.
        var page1Repeat = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":1}",
            state
        );
        Assert.True(page1Repeat.DispatchSucceeded);
        Assert.Equal(page1.OutboundMessages[0], page1Repeat.OutboundMessages[0]);

        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_build_uses_canonical_map_state_and_reflects_map_mutations()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");
        state.SessionCapabilities.Add("query.read");

        var addResult = processor.ProcessIncomingText(
            "{\"type\":\"map-set\",\"key\":\"world/d\",\"value\":\"4\"}",
            state
        );
        Assert.True(addResult.DispatchSucceeded);

        var deleteResult = processor.ProcessIncomingText(
            "{\"type\":\"map-delete\",\"key\":\"world/b\"}",
            state
        );
        Assert.True(deleteResult.DispatchSucceeded);

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\"}",
            state
        );
        Assert.True(buildResult.DispatchSucceeded);

        var readResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.read\",\"projection_id\":\"p.rooms\",\"limit\":10}",
            state
        );
        Assert.True(readResult.DispatchSucceeded);
        Assert.Contains("\"k\":\"world/a\"", readResult.OutboundMessages[0]);
        Assert.Contains("\"k\":\"world/c\"", readResult.OutboundMessages[0]);
        Assert.Contains("\"k\":\"world/d\"", readResult.OutboundMessages[0]);
        Assert.DoesNotContain("\"k\":\"world/b\"", readResult.OutboundMessages[0]);
    }

    [Fact]
    public void Projection_build_rejects_unknown_checkpoint_sequence()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"seq\",\"canonical_seq\":999}}",
            state
        );

        Assert.True(buildResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.build.rejected\"", buildResult.OutboundMessages[0]);
        Assert.Contains("\"reason_class\":\"reject.checkpoint_not_found\"", buildResult.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_build_rejects_unknown_checkpoint_hash()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"hash\",\"canonical_hash\":\"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\"}}",
            state
        );

        Assert.True(buildResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.build.rejected\"", buildResult.OutboundMessages[0]);
        Assert.Contains("\"reason_class\":\"reject.checkpoint_not_found\"", buildResult.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_build_rejects_malformed_frontier_token_with_selector_invalid()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"frontier\",\"frontier\":[\"bad:1\"]}}",
            state
        );

        Assert.True(buildResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.build.rejected\"", buildResult.OutboundMessages[0]);
        Assert.Contains("\"reason_class\":\"reject.checkpoint_selector_invalid\"", buildResult.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_build_rejects_mixed_selector_fields_with_selector_invalid()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"seq\",\"canonical_seq\":0,\"canonical_hash\":\"deadbeef\"}}",
            state
        );

        Assert.True(buildResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.build.rejected\"", buildResult.OutboundMessages[0]);
        Assert.Contains("\"reason_class\":\"reject.checkpoint_selector_invalid\"", buildResult.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_build_rejects_invalid_canonical_hash_format_with_selector_invalid()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var buildResult = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"hash\",\"canonical_hash\":\"zzzz\"}}",
            state
        );

        Assert.True(buildResult.DispatchSucceeded);
        Assert.Contains("\"type\":\"projection.build.rejected\"", buildResult.OutboundMessages[0]);
        Assert.Contains("\"reason_class\":\"reject.checkpoint_selector_invalid\"", buildResult.OutboundMessages[0]);
        Assert.Empty(bridge.Commands);
    }

    [Fact]
    public void Projection_build_selector_payload_validation_uses_bounded_rejection_taxonomy()
    {
        var bridge = new FakeRuntimeCommandBridge();
        var mapper = new RuntimeProtocolMapper();
        var processor = new RuntimeMessageProcessor(bridge, mapper);
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("query.admin");

        var registerResult = processor.ProcessIncomingText(
            "{\"type\":\"query.register\",\"query_spec_id\":\"q.rooms\",\"version\":\"v1\",\"descriptor\":{\"kind\":\"map_prefix\",\"prefix\":\"world/\"}}",
            state
        );
        Assert.True(registerResult.DispatchSucceeded);

        var malformedFrontier = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"frontier\",\"frontier\":[\"bad\"]}}",
            state
        );
        var mixedFields = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"seq\",\"canonical_seq\":1,\"frontier\":[\"seq:1\"]}}",
            state
        );
        var badHashFormat = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"hash\",\"canonical_hash\":\"1234\"}}",
            state
        );
        var unknownHash = processor.ProcessIncomingText(
            "{\"type\":\"projection.build\",\"projection_id\":\"p.rooms\",\"query_spec_id\":\"q.rooms\",\"target_checkpoint\":{\"selector\":\"hash\",\"canonical_hash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"}}",
            state
        );

        var reasonClasses = new[]
        {
            ExtractReasonClass(malformedFrontier.OutboundMessages[0]),
            ExtractReasonClass(mixedFields.OutboundMessages[0]),
            ExtractReasonClass(badHashFormat.OutboundMessages[0]),
            ExtractReasonClass(unknownHash.OutboundMessages[0])
        };

        Assert.Equal("reject.checkpoint_selector_invalid", reasonClasses[0]);
        Assert.Equal("reject.checkpoint_selector_invalid", reasonClasses[1]);
        Assert.Equal("reject.checkpoint_selector_invalid", reasonClasses[2]);
        Assert.Equal("reject.checkpoint_not_found", reasonClasses[3]);

        var allowed = new HashSet<string>(StringComparer.Ordinal)
        {
            "reject.checkpoint_selector_invalid",
            "reject.checkpoint_not_found"
        };
        Assert.All(reasonClasses, reasonClass =>
        {
            Assert.NotNull(reasonClass);
            Assert.Contains(reasonClass!, allowed);
        });
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

    private static string? ExtractReasonClass(string json)
    {
        const string marker = "\"reason_class\":\"";
        var start = json.IndexOf(marker, StringComparison.Ordinal);
        if (start < 0)
        {
            return null;
        }

        start += marker.Length;
        var end = json.IndexOf('"', start);
        if (end < 0)
        {
            return null;
        }

        return json[start..end];
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