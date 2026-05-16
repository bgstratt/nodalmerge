using ActiveSync.DotNetHost.Ffi;
using ActiveSync.DotNetHost.Runtime;
using ActiveSync.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging.Abstractions;
using System.Diagnostics.Metrics;
using System.Security.Cryptography;
using System.Text.Json;

namespace ActiveSync.DotNetHost.Tests;

public class RuntimeDagPersistenceServiceTests
{
    [Fact]
    public async Task HydrateRoomIfNeededAsync_AppliesSnapshotThenPostBoundaryDelta()
    {
        var roomId = "room-a";
        var now = DateTimeOffset.UtcNow;

        var snapshotPayload = new byte[] { 1, 2, 3 };
        var oldDeltaPayload = new byte[] { 4, 5, 6 };
        var newDeltaPayload = new byte[] { 7, 8, 9 };

        var store = new TestNodeStoreProvider
        {
            CompactionSnapshot = new CompactionSnapshot(
                roomId,
                snapshotPayload,
                now.AddMinutes(-5)
            ),
            RoomSnapshot = new NodeSnapshot(
                roomId,
                new[]
                {
                    new AcceptedNodeRecord(
                        "pack:old",
                        oldDeltaPayload,
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-10)
                    ),
                    new AcceptedNodeRecord(
                        "pack:new",
                        newDeltaPayload,
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-1)
                    )
                }
            )
        };

        var bridge = new RecordingRuntimeCommandBridge();
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        await service.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);

        Assert.Equal(AsStatus.Ok, bridge.EnsureRoomStatus);
        Assert.Equal(2, bridge.ImportPayloads.Count);
        Assert.Equal(snapshotPayload, bridge.ImportPayloads[0]);
        Assert.Equal(newDeltaPayload, bridge.ImportPayloads[1]);
    }

    [Fact]
    public async Task HydrateRoomIfNeededAsync_FallsBackToAcceptedNodes_WhenSnapshotImportFails()
    {
        var roomId = "room-b";
        var now = DateTimeOffset.UtcNow;

        var snapshotPayload = new byte[] { 10, 11, 12 };
        var firstDeltaPayload = new byte[] { 13, 14, 15 };
        var secondDeltaPayload = new byte[] { 16, 17, 18 };

        var store = new TestNodeStoreProvider
        {
            CompactionSnapshot = new CompactionSnapshot(
                roomId,
                snapshotPayload,
                now.AddMinutes(-5)
            ),
            RoomSnapshot = new NodeSnapshot(
                roomId,
                new[]
                {
                    new AcceptedNodeRecord(
                        "pack:first",
                        firstDeltaPayload,
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-10)
                    ),
                    new AcceptedNodeRecord(
                        "pack:second",
                        secondDeltaPayload,
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-1)
                    )
                }
            )
        };

        var bridge = new RecordingRuntimeCommandBridge
        {
            FailingImportPayload = snapshotPayload
        };
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        await service.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);

        Assert.Equal(AsStatus.Ok, bridge.EnsureRoomStatus);
        Assert.Equal(2, bridge.ImportPayloads.Count);
        Assert.Equal(firstDeltaPayload, bridge.ImportPayloads[0]);
        Assert.Equal(secondDeltaPayload, bridge.ImportPayloads[1]);
    }

    [Fact]
    public async Task HydrateRoomIfNeededAsync_UsesBoundaryNodeOrdering_WhenBoundaryIdIsPresent()
    {
        var roomId = "room-boundary";
        var now = DateTimeOffset.UtcNow;

        var snapshotPayload = new byte[] { 21, 22, 23 };
        var boundaryPayload = new byte[] { 24, 25, 26 };
        var deltaAfterBoundaryPayload = new byte[] { 27, 28, 29 };

        var store = new TestNodeStoreProvider
        {
            CompactionSnapshot = new CompactionSnapshot(
                roomId,
                snapshotPayload,
                now.AddMinutes(-5),
                BoundaryNodeIdHex: "pack:boundary"
            ),
            RoomSnapshot = new NodeSnapshot(
                roomId,
                new[]
                {
                    new AcceptedNodeRecord(
                        "pack:boundary",
                        boundaryPayload,
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-1)
                    ),
                    new AcceptedNodeRecord(
                        "pack:delta-after-boundary",
                        deltaAfterBoundaryPayload,
                        AcceptedNodeKinds.Pack,
                        // Intentionally older than snapshot.CreatedAtUtc to prove boundary-order takes precedence.
                        AcceptedAtUtc: now.AddMinutes(-10)
                    )
                }
            )
        };

        var bridge = new RecordingRuntimeCommandBridge();
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        await service.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);

        Assert.Equal(AsStatus.Ok, bridge.EnsureRoomStatus);
        Assert.Equal(2, bridge.ImportPayloads.Count);
        Assert.Equal(snapshotPayload, bridge.ImportPayloads[0]);
        Assert.Equal(deltaAfterBoundaryPayload, bridge.ImportPayloads[1]);
    }

    [Fact]
    public async Task HydrateRoomIfNeededAsync_DeduplicatesRepeatedNodeIds_InDeltaSet()
    {
        var roomId = "room-dedupe";
        var now = DateTimeOffset.UtcNow;

        var snapshotPayload = new byte[] { 41, 42, 43 };
        var duplicatePayload = new byte[] { 44, 45, 46 };

        var store = new TestNodeStoreProvider
        {
            CompactionSnapshot = new CompactionSnapshot(
                roomId,
                snapshotPayload,
                now.AddMinutes(-5),
                BoundaryNodeIdHex: "pack:boundary"
            ),
            RoomSnapshot = new NodeSnapshot(
                roomId,
                new[]
                {
                    new AcceptedNodeRecord(
                        "pack:boundary",
                        new byte[] { 1, 2, 3 },
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-6)
                    ),
                    new AcceptedNodeRecord(
                        "pack:dup",
                        duplicatePayload,
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-4)
                    ),
                    new AcceptedNodeRecord(
                        "pack:dup",
                        duplicatePayload,
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-3)
                    )
                }
            )
        };

        var bridge = new RecordingRuntimeCommandBridge();
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        using var metrics = new RuntimeDagMeterCapture();

        await service.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);

        Assert.Equal(2, bridge.ImportPayloads.Count);
        Assert.Equal(snapshotPayload, bridge.ImportPayloads[0]);
        Assert.Equal(duplicatePayload, bridge.ImportPayloads[1]);
        Assert.Equal(0, metrics.GetTotal("room_recovery_semantic_mutations_total"));
        Assert.Equal(1, metrics.GetTotal("room_recovery_idempotent_replay_total"));
        Assert.Equal(1, metrics.GetTotal("room_recovery_signal_class_total"));
    }

    [Fact]
    public async Task HydrateRoomIfNeededAsync_EmitsSemanticChurnMetrics_WhenStateChanges()
    {
        var roomId = "room-semantic-churn";
        var now = DateTimeOffset.UtcNow;

        var store = new TestNodeStoreProvider
        {
            RoomSnapshot = new NodeSnapshot(
                roomId,
                new[]
                {
                    new AcceptedNodeRecord(
                        "pack:semantic-1",
                        new byte[] { 51, 52, 53 },
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-1)
                    )
                }
            )
        };

        var preStatePayload = JsonSerializer.SerializeToUtf8Bytes(new Dictionary<string, object>
        {
            ["boards/b1"] = new { name = "Old" }
        });
        var postStatePayload = JsonSerializer.SerializeToUtf8Bytes(new Dictionary<string, object>
        {
            ["boards/b1"] = new { name = "New" },
            ["buttons/btn-1"] = new { label = "Play" }
        });

        var bridge = new RecordingRuntimeCommandBridge
        {
            ServerPackPayloadSequence = new Queue<byte[]>(new[]
            {
                preStatePayload,
                postStatePayload
            })
        };
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        using var metrics = new RuntimeDagMeterCapture();
        await service.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);

        Assert.Equal(2, metrics.GetTotal("room_recovery_semantic_mutations_total"));
        Assert.Equal(0, metrics.GetTotal("room_recovery_idempotent_replay_total"));
        Assert.Equal(1, metrics.GetTotal("room_recovery_signal_class_total"));
    }

    [Fact]
    public async Task PersistInboundPackAsync_CreatesCompactionSnapshot_WhenEligibilityThresholdReached()
    {
        var roomId = "room-c";
        var store = new TestNodeStoreProvider();
        var bridge = new RecordingRuntimeCommandBridge();
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        for (var i = 0; i < 32; i += 1)
        {
            var payload = new byte[] { (byte)(i + 1), (byte)(i + 2), (byte)(i + 3) };
            var payloadB64 = Convert.ToBase64String(payload);
            await service.PersistInboundPackAsync(roomId, payloadB64, CancellationToken.None);
        }

        Assert.NotNull(store.CompactionSnapshot);
        Assert.Equal(roomId, store.CompactionSnapshot!.RoomId);
        Assert.Equal(2, store.CompactionSnapshot.SchemaVersion);
        Assert.False(string.IsNullOrWhiteSpace(store.CompactionSnapshot.BoundaryNodeIdHex));

        // Conservative Slice C default: prune disabled until full native snapshot materialization is in place.
        Assert.Empty(store.DeletedNodeIds);
    }

    [Fact]
    public async Task PersistInboundPackAsync_PrunesEligibleNodes_WhenPruningEnabled()
    {
        var roomId = "room-d";
        var store = new TestNodeStoreProvider();
        var bridge = new RecordingRuntimeCommandBridge();
        var options = new RuntimeDagCompactionOptions(
            Enabled: true,
            MinEligibleNodes: 4,
            EnablePruning: true
        );
        var service = new RuntimeDagPersistenceService(
            store,
            bridge,
            NullLogger<RuntimeDagPersistenceService>.Instance,
            options
        );

        for (var i = 0; i < 4; i += 1)
        {
            var payload = new byte[] { (byte)(31 + i), (byte)(41 + i), (byte)(51 + i) };
            var payloadB64 = Convert.ToBase64String(payload);
            await service.PersistInboundPackAsync(roomId, payloadB64, CancellationToken.None);
        }

        Assert.NotNull(store.CompactionSnapshot);
        Assert.NotEmpty(store.DeletedNodeIds);
    }

    [Fact]
    public async Task PersistInboundPackAsync_PruningNeverDeletesTombstoneRecords()
    {
        var roomId = "room-tombstone-policy";
        var now = DateTimeOffset.UtcNow;
        var store = new TestNodeStoreProvider
        {
            RoomSnapshot = new NodeSnapshot(
                roomId,
                new[]
                {
                    new AcceptedNodeRecord(
                        "pack:tombstone",
                        new byte[] { 1, 1, 1 },
                        AcceptedNodeKinds.Pack,
                        IsTombstone: true,
                        AcceptedAtUtc: now.AddMinutes(-10),
                        EligibleForCompactionAtUtc: now.AddMinutes(-9)
                    ),
                    new AcceptedNodeRecord(
                        "pack:a",
                        new byte[] { 2, 2, 2 },
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-8),
                        EligibleForCompactionAtUtc: now.AddMinutes(-7)
                    ),
                    new AcceptedNodeRecord(
                        "pack:b",
                        new byte[] { 3, 3, 3 },
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-6),
                        EligibleForCompactionAtUtc: now.AddMinutes(-5)
                    ),
                    new AcceptedNodeRecord(
                        "pack:c",
                        new byte[] { 4, 4, 4 },
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-4),
                        EligibleForCompactionAtUtc: now.AddMinutes(-3)
                    ),
                    new AcceptedNodeRecord(
                        "pack:d",
                        new byte[] { 5, 5, 5 },
                        AcceptedNodeKinds.Pack,
                        AcceptedAtUtc: now.AddMinutes(-2),
                        EligibleForCompactionAtUtc: now.AddMinutes(-1)
                    )
                }
            )
        };

        var bridge = new RecordingRuntimeCommandBridge();
        var options = new RuntimeDagCompactionOptions(
            Enabled: true,
            MinEligibleNodes: 4,
            EnablePruning: true,
            RetentionWindow: TimeSpan.Zero
        );
        var service = new RuntimeDagPersistenceService(
            store,
            bridge,
            NullLogger<RuntimeDagPersistenceService>.Instance,
            options
        );

        await service.PersistInboundPackAsync(roomId, Convert.ToBase64String(new byte[] { 7, 8, 9 }), CancellationToken.None);

        Assert.DoesNotContain("pack:tombstone", store.DeletedNodeIds);
        Assert.NotEmpty(store.DeletedNodeIds);
    }

    [Fact]
    public async Task PersistInboundPackAsync_CompactionUsesServerPackPayload_WhenAvailable()
    {
        var roomId = "room-e";
        var store = new TestNodeStoreProvider();
        var bridge = new RecordingRuntimeCommandBridge
        {
            ServerPackPayload = new byte[] { 200, 201, 202, 203 },
            ServerPackRootHex = "deadbeef"
        };
        var options = new RuntimeDagCompactionOptions(
            Enabled: true,
            MinEligibleNodes: 4,
            EnablePruning: false
        );
        var service = new RuntimeDagPersistenceService(
            store,
            bridge,
            NullLogger<RuntimeDagPersistenceService>.Instance,
            options
        );

        for (var i = 0; i < 4; i += 1)
        {
            var payload = new byte[] { (byte)(61 + i), (byte)(71 + i), (byte)(81 + i) };
            await service.PersistInboundPackAsync(roomId, Convert.ToBase64String(payload), CancellationToken.None);
        }

        Assert.NotNull(store.CompactionSnapshot);
        Assert.Equal(new byte[] { 200, 201, 202, 203 }, store.CompactionSnapshot!.SnapshotPayload);
        Assert.Equal("deadbeef", store.CompactionSnapshot.FrontierHashHex);
    }

    [Fact]
    public async Task PersistInboundPackAsync_SkipsDuplicatePackHashReplay()
    {
        var roomId = "room-replay";
        var store = new TestNodeStoreProvider();
        var bridge = new RecordingRuntimeCommandBridge();
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        using var metrics = new RuntimeDagMeterCapture();

        var payload = new byte[] { 91, 92, 93, 94 };
        var nodesB64 = Convert.ToBase64String(payload);

        await service.PersistInboundPackAsync(roomId, nodesB64, CancellationToken.None);
        await service.PersistInboundPackAsync(roomId, nodesB64, CancellationToken.None);

        Assert.Equal(1, store.PersistAcceptedCalls);
        Assert.Equal(1, metrics.GetTotal("room_duplicate_pack_replay_suppressed_total"));
    }

    [Fact]
    public async Task PersistRoomSnapshotAsync_PersistsServerPackPayload()
    {
        var roomId = "room-snapshot";
        var store = new TestNodeStoreProvider();
        var bridge = new RecordingRuntimeCommandBridge
        {
            ServerPackPayload = new byte[] { 101, 102, 103, 104 }
        };
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        await service.PersistRoomSnapshotAsync(roomId, CancellationToken.None);

        Assert.Equal(1, store.PersistAcceptedCalls);
        var snapshot = await store.LoadRoomSnapshotAsync(roomId, CancellationToken.None);
        Assert.NotNull(snapshot);
        Assert.Single(snapshot!.Nodes);
    }

    [Fact]
    public async Task PersistRoomSnapshotAsync_SkipsDuplicateServerPackPayload()
    {
        var roomId = "room-snapshot-replay";
        var store = new TestNodeStoreProvider();
        var bridge = new RecordingRuntimeCommandBridge
        {
            ServerPackPayload = new byte[] { 121, 122, 123, 124 }
        };
        var service = new RuntimeDagPersistenceService(store, bridge, NullLogger<RuntimeDagPersistenceService>.Instance);

        using var metrics = new RuntimeDagMeterCapture();

        await service.PersistRoomSnapshotAsync(roomId, CancellationToken.None);
        await service.PersistRoomSnapshotAsync(roomId, CancellationToken.None);

        Assert.Equal(1, store.PersistAcceptedCalls);
        Assert.Equal(1, metrics.GetTotal("room_duplicate_pack_replay_suppressed_total"));
    }

    private sealed class TestNodeStoreProvider : INodeStoreProvider
    {
        private readonly List<AcceptedNodeRecord> _nodes = new();

        public int PersistAcceptedCalls { get; private set; }

        public NodeSnapshot? RoomSnapshot { get; set; }

        public CompactionSnapshot? CompactionSnapshot { get; set; }

        public List<string> DeletedNodeIds { get; } = new();

        public ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
        {
            if (RoomSnapshot is null)
            {
                return ValueTask.FromResult<NodeSnapshot?>(new NodeSnapshot(roomId, _nodes.ToArray()));
            }

            return ValueTask.FromResult<NodeSnapshot?>(RoomSnapshot);
        }

        public ValueTask<CompactionSnapshot?> LoadCompactionSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
        {
            return ValueTask.FromResult(CompactionSnapshot);
        }

        public ValueTask PersistAcceptedNodesAsync(string roomId, IReadOnlyList<AcceptedNodeRecord> nodes, CancellationToken cancellationToken = default)
        {
            PersistAcceptedCalls += 1;
            foreach (var node in nodes)
            {
                _nodes.RemoveAll(existing => string.Equals(existing.NodeIdHex, node.NodeIdHex, StringComparison.Ordinal));
                _nodes.Add(node with
                {
                    EligibleForCompactionAtUtc = DateTimeOffset.UtcNow.AddMinutes(-1),
                    AcceptedAtUtc = node.AcceptedAtUtc ?? DateTimeOffset.UtcNow.AddMinutes(-2)
                });
            }

            return ValueTask.CompletedTask;
        }

        public ValueTask DeleteAcceptedNodesAsync(string roomId, IReadOnlyList<string> nodeIdHexes, CancellationToken cancellationToken = default)
        {
            DeletedNodeIds.AddRange(nodeIdHexes);
            _nodes.RemoveAll(node => nodeIdHexes.Contains(node.NodeIdHex, StringComparer.Ordinal));
            return ValueTask.CompletedTask;
        }

        public ValueTask PersistCompactionSnapshotAsync(string roomId, CompactionSnapshot snapshot, CancellationToken cancellationToken = default)
        {
            CompactionSnapshot = snapshot;
            return ValueTask.CompletedTask;
        }
    }

    private sealed class RecordingRuntimeCommandBridge : IRuntimeCommandBridge
    {
        public byte[]? FailingImportPayload { get; set; }

        public byte[]? ServerPackPayload { get; set; }

        public string? ServerPackRootHex { get; set; }

        public Queue<byte[]>? ServerPackPayloadSequence { get; set; }

        public AsStatus EnsureRoomStatus { get; private set; } = AsStatus.Internal;

        public List<byte[]> ImportPayloads { get; } = new();

        public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
        {
            using var doc = JsonDocument.Parse(commandJson);
            if (IsEnsureRoom(doc.RootElement))
            {
                EnsureRoomStatus = AsStatus.Ok;
                return FfiJsonBridgeResult.Success("[]");
            }

            if (IsRequestServerPack(doc.RootElement))
            {
                byte[]? payload = null;
                string? rootHex = ServerPackRootHex;

                if (ServerPackPayloadSequence is not null && ServerPackPayloadSequence.Count > 0)
                {
                    payload = ServerPackPayloadSequence.Dequeue();
                    rootHex = Convert.ToHexStringLower(SHA256.HashData(payload));
                }
                else if (ServerPackPayload is not null)
                {
                    payload = ServerPackPayload;
                }

                if (payload is null)
                {
                    return FfiJsonBridgeResult.Success("[]");
                }

                var nodesB64 = Convert.ToBase64String(payload);
                var eventsJson =
                    "[{\"ServerPackPrepared\":{\"room_id\":\"room-test\",\"nodes_b64\":\""
                    + nodesB64
                    + "\",\"root_hex\":\""
                    + (rootHex ?? ServerPackRootHex ?? "")
                    + "\"}}]";
                return FfiJsonBridgeResult.Success(eventsJson);
            }

            var importPayload = ReadImportPayload(doc.RootElement);
            if (importPayload is null)
            {
                return FfiJsonBridgeResult.Failure(AsStatus.InvalidArg);
            }

            if (FailingImportPayload is not null && importPayload.SequenceEqual(FailingImportPayload))
            {
                return FfiJsonBridgeResult.Failure(AsStatus.Internal);
            }

            ImportPayloads.Add(importPayload);
            return FfiJsonBridgeResult.Success("[]");
        }

        private static bool IsEnsureRoom(JsonElement root)
        {
            return root.TryGetProperty("command", out var command)
                && command.ValueKind == JsonValueKind.String
                && string.Equals(command.GetString(), "EnsureRoom", StringComparison.Ordinal);
        }

        private static bool IsRequestServerPack(JsonElement root)
        {
            return root.TryGetProperty("command", out var command)
                && command.ValueKind == JsonValueKind.Object
                && command.TryGetProperty("RequestServerPack", out var request)
                && request.ValueKind == JsonValueKind.Object;
        }

        private static byte[]? ReadImportPayload(JsonElement root)
        {
            if (!root.TryGetProperty("command", out var command)
                || command.ValueKind != JsonValueKind.Object
                || !command.TryGetProperty("ImportPack", out var importPack)
                || importPack.ValueKind != JsonValueKind.Object
                || !importPack.TryGetProperty("nodes_b64", out var nodesB64)
                || nodesB64.ValueKind != JsonValueKind.String)
            {
                return null;
            }

            var encoded = nodesB64.GetString();
            if (string.IsNullOrWhiteSpace(encoded))
            {
                return null;
            }

            return Convert.FromBase64String(encoded);
        }
    }

    private sealed class RuntimeDagMeterCapture : IDisposable
    {
        private readonly MeterListener _listener;
        private readonly Dictionary<string, long> _totals = new(StringComparer.Ordinal);

        public RuntimeDagMeterCapture()
        {
            _listener = new MeterListener
            {
                InstrumentPublished = (instrument, listener) =>
                {
                    if (string.Equals(instrument.Meter.Name, "ActiveSync.DotNetHost.RuntimeDag", StringComparison.Ordinal))
                    {
                        listener.EnableMeasurementEvents(instrument);
                    }
                }
            };

            _listener.SetMeasurementEventCallback<long>((instrument, measurement, _, _) =>
            {
                if (_totals.TryGetValue(instrument.Name, out var total))
                {
                    _totals[instrument.Name] = total + measurement;
                }
                else
                {
                    _totals[instrument.Name] = measurement;
                }
            });

            _listener.Start();
        }

        public long GetTotal(string instrumentName)
        {
            return _totals.TryGetValue(instrumentName, out var total) ? total : 0;
        }

        public void Dispose()
        {
            _listener.Dispose();
        }
    }
}
