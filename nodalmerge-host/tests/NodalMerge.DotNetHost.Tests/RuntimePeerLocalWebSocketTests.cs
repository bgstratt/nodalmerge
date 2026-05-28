using System.Net.WebSockets;
using System.Text;
using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using Microsoft.Extensions.Logging.Abstractions;

namespace NodalMerge.DotNetHost.Tests;

public class RuntimePeerLocalWebSocketTests
{
    // Deterministic pack from runtime-local-ffi `dump_pack_b64_fixture` (make_nodes + pack_nodes).
    private const string SamplePackNodesB64 =
        "ATjYeUZ1dzKx/y1ho/P6XQdQ8YmMxOd5wO1PRscM4/G84v4qObcyZkKCfW2C1JdiLNbCl/54Jw6qQ2sB8Ia288sBAAEAAAl3b3JsZC9jbGkDZmZpALmstYPVzEJ1bhFNZTleiK5U/idAHbCNnBoSauXFfQvV5uwdUnMQ9NynyTfIffemFSR8OTvUEvgYNUhnZQga6Ag=";

    private static bool LocalFfiAvailable =>
        !string.IsNullOrWhiteSpace(Environment.GetEnvironmentVariable("NODALMERGE_LOCAL_FFI_DLL"))
        || File.Exists(Path.Combine(AppContext.BaseDirectory, "nodalmerge_runtime_local_ffi.dll"))
        || File.Exists(Path.Combine(Directory.GetCurrentDirectory(), "target", "release", "nodalmerge_runtime_local_ffi.dll"))
        || File.Exists(Path.Combine(Directory.GetCurrentDirectory(), "target", "debug", "nodalmerge_runtime_local_ffi.dll"));

    [Fact]
    public async Task WebSocket_loop_flushes_peer_local_after_pack_when_enabled()
    {
        if (!LocalFfiAvailable)
        {
            return;
        }

        NativeLibraryResolver.Configure();
        var options = new RuntimePeerLocalPersistenceOptions
        {
            Enabled = true,
            Backend = "memory"
        };
        using var peerLocal = new RuntimePeerLocalPersistenceService(
            options,
            NullLogger<RuntimePeerLocalPersistenceService>.Instance
        );
        Assert.True(peerLocal.IsEnabled);

        var runner = new RuntimeWebSocketLoopRunner();
        var state = new RuntimeConnectionState(42)
        {
            IsInitialized = true,
            RoomId = "room-peerlocal-ws",
            PeerPubkeyHex = "aa".PadRight(64, 'b')
        };

        var frameProcessor = CreateFrameProcessor(FfiJsonBridgeResult.Success("[]"));

        var packJson =
            $"{{\"type\":\"pack\",\"room\":\"{state.RoomId}\",\"nodes\":\"{SamplePackNodesB64}\",\"trace_id\":\"peerlocal-test\"}}";
        var socket = new FakeWebSocket([
            FakeReceiveFrame.Text(packJson),
            FakeReceiveFrame.Close()
        ]);

        await runner.RunAsync(
            socket,
            frameProcessor,
            state,
            peerLocalPersistenceService: peerLocal,
            cancellationToken: CancellationToken.None
        );

        using var client = new LocalPersistFfiClient("memory");
        var hash = client.CanonicalHashHex(state.RoomId!);
        Assert.Equal(64, hash.Length);
    }

    private static RuntimeFrameProcessor CreateFrameProcessor(params FfiJsonBridgeResult[] bridgeResults)
    {
        var bridge = new StubRuntimeCommandBridge(bridgeResults);
        return new RuntimeFrameProcessor(new RuntimeMessageProcessor(bridge, new RuntimeProtocolMapper()));
    }

    private sealed class StubRuntimeCommandBridge : IRuntimeCommandBridge
    {
        private readonly Queue<FfiJsonBridgeResult> _results;

        public StubRuntimeCommandBridge(params FfiJsonBridgeResult[] results)
        {
            _results = new Queue<FfiJsonBridgeResult>(results.Length > 0 ? results : [FfiJsonBridgeResult.Success("[]")]);
        }

        public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
        {
            return _results.Count > 1 ? _results.Dequeue() : _results.Peek();
        }
    }
}
