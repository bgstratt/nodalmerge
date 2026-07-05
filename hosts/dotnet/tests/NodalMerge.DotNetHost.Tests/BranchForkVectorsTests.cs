using NodalMerge.DotNetHost.Runtime;

namespace NodalMerge.DotNetHost.Tests;

public sealed class BranchForkVectorsTests
{
    [Fact]
    public void BranchFork006UnauthorizedRejected()
    {
        // Wave 0 stub for BRANCH-FORK-006.
        // `fork-room` command surface is not yet exposed in RuntimeProtocolMapper,
        // so we lock in the same admin-gated reject metadata contract through the
        // nearest control-plane surrogate (`set-room-key` requiring `room.admin`).
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
