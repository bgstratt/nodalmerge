using NodalMerge.DotNetHost.Runtime;

namespace NodalMerge.DotNetHost.Tests;

public sealed class SpecAuthVectorsTests
{
    [Fact]
    public void SpecAuth003RejectedIntentRollsBack()
    {
        // Wave 0 stub for SPEC-AUTH-003.
        // Intent command surface is not yet exposed as a dedicated protocol
        // command in DotNet host runtime. This test locks in deterministic
        // reject metadata + no-dispatch behavior using the nearest
        // capability-gated surrogate command.
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

        var result = processor.ProcessIncomingText(
            "{\"type\":\"set-policy\",\"default\":\"allow\",\"rules\":[]}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains(
            "reject.control_plane_forbidden: command=set-policy requires=policy.admin",
            result.OutboundMessages[0]
        );
        Assert.Empty(bridge.Commands);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "set-policy", "policy.admin", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
    }

    [Fact]
    public void SpecAuth006CapabilityGatedIntentNamespaceReject()
    {
        // Wave 0 stub for SPEC-AUTH-006.
        // Until intent namespace commands are exposed explicitly in host protocol,
        // we use the existing capability-gated control-plane command to lock in
        // deterministic reject metadata and metric tags.
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
            "set-room-key",
            "room.admin",
            "reject.control_plane_forbidden"
        );

        var result = processor.ProcessIncomingText(
            "{\"type\":\"set-room-key\",\"pubkey\":\"peer-admin\"}",
            state
        );

        Assert.False(result.DispatchSucceeded);
        Assert.False(result.ShouldCloseConnection);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"error\"", result.OutboundMessages[0]);
        Assert.Contains(
            "reject.control_plane_forbidden: command=set-room-key requires=room.admin",
            result.OutboundMessages[0]
        );
        Assert.Empty(bridge.Commands);
        Assert.True(
            metrics.GetTotalByTags("dotnet-host", "set-room-key", "room.admin", "reject.control_plane_forbidden")
            >= beforeDenied + 1
        );
    }
}
