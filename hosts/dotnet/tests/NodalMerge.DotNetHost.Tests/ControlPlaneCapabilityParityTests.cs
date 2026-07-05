using System.Text.Json;
using NodalMerge.DotNetHost.Runtime;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Asserts this host's live control-plane capability gating against the
/// canonical registry (<c>engine/commands/registry.json</c>, linked into this
/// project as <c>command-registry.json</c>). The Rust mirror is
/// <c>server/server/tests/control_plane_capability_parity.rs</c>, which reads
/// the same file via the <c>nodalmerge-command-registry</c> crate. To
/// add/remove/re-gate a command, change the registry and the implementation
/// together — this test fails on any drift.
///
/// Rather than a second hand-maintained table, this drives the real
/// message-handling path (<see cref="RuntimeMessageProcessor.ProcessIncomingText"/>)
/// with an empty capability set for every registry row whose
/// <c>dotnet_host</c> surface is not <c>absent</c>, and asserts the exact
/// reject envelope. Rows marked <c>absent</c> (today: <c>archive.export</c>,
/// <c>replay.read-range</c>) are asserted to NOT produce a
/// control-plane-forbidden rejection, so silently adding a gated route
/// without updating the registry also fails.
///
/// The registry covers *gating* only — several commands are wire-compatible
/// stubs beyond the gate (see the registry's <c>surfaces</c>/<c>notes</c>).
/// </summary>
public sealed class ControlPlaneCapabilityParityTests
{
    private sealed record RegistryRow(string Command, string RequiredCapability, string DotnetStatus);

    private static readonly IReadOnlyList<RegistryRow> Rows = LoadRegistry();

    private static IReadOnlyList<RegistryRow> LoadRegistry()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "command-registry.json");
        using var doc = JsonDocument.Parse(File.ReadAllText(path));
        var rows = new List<RegistryRow>();
        foreach (var el in doc.RootElement.GetProperty("commands").EnumerateArray())
        {
            rows.Add(new RegistryRow(
                el.GetProperty("command").GetString()!,
                el.GetProperty("required_capability").GetString()!,
                el.GetProperty("surfaces").GetProperty("dotnet_host").GetString()!
            ));
        }
        return rows;
    }

    public static TheoryData<string, string> GatedCommands()
    {
        var data = new TheoryData<string, string>();
        foreach (var row in Rows.Where(r => r.DotnetStatus != "absent"))
        {
            data.Add(row.Command, row.RequiredCapability);
        }
        return data;
    }

    public static TheoryData<string> AbsentCommands()
    {
        var data = new TheoryData<string>();
        foreach (var row in Rows.Where(r => r.DotnetStatus == "absent"))
        {
            data.Add(row.Command);
        }
        return data;
    }

    private static RuntimeMessageProcessResult Process(string command)
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
        return processor.ProcessIncomingText($"{{\"type\":\"{command}\"}}", state);
    }

    [Fact]
    public void RegistryLoadsWithRows()
    {
        Assert.NotEmpty(Rows);
        Assert.Contains(Rows, r => r.Command == "set-policy");
    }

    [Theory]
    [MemberData(nameof(GatedCommands))]
    public void CommandIsRejectedWithoutRequiredCapability(string command, string requiredCapability)
    {
        var result = Process(command);

        Assert.False(result.DispatchSucceeded);
        Assert.Single(result.OutboundMessages);
        Assert.Contains(
            $"reject.control_plane_forbidden: command={command} requires={requiredCapability}",
            result.OutboundMessages[0]
        );
    }

    [Theory]
    [MemberData(nameof(AbsentCommands))]
    public void AbsentCommandIsNotCapabilityGated(string command)
    {
        var result = Process(command);

        // Unrouted commands fail for other reasons (unknown type), but must
        // not emit a control-plane-forbidden rejection: if this fires, a
        // route was added without updating engine/commands/registry.json.
        foreach (var message in result.OutboundMessages)
        {
            Assert.DoesNotContain("reject.control_plane_forbidden", message);
        }
    }
}
