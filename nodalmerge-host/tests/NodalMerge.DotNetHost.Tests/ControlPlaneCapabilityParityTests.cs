using NodalMerge.DotNetHost.Runtime;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Cross-runtime parity check for control-plane capability gating.
///
/// This is one half of a parity pair: the Rust side asserts the same table
/// (as data, not code) against
/// <c>nodalmerge_server::ws_handler::required_capability_for_control_plane_command</c>
/// in <c>server/tests/control_plane_capability_parity.rs</c>. If you add,
/// remove, or re-gate a control-plane command in either runtime, update the
/// table in *both* places in the same change, or these two tests will
/// disagree about what "correct" means without either one failing.
///
/// Unlike the Rust side (which calls a pure lookup function), this drives the
/// real message-handling path (<see cref="RuntimeMessageProcessor.ProcessIncomingText"/>)
/// with an empty capability set, since <see cref="RuntimeProtocolMapper"/> has
/// no equivalent standalone lookup function today — the gate checks are
/// inlined per command. That means this test exercises actual runtime
/// behavior rather than a second hand-maintained copy of the table.
///
/// Commands that exist only on the Rust side (no WS route implemented here
/// yet) are intentionally not represented here — see the corresponding
/// comment in the Rust table.
///
/// This only covers *gating*, not whether the command does the real thing
/// once authorized. <c>archive.import</c> passes this test (it's correctly
/// gated) but is a known non-functional stub end-to-end on this runtime —
/// see the comment on its handler in RuntimeProtocolMapper.cs and on
/// <c>HostCommand::ImportArchive</c> in host-core/src/engine.rs.
/// <c>archive.export</c> has no .NET path at all and is excluded from this
/// table for that reason (not because gating is wrong — there's nothing to
/// gate).
/// </summary>
public sealed class ControlPlaneCapabilityParityTests
{
    public static readonly TheoryData<string, string> GatedCommands = new()
    {
        { "set-policy", "policy.admin" },
        { "set-room-key", "room.admin" },
        { "start-tick", "tick.admin" },
        { "stop-tick", "tick.admin" },
        { "archive.describe", "archive.read" },
        { "archive.validate", "archive.admin" },
        { "archive.import", "archive.admin" },
        { "query.register", "query.admin" },
        { "projection.build", "query.admin" },
        { "projection.invalidate", "query.admin" },
        { "projection.read", "query.read" },
        { "projection.list", "query.read" },
        { "topology.create-child", "topology.admin" },
        { "topology.describe-lineage", "topology.admin" },
        { "topology.list-children", "topology.admin" },
        { "topology.propose-promotion", "topology.admin" },
        { "topology.validate-promotion", "topology.admin" },
        { "topology.apply-promotion", "topology.admin" },
        // .NET-only today: host-core already has these HostCommand variants,
        // but nodalmerge-server has no WS route for them yet. See the mirror
        // comment in server/tests/control_plane_capability_parity.rs.
        { "checkpoint.promote", "query.admin" },
        { "graph.get-frontier", "query.admin" },
        { "graph.get-causal-parents", "query.admin" },
        { "graph.get-canonical-resolution", "query.admin" },
        { "graph.compute-sync-diff", "query.admin" },
    };

    [Theory]
    [MemberData(nameof(GatedCommands))]
    public void CommandIsRejectedWithoutRequiredCapability(string command, string requiredCapability)
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

        var result = processor.ProcessIncomingText($"{{\"type\":\"{command}\"}}", state);

        Assert.False(result.DispatchSucceeded);
        Assert.Empty(bridge.Commands);
        Assert.Single(result.OutboundMessages);
        Assert.Contains(
            $"reject.control_plane_forbidden: command={command} requires={requiredCapability}",
            result.OutboundMessages[0]
        );
    }
}
