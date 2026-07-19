using Blake3;
using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using System.Security.Cryptography;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Tests;

public sealed class Row19AutomatedScenarioHarnessTests
{
    [Fact]
    public async Task R19_98_multi_device_save_delete_restart_converges_without_resurrection()
    {
        var trace = CreateTrace("r19-98-save-delete-restart", seed: 9801);
        var roomId = $"room-{trace.Seed}";

        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-row19-98", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(tempRoot);
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? preRestartRoot = null;

        await using (var provider = BuildProvider(configuration))
        {
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridgeDeviceA = new FakeScenarioRuntimeCommandBridge();
            var bridgeDeviceB = new FakeScenarioRuntimeCommandBridge();
            var dag = CreateDag(nodeStore, bridgeDeviceA);

            SubmitEnsureRoom(bridgeDeviceA, roomId);
            SubmitMapSet(
                bridgeDeviceA,
                roomId,
                "boards",
                "board-1",
                JsonSerializer.SerializeToElement(new
                {
                    name = "Main",
                    trace_id = trace.TraceId,
                    source = "device-a"
                })
            );

            var packFromA = SubmitRequestServerPack(bridgeDeviceA, roomId);
            await dag.PersistInboundPackAsync(roomId, packFromA.NodesB64, CancellationToken.None);

            SubmitEnsureRoom(bridgeDeviceB, roomId);
            SubmitImportPack(bridgeDeviceB, roomId, packFromA.NodesB64);
            SubmitMapDelete(bridgeDeviceB, roomId, "boards", "board-1");

            var packFromB = SubmitRequestServerPack(bridgeDeviceB, roomId);
            preRestartRoot = packFromB.RootHex;

            await dag.PersistInboundPackAsync(roomId, packFromB.NodesB64, CancellationToken.None);
        }

        await using (var provider = BuildProvider(configuration))
        {
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeScenarioRuntimeCommandBridge();
            var dag = CreateDag(nodeStore, bridge);

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestart = SubmitRequestServerPack(bridge, roomId);
            var state = DecodePack(postRestart.NodesB64);

            Assert.False(string.IsNullOrWhiteSpace(preRestartRoot));
            Assert.Equal(preRestartRoot, postRestart.RootHex);
            Assert.DoesNotContain("boards/board-1", state.Keys);
        }
    }

    [Fact]
    public async Task R19_98_offline_edit_then_reconnect_applies_buffered_ops_deterministically()
    {
        var trace = CreateTrace("r19-98-offline-reconnect", seed: 9802);
        var roomId = $"room-{trace.Seed}";

        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-row19-98", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(tempRoot);
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        var offlineBufferedOps = new[]
        {
            JsonSerializer.SerializeToElement(new { value = "offline-1", trace_id = trace.TraceId }),
            JsonSerializer.SerializeToElement(new { value = "offline-2", trace_id = trace.TraceId }),
            JsonSerializer.SerializeToElement(new { value = "offline-3", trace_id = trace.TraceId })
        };

        string? preRestartRoot = null;

        await using (var provider = BuildProvider(configuration))
        {
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeScenarioRuntimeCommandBridge();
            var dag = CreateDag(nodeStore, bridge);

            SubmitEnsureRoom(bridge, roomId);
            SubmitMapSet(
                bridge,
                roomId,
                "text",
                "draft",
                JsonSerializer.SerializeToElement(new
                {
                    value = "online-initial",
                    trace_id = trace.TraceId
                })
            );

            var initialPack = SubmitRequestServerPack(bridge, roomId);
            await dag.PersistInboundPackAsync(roomId, initialPack.NodesB64, CancellationToken.None);

            // Simulate reconnect replay by deterministically applying buffered offline operations in order.
            foreach (var op in offlineBufferedOps)
            {
                SubmitMapSet(bridge, roomId, "text", "draft", op);
            }

            var replayedPack = SubmitRequestServerPack(bridge, roomId);
            preRestartRoot = replayedPack.RootHex;
            await dag.PersistInboundPackAsync(roomId, replayedPack.NodesB64, CancellationToken.None);
        }

        await using (var provider = BuildProvider(configuration))
        {
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeScenarioRuntimeCommandBridge();
            var dag = CreateDag(nodeStore, bridge);

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestart = SubmitRequestServerPack(bridge, roomId);
            var state = DecodePack(postRestart.NodesB64);

            Assert.False(string.IsNullOrWhiteSpace(preRestartRoot));
            Assert.Equal(preRestartRoot, postRestart.RootHex);
            Assert.True(state.TryGetValue("text/draft", out var textNode));
            Assert.Equal("offline-3", textNode.GetProperty("value").GetString());
            Assert.Equal(trace.TraceId, textNode.GetProperty("trace_id").GetString());
            Assert.Equal(3, offlineBufferedOps.Length);
        }
    }

    [Fact]
    public async Task R19_98_layout_position_integrity_preserves_fixed_and_flexible_coordinates_after_restart()
    {
        var trace = CreateTrace("r19-98-layout-integrity", seed: 9803);
        var roomId = $"room-{trace.Seed}";

        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-row19-98", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(tempRoot);
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? preRestartRoot = null;

        await using (var provider = BuildProvider(configuration))
        {
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeScenarioRuntimeCommandBridge();
            var dag = CreateDag(nodeStore, bridge);

            SubmitEnsureRoom(bridge, roomId);

            SubmitMapSet(
                bridge,
                roomId,
                "boards",
                "fixed",
                JsonSerializer.SerializeToElement(new
                {
                    name = "Fixed",
                    rows = 5,
                    columns = 5,
                    trace_id = trace.TraceId
                })
            );
            SubmitMapSet(
                bridge,
                roomId,
                "positions",
                "fixed-btn",
                JsonSerializer.SerializeToElement(new
                {
                    board = "fixed",
                    row = 4,
                    column = 3,
                    trace_id = trace.TraceId
                })
            );

            SubmitMapSet(
                bridge,
                roomId,
                "boards",
                "flex",
                JsonSerializer.SerializeToElement(new
                {
                    name = "Flexible",
                    rows = 0,
                    columns = 0,
                    trace_id = trace.TraceId
                })
            );
            SubmitMapSet(
                bridge,
                roomId,
                "positions",
                "flex-btn",
                JsonSerializer.SerializeToElement(new
                {
                    board = "flex",
                    x = -12,
                    y = 2048,
                    anchor = "free",
                    trace_id = trace.TraceId
                })
            );

            var pack = SubmitRequestServerPack(bridge, roomId);
            preRestartRoot = pack.RootHex;
            await dag.PersistInboundPackAsync(roomId, pack.NodesB64, CancellationToken.None);
        }

        await using (var provider = BuildProvider(configuration))
        {
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeScenarioRuntimeCommandBridge();
            var dag = CreateDag(nodeStore, bridge);

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestart = SubmitRequestServerPack(bridge, roomId);
            var state = DecodePack(postRestart.NodesB64);

            Assert.False(string.IsNullOrWhiteSpace(preRestartRoot));
            Assert.Equal(preRestartRoot, postRestart.RootHex);

            var fixedBoard = state["boards/fixed"];
            Assert.Equal(5, fixedBoard.GetProperty("rows").GetInt32());
            Assert.Equal(5, fixedBoard.GetProperty("columns").GetInt32());

            var fixedPos = state["positions/fixed-btn"];
            Assert.Equal(4, fixedPos.GetProperty("row").GetInt32());
            Assert.Equal(3, fixedPos.GetProperty("column").GetInt32());

            var flexBoard = state["boards/flex"];
            Assert.Equal(0, flexBoard.GetProperty("rows").GetInt32());
            Assert.Equal(0, flexBoard.GetProperty("columns").GetInt32());

            var flexPos = state["positions/flex-btn"];
            Assert.Equal(-12, flexPos.GetProperty("x").GetInt32());
            Assert.Equal(2048, flexPos.GetProperty("y").GetInt32());
            Assert.Equal("free", flexPos.GetProperty("anchor").GetString());
        }
    }

    [Fact]
    public async Task R19_98_asset_propagation_and_retrieval_survives_restart()
    {
        var trace = CreateTrace("r19-98-asset-propagation", seed: 9804);
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-row19-98", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");
        Directory.CreateDirectory(tempRoot);

        var configuration = BuildSqliteFileConfiguration(dbPath, blobRoot);
        // Slice 3.2 added BLAKE3 verify-on-read to the identity path, so the
        // key must be the real content hash, not a fabricated placeholder.
        var payload = System.Text.Encoding.UTF8.GetBytes($"asset::{trace.TraceId}::v1");
        var blobHash = Hasher.Hash(payload).ToString();

        await using (var provider = BuildProvider(configuration))
        {
            var blobStore = provider.GetRequiredService<IBlobStoreProvider>();

            // Device A write.
            await blobStore.PutBlobAsync(blobHash, payload, "application/octet-stream", CancellationToken.None);

            // Device B read through shared provider store.
            var read = await blobStore.TryGetBlobAsync(blobHash, CancellationToken.None);
            Assert.True(read.Found);
            Assert.Equal(payload, read.Bytes);
        }

        await using (var provider = BuildProvider(configuration))
        {
            var blobStore = provider.GetRequiredService<IBlobStoreProvider>();
            var postRestartRead = await blobStore.TryGetBlobAsync(blobHash, CancellationToken.None);

            Assert.True(postRestartRead.Found);
            Assert.Equal(payload, postRestartRead.Bytes);
        }
    }

    [Fact]
    public async Task R19_98_duplicate_apply_churn_guard_suppresses_repeated_pack_replay()
    {
        var trace = CreateTrace("r19-98-duplicate-churn", seed: 9805);
        var roomId = $"room-{trace.Seed}";

        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-row19-98", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(tempRoot);
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        await using var provider = BuildProvider(configuration);
        var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
        var bridge = new FakeScenarioRuntimeCommandBridge();
        var dag = CreateDag(nodeStore, bridge, enableCompaction: false);

        SubmitEnsureRoom(bridge, roomId);
        SubmitMapSet(
            bridge,
            roomId,
            "text",
            "dup-guard",
            JsonSerializer.SerializeToElement(new
            {
                value = "stable",
                trace_id = trace.TraceId
            })
        );

        var baselinePack = SubmitRequestServerPack(bridge, roomId);
        Assert.False(string.IsNullOrWhiteSpace(baselinePack.RootHex));

        for (var i = 0; i < 25; i++)
        {
            await dag.PersistInboundPackAsync(roomId, baselinePack.NodesB64, CancellationToken.None);
        }

        var snapshot = await nodeStore.LoadRoomSnapshotAsync(roomId, CancellationToken.None);
        Assert.NotNull(snapshot);
        Assert.Single(snapshot!.Nodes);
    }

    [Fact]
    public async Task R19_98_duplicate_apply_churn_soak_10k_replays_stays_within_budget()
    {
        var trace = CreateTrace("r19-98-duplicate-churn-soak", seed: 9806);
        var roomId = $"room-{trace.Seed}";
        const int replayCount = 10_000;
        const int maxPersistedNodeBudget = 2;

        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-row19-98", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(tempRoot);
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        await using var provider = BuildProvider(configuration);
        var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
        var bridge = new FakeScenarioRuntimeCommandBridge();
        var dag = CreateDag(nodeStore, bridge, enableCompaction: false);

        SubmitEnsureRoom(bridge, roomId);
        SubmitMapSet(
            bridge,
            roomId,
            "text",
            "dup-soak",
            JsonSerializer.SerializeToElement(new
            {
                value = "steady",
                trace_id = trace.TraceId
            })
        );

        var baselinePack = SubmitRequestServerPack(bridge, roomId);
        Assert.False(string.IsNullOrWhiteSpace(baselinePack.RootHex));

        for (var i = 0; i < replayCount; i++)
        {
            await dag.PersistInboundPackAsync(roomId, baselinePack.NodesB64, CancellationToken.None);
        }

        var snapshot = await nodeStore.LoadRoomSnapshotAsync(roomId, CancellationToken.None);
        Assert.NotNull(snapshot);
        Assert.True(
            snapshot!.Nodes.Count <= maxPersistedNodeBudget,
            $"expected persisted-node budget <= {maxPersistedNodeBudget} after {replayCount} duplicate replays, got {snapshot.Nodes.Count}");
    }

    private static RuntimeDagPersistenceService CreateDag(INodeStoreProvider nodeStore, IRuntimeCommandBridge bridge, bool enableCompaction = true)
    {
        return new RuntimeDagPersistenceService(
            nodeStore,
            bridge,
            NullLogger<RuntimeDagPersistenceService>.Instance,
            new RuntimeDagCompactionOptions(
                Enabled: enableCompaction,
                MinEligibleNodes: 4,
                EnablePruning: false,
                RetentionWindow: TimeSpan.Zero)
        );
    }

    private static ScenarioTrace CreateTrace(string scenarioId, int seed)
    {
        return new ScenarioTrace(scenarioId, seed, $"{scenarioId}-seed-{seed}");
    }

    private static IConfiguration BuildSqliteNodeOnlyConfiguration(string dbPath)
    {
        return new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "Sqlite",
                    ["NodalMerge:Storage:Sqlite:DbPath"] = dbPath
                }
            )
            .Build();
    }

    private static IConfiguration BuildSqliteFileConfiguration(string dbPath, string blobRoot)
    {
        return new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "Sqlite",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:Sqlite:DbPath"] = dbPath,
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = blobRoot
                }
            )
            .Build();
    }

    private static ServiceProvider BuildProvider(IConfiguration configuration)
    {
        var services = new ServiceCollection();
        services.AddNodalMergeHostProviders(configuration);
        return services.BuildServiceProvider();
    }

    private static void SubmitEnsureRoom(IRuntimeCommandBridge bridge, string roomId)
    {
        var ensure = bridge.ProcessJsonCommand(JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = "EnsureRoom"
        }));
        Assert.Equal(AsStatus.Ok, ensure.Status);
    }

    private static void SubmitMapSet(
        IRuntimeCommandBridge bridge,
        string roomId,
        string @namespace,
        string key,
        JsonElement value)
    {
        var mapSet = bridge.ProcessJsonCommand(JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = new
            {
                MapSet = new
                {
                    @namespace,
                    key,
                    value
                }
            }
        }));

        Assert.Equal(AsStatus.Ok, mapSet.Status);
    }

    private static void SubmitMapDelete(IRuntimeCommandBridge bridge, string roomId, string @namespace, string key)
    {
        var mapDelete = bridge.ProcessJsonCommand(JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = new
            {
                MapDelete = new
                {
                    @namespace,
                    key
                }
            }
        }));

        Assert.Equal(AsStatus.Ok, mapDelete.Status);
    }

    private static void SubmitImportPack(IRuntimeCommandBridge bridge, string roomId, string nodesB64)
    {
        var import = bridge.ProcessJsonCommand(JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = new
            {
                ImportPack = new
                {
                    nodes_b64 = nodesB64
                }
            }
        }));

        Assert.Equal(AsStatus.Ok, import.Status);
    }

    private static (string NodesB64, string? RootHex) SubmitRequestServerPack(IRuntimeCommandBridge bridge, string roomId)
    {
        var response = bridge.ProcessJsonCommand(JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = new
            {
                RequestServerPack = new
                {
                    known_ids = Array.Empty<string>()
                }
            }
        }));

        Assert.Equal(AsStatus.Ok, response.Status);

        using var doc = JsonDocument.Parse(response.EventsJson);
        Assert.Equal(JsonValueKind.Array, doc.RootElement.ValueKind);

        foreach (var evt in doc.RootElement.EnumerateArray())
        {
            if (evt.ValueKind != JsonValueKind.Object
                || !evt.TryGetProperty("ServerPackPrepared", out var serverPack)
                || serverPack.ValueKind != JsonValueKind.Object)
            {
                continue;
            }

            var nodesB64 = serverPack.GetProperty("nodes_b64").GetString();
            var rootHex = serverPack.TryGetProperty("root_hex", out var rootNode)
                ? rootNode.GetString()
                : null;

            Assert.False(string.IsNullOrWhiteSpace(nodesB64));
            return (nodesB64!, rootHex);
        }

        throw new Xunit.Sdk.XunitException("expected ServerPackPrepared event");
    }

    private static Dictionary<string, JsonElement> DecodePack(string nodesB64)
    {
        var payload = Convert.FromBase64String(nodesB64);
        return JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(payload)
            ?? new Dictionary<string, JsonElement>(StringComparer.Ordinal);
    }

    private sealed record ScenarioTrace(string ScenarioId, int Seed, string TraceId);

    private sealed class FakeScenarioRuntimeCommandBridge : IRuntimeCommandBridge
    {
        private readonly Dictionary<string, SortedDictionary<string, JsonElement>> _rooms = new(StringComparer.Ordinal);

        public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
        {
            using var doc = JsonDocument.Parse(commandJson);
            var root = doc.RootElement;

            if (!root.TryGetProperty("room_id", out var roomIdNode) || roomIdNode.ValueKind != JsonValueKind.String)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            var roomId = roomIdNode.GetString();
            if (string.IsNullOrWhiteSpace(roomId)
                || !root.TryGetProperty("command", out var commandNode))
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            if (commandNode.ValueKind == JsonValueKind.String
                && string.Equals(commandNode.GetString(), "EnsureRoom", StringComparison.Ordinal))
            {
                EnsureRoom(roomId!);
                return FfiJsonBridgeResult.Success("[]");
            }

            if (commandNode.ValueKind != JsonValueKind.Object)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            if (commandNode.TryGetProperty("MapSet", out var mapSet) && mapSet.ValueKind == JsonValueKind.Object)
            {
                return HandleMapSet(roomId!, mapSet);
            }

            if (commandNode.TryGetProperty("MapDelete", out var mapDelete) && mapDelete.ValueKind == JsonValueKind.Object)
            {
                return HandleMapDelete(roomId!, mapDelete);
            }

            if (commandNode.TryGetProperty("RequestServerPack", out var requestPack) && requestPack.ValueKind == JsonValueKind.Object)
            {
                return HandleRequestServerPack(roomId!);
            }

            if (commandNode.TryGetProperty("ImportPack", out var importPack) && importPack.ValueKind == JsonValueKind.Object)
            {
                return HandleImportPack(roomId!, importPack);
            }

            return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
        }

        private FfiJsonBridgeResult HandleMapSet(string roomId, JsonElement mapSet)
        {
            EnsureRoom(roomId);
            if (!mapSet.TryGetProperty("namespace", out var nsNode)
                || nsNode.ValueKind != JsonValueKind.String
                || !mapSet.TryGetProperty("key", out var keyNode)
                || keyNode.ValueKind != JsonValueKind.String
                || !mapSet.TryGetProperty("value", out var valueNode))
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            var compositeKey = nsNode.GetString() + "/" + keyNode.GetString();
            _rooms[roomId][compositeKey] = valueNode.Clone();
            return FfiJsonBridgeResult.Success("[]");
        }

        private FfiJsonBridgeResult HandleMapDelete(string roomId, JsonElement mapDelete)
        {
            EnsureRoom(roomId);
            if (!mapDelete.TryGetProperty("namespace", out var nsNode)
                || nsNode.ValueKind != JsonValueKind.String
                || !mapDelete.TryGetProperty("key", out var keyNode)
                || keyNode.ValueKind != JsonValueKind.String)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            var compositeKey = nsNode.GetString() + "/" + keyNode.GetString();
            _rooms[roomId].Remove(compositeKey);
            return FfiJsonBridgeResult.Success("[]");
        }

        private FfiJsonBridgeResult HandleImportPack(string roomId, JsonElement importPack)
        {
            EnsureRoom(roomId);

            if (!importPack.TryGetProperty("nodes_b64", out var nodesB64Node)
                || nodesB64Node.ValueKind != JsonValueKind.String)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            var nodesB64 = nodesB64Node.GetString();
            if (string.IsNullOrWhiteSpace(nodesB64))
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            var payload = Convert.FromBase64String(nodesB64);
            var restored = JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(payload);
            _rooms[roomId] = new SortedDictionary<string, JsonElement>(restored ?? new Dictionary<string, JsonElement>(), StringComparer.Ordinal);

            return FfiJsonBridgeResult.Success("[]");
        }

        private FfiJsonBridgeResult HandleRequestServerPack(string roomId)
        {
            EnsureRoom(roomId);

            var packPayload = BuildPackPayload(_rooms[roomId]);
            var nodesB64 = Convert.ToBase64String(packPayload);
            var rootHex = Convert.ToHexStringLower(SHA256.HashData(packPayload));

            var eventsJson = JsonSerializer.Serialize(new object[]
            {
                new
                {
                    ServerPackPrepared = new
                    {
                        room_id = roomId,
                        nodes_b64 = nodesB64,
                        root_hex = rootHex
                    }
                }
            });

            return FfiJsonBridgeResult.Success(eventsJson);
        }

        private void EnsureRoom(string roomId)
        {
            if (!_rooms.ContainsKey(roomId))
            {
                _rooms[roomId] = new SortedDictionary<string, JsonElement>(StringComparer.Ordinal);
            }
        }

        private static byte[] BuildPackPayload(SortedDictionary<string, JsonElement> state)
        {
            var dict = state.ToDictionary(kvp => kvp.Key, kvp => kvp.Value);
            return JsonSerializer.SerializeToUtf8Bytes(dict);
        }
    }
}
