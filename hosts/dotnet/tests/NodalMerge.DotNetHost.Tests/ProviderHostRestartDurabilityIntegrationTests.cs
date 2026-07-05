using NodalMerge.DotNetHost;
using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using System.Security.Cryptography;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Tests;

public sealed class ProviderHostRestartDurabilityIntegrationTests
{
    [Fact]
    public async Task Host_restart_preserves_nodes_and_blobs_in_sqlite_file_profile()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");

        await using (var app = BuildTestApp(dbPath, blobRoot))
        {
            await app.StartAsync();

            var nodeStore = app.Services.GetRequiredService<INodeStoreProvider>();
            var blobStore = app.Services.GetRequiredService<IBlobStoreProvider>();

            await nodeStore.PersistAcceptedNodesAsync(
                "room-a",
                [new AcceptedNodeRecord("node-a", [10, 20, 30])],
                CancellationToken.None
            );

            await blobStore.PutBlobAsync("sha256:room-a-blob", [1, 3, 3, 7], "application/octet-stream", CancellationToken.None);
        }

        await using (var app = BuildTestApp(dbPath, blobRoot))
        {
            await app.StartAsync();

            var nodeStore = app.Services.GetRequiredService<INodeStoreProvider>();
            var blobStore = app.Services.GetRequiredService<IBlobStoreProvider>();

            var snapshot = await nodeStore.LoadRoomSnapshotAsync("room-a", CancellationToken.None);
            Assert.NotNull(snapshot);
            Assert.Single(snapshot!.Nodes);
            Assert.Equal("node-a", snapshot.Nodes[0].NodeIdHex);
            Assert.Equal([10, 20, 30], snapshot.Nodes[0].Payload);

            var blob = await blobStore.TryGetBlobAsync("sha256:room-a-blob", CancellationToken.None);
            Assert.True(blob.Found);
            Assert.Equal([1, 3, 3, 7], blob.Bytes);
        }
    }

    [Fact]
    public async Task Restart_preserves_server_pack_root_hash_via_snapshot_hydration_path()
    {
        var roomId = "room-root";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? preRestartRoot = null;

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(
                    Enabled: true,
                    MinEligibleNodes: 4,
                    EnablePruning: false,
                    RetentionWindow: TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            for (var i = 0; i < 5; i += 1)
            {
                SubmitMapSet(bridge, roomId, "world", $"k{i}", JsonSerializer.SerializeToElement(new { v = i }));
                var pack = SubmitRequestServerPack(bridge, roomId);
                preRestartRoot = pack.RootHex;
                await dag.PersistInboundPackAsync(roomId, pack.NodesB64, CancellationToken.None);
            }

            var persistedSnapshot = await nodeStore.LoadCompactionSnapshotAsync(roomId, CancellationToken.None);
            Assert.NotNull(persistedSnapshot);
            Assert.False(string.IsNullOrWhiteSpace(persistedSnapshot!.BoundaryNodeIdHex));
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(
                    Enabled: true,
                    MinEligibleNodes: 4,
                    EnablePruning: false,
                    RetentionWindow: TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestartPack = SubmitRequestServerPack(bridge, roomId);

            Assert.False(string.IsNullOrWhiteSpace(preRestartRoot));
            Assert.Equal(preRestartRoot, postRestartPack.RootHex);
        }
    }

    [Fact]
    public async Task Pruning_enabled_compaction_preserves_convergence_after_restart()
    {
        var roomId = "room-prune";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? preRestartRoot = null;
        int persistedNodeCountAfterPrune = int.MaxValue;

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(
                    Enabled: true,
                    MinEligibleNodes: 4,
                    EnablePruning: true,
                    RetentionWindow: TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            for (var i = 0; i < 8; i += 1)
            {
                SubmitMapSet(bridge, roomId, "world", $"p{i}", JsonSerializer.SerializeToElement(new { v = i, kind = "prune" }));
                var pack = SubmitRequestServerPack(bridge, roomId);
                preRestartRoot = pack.RootHex;
                await dag.PersistInboundPackAsync(roomId, pack.NodesB64, CancellationToken.None);
            }

            var persisted = await nodeStore.LoadRoomSnapshotAsync(roomId, CancellationToken.None);
            Assert.NotNull(persisted);
            persistedNodeCountAfterPrune = persisted!.Nodes.Count;
            Assert.True(persistedNodeCountAfterPrune < 8);
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(
                    Enabled: true,
                    MinEligibleNodes: 4,
                    EnablePruning: true,
                    RetentionWindow: TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestartPack = SubmitRequestServerPack(bridge, roomId);

            Assert.False(string.IsNullOrWhiteSpace(preRestartRoot));
            Assert.Equal(preRestartRoot, postRestartPack.RootHex);
            Assert.True(persistedNodeCountAfterPrune >= 1);
        }
    }

    [Fact]
    public async Task Restart_preserves_entity_reconciliation_counts_for_board_button_position_text_list_namespaces()
    {
        var roomId = "room-reconcile-counts";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        Dictionary<string, int>? preRestartCounts = null;

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(
                    Enabled: true,
                    MinEligibleNodes: 4,
                    EnablePruning: false,
                    RetentionWindow: TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            SubmitMapSet(bridge, roomId, "boards", "board-1", JsonSerializer.SerializeToElement(new { name = "Main" }));
            SubmitMapSet(bridge, roomId, "boards", "board-2", JsonSerializer.SerializeToElement(new { name = "Secondary" }));
            SubmitMapSet(bridge, roomId, "buttons", "btn-1", JsonSerializer.SerializeToElement(new { label = "Play" }));
            SubmitMapSet(bridge, roomId, "buttons", "btn-2", JsonSerializer.SerializeToElement(new { label = "Stop" }));
            SubmitMapSet(bridge, roomId, "positions", "pos-1", JsonSerializer.SerializeToElement(new { x = 1, y = 2 }));
            SubmitMapSet(bridge, roomId, "text", "txt-1", JsonSerializer.SerializeToElement(new { value = "hello" }));
            SubmitMapSet(bridge, roomId, "list", "lst-1", JsonSerializer.SerializeToElement(new[] { "a", "b" }));

            var pack = SubmitRequestServerPack(bridge, roomId);
            preRestartCounts = ExtractEntityCountsFromPack(pack.NodesB64);

            await dag.PersistInboundPackAsync(roomId, pack.NodesB64, CancellationToken.None);
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(
                    Enabled: true,
                    MinEligibleNodes: 4,
                    EnablePruning: false,
                    RetentionWindow: TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestartPack = SubmitRequestServerPack(bridge, roomId);
            var postRestartCounts = ExtractEntityCountsFromPack(postRestartPack.NodesB64);

            Assert.NotNull(preRestartCounts);
            Assert.Equal(preRestartCounts!, postRestartCounts);
            Assert.Equal(2, postRestartCounts["boards"]);
            Assert.Equal(2, postRestartCounts["buttons"]);
            Assert.Equal(1, postRestartCounts["positions"]);
            Assert.Equal(1, postRestartCounts["text"]);
            Assert.Equal(1, postRestartCounts["list"]);
        }
    }

    [Fact]
    public async Task Warm_restart_cycles_preserve_root_hash_and_entity_counts()
    {
        var roomId = "room-warm-restart";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? rootAfterWrites = null;
        Dictionary<string, int>? expectedCounts = null;

        await using (var first = BuildProvider(configuration))
        {
            var nodeStore = first.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            SubmitMapSet(bridge, roomId, "boards", "b1", JsonSerializer.SerializeToElement(new { name = "board" }));
            SubmitMapSet(bridge, roomId, "buttons", "btn1", JsonSerializer.SerializeToElement(new { label = "play" }));
            SubmitMapSet(bridge, roomId, "positions", "pos1", JsonSerializer.SerializeToElement(new { x = 5, y = 6 }));

            var pack = SubmitRequestServerPack(bridge, roomId);
            rootAfterWrites = pack.RootHex;
            expectedCounts = ExtractEntityCountsFromPack(pack.NodesB64);
            await dag.PersistInboundPackAsync(roomId, pack.NodesB64, CancellationToken.None);
        }

        await using (var second = BuildProvider(configuration))
        {
            var nodeStore = second.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var pack = SubmitRequestServerPack(bridge, roomId);
            Assert.Equal(rootAfterWrites, pack.RootHex);
            Assert.Equal(expectedCounts, ExtractEntityCountsFromPack(pack.NodesB64));
        }

        await using (var third = BuildProvider(configuration))
        {
            var nodeStore = third.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var pack = SubmitRequestServerPack(bridge, roomId);
            Assert.Equal(rootAfterWrites, pack.RootHex);
            Assert.Equal(expectedCounts, ExtractEntityCountsFromPack(pack.NodesB64));
        }
    }

    [Fact]
    public async Task Restart_during_traffic_preserves_final_convergence()
    {
        var roomId = "room-restart-during-traffic";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? expectedRoot = null;
        Dictionary<string, int>? expectedCounts = null;

        await using (var first = BuildProvider(configuration))
        {
            var nodeStore = first.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            for (var i = 0; i < 4; i += 1)
            {
                SubmitMapSet(bridge, roomId, "boards", $"b{i}", JsonSerializer.SerializeToElement(new { name = $"board-{i}" }));
                var pack = SubmitRequestServerPack(bridge, roomId);
                await dag.PersistInboundPackAsync(roomId, pack.NodesB64, CancellationToken.None);
            }
        }

        await using (var second = BuildProvider(configuration))
        {
            var nodeStore = second.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            for (var i = 0; i < 4; i += 1)
            {
                SubmitMapSet(bridge, roomId, "buttons", $"btn{i}", JsonSerializer.SerializeToElement(new { label = $"btn-{i}" }));
                var pack = SubmitRequestServerPack(bridge, roomId);
                expectedRoot = pack.RootHex;
                expectedCounts = ExtractEntityCountsFromPack(pack.NodesB64);
                await dag.PersistInboundPackAsync(roomId, pack.NodesB64, CancellationToken.None);
            }
        }

        await using (var third = BuildProvider(configuration))
        {
            var nodeStore = third.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var pack = SubmitRequestServerPack(bridge, roomId);

            Assert.False(string.IsNullOrWhiteSpace(expectedRoot));
            Assert.Equal(expectedRoot, pack.RootHex);
            Assert.NotNull(expectedCounts);
            Assert.Equal(expectedCounts!, ExtractEntityCountsFromPack(pack.NodesB64));
        }
    }

    [Fact]
    public async Task Periodic_restart_soak_cycles_do_not_drift_root_or_reconciliation_counts()
    {
        var roomId = "room-restart-soak";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        var cycleCount = 6;
        string? expectedRoot = null;
        Dictionary<string, int>? expectedCounts = null;

        for (var cycle = 0; cycle < cycleCount; cycle += 1)
        {
            await using var provider = BuildProvider(configuration);
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var preMutationPack = SubmitRequestServerPack(bridge, roomId);
            var preMutationCounts = ExtractEntityCountsFromPack(preMutationPack.NodesB64);

            if (expectedRoot is not null)
            {
                Assert.Equal(expectedRoot, preMutationPack.RootHex);
                Assert.NotNull(expectedCounts);
                Assert.Equal(expectedCounts!, preMutationCounts);
            }

            SubmitMapSet(bridge, roomId, "boards", $"board-soak-{cycle}", JsonSerializer.SerializeToElement(new { name = $"Board {cycle}" }));
            SubmitMapSet(bridge, roomId, "buttons", $"button-soak-{cycle}", JsonSerializer.SerializeToElement(new { label = $"Button {cycle}" }));

            var postMutationPack = SubmitRequestServerPack(bridge, roomId);
            var postMutationCounts = ExtractEntityCountsFromPack(postMutationPack.NodesB64);

            expectedRoot = postMutationPack.RootHex;
            expectedCounts = new Dictionary<string, int>(StringComparer.Ordinal)
            {
                ["boards"] = cycle + 1,
                ["buttons"] = cycle + 1,
                ["positions"] = 0,
                ["text"] = 0,
                ["list"] = 0
            };

            Assert.Equal(expectedCounts, postMutationCounts);
            await dag.PersistInboundPackAsync(roomId, postMutationPack.NodesB64, CancellationToken.None);
        }

        await using (var finalProvider = BuildProvider(configuration))
        {
            var nodeStore = finalProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var finalPack = SubmitRequestServerPack(bridge, roomId);
            var finalCounts = ExtractEntityCountsFromPack(finalPack.NodesB64);

            Assert.False(string.IsNullOrWhiteSpace(expectedRoot));
            Assert.Equal(expectedRoot, finalPack.RootHex);
            Assert.NotNull(expectedCounts);
            Assert.Equal(expectedCounts!, finalCounts);
            Assert.Equal(cycleCount, finalCounts["boards"]);
            Assert.Equal(cycleCount, finalCounts["buttons"]);
        }
    }

    [Fact]
    public async Task Quiet_steady_state_restart_cycles_do_not_add_churn_or_state_drift()
    {
        var roomId = "room-quiet-steady-state";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? baselineRoot = null;
        Dictionary<string, int>? baselineCounts = null;

        await using (var seedProvider = BuildProvider(configuration))
        {
            var nodeStore = seedProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            SubmitMapSet(bridge, roomId, "boards", "board-quiet", JsonSerializer.SerializeToElement(new { name = "Quiet" }));
            SubmitMapSet(bridge, roomId, "buttons", "button-quiet", JsonSerializer.SerializeToElement(new { label = "Stable" }));

            var seedPack = SubmitRequestServerPack(bridge, roomId);
            baselineRoot = seedPack.RootHex;
            baselineCounts = ExtractEntityCountsFromPack(seedPack.NodesB64);
            await dag.PersistInboundPackAsync(roomId, seedPack.NodesB64, CancellationToken.None);
        }

        Assert.False(string.IsNullOrWhiteSpace(baselineRoot));
        Assert.NotNull(baselineCounts);

        const int restartCycles = 5;
        for (var cycle = 0; cycle < restartCycles; cycle += 1)
        {
            await using var provider = BuildProvider(configuration);
            var nodeStore = provider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var cyclePack = SubmitRequestServerPack(bridge, roomId);
            var cycleCounts = ExtractEntityCountsFromPack(cyclePack.NodesB64);

            Assert.Equal(baselineRoot, cyclePack.RootHex);
            Assert.Equal(baselineCounts!, cycleCounts);

            // Feed the same payload back in to simulate transport replay noise.
            await dag.PersistInboundPackAsync(roomId, cyclePack.NodesB64, CancellationToken.None);
        }
    }

    [Fact]
    public async Task Delete_remains_terminal_after_restart_and_hydration()
    {
        var roomId = "room-delete-restart";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? expectedRoot = null;
        Dictionary<string, int>? expectedCounts = null;

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            SubmitMapSet(bridge, roomId, "boards", "board-1", JsonSerializer.SerializeToElement(new { name = "Board" }));
            SubmitMapSet(bridge, roomId, "buttons", "button-1", JsonSerializer.SerializeToElement(new { label = "Play" }));

            var beforeDeletePack = SubmitRequestServerPack(bridge, roomId);
            await dag.PersistInboundPackAsync(roomId, beforeDeletePack.NodesB64, CancellationToken.None);

            SubmitMapDelete(bridge, roomId, "buttons", "button-1");
            var afterDeletePack = SubmitRequestServerPack(bridge, roomId);
            expectedRoot = afterDeletePack.RootHex;
            expectedCounts = ExtractEntityCountsFromPack(afterDeletePack.NodesB64);

            Assert.Equal(0, expectedCounts["buttons"]);
            await dag.PersistInboundPackAsync(roomId, afterDeletePack.NodesB64, CancellationToken.None);
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestartPack = SubmitRequestServerPack(bridge, roomId);
            var postRestartCounts = ExtractEntityCountsFromPack(postRestartPack.NodesB64);

            Assert.NotNull(expectedCounts);
            Assert.Equal(expectedRoot, postRestartPack.RootHex);
            Assert.Equal(expectedCounts!, postRestartCounts);
            Assert.Equal(0, postRestartCounts["buttons"]);
        }
    }

    [Fact]
    public async Task Delete_state_survives_pruning_compaction_without_resurrection()
    {
        var roomId = "room-delete-prune";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string? expectedRoot = null;
        Dictionary<string, int>? expectedCounts = null;

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            SubmitMapSet(bridge, roomId, "boards", "board-1", JsonSerializer.SerializeToElement(new { name = "Board" }));
            SubmitMapSet(bridge, roomId, "buttons", "button-1", JsonSerializer.SerializeToElement(new { label = "Play" }));

            var createdPack = SubmitRequestServerPack(bridge, roomId);
            await dag.PersistInboundPackAsync(roomId, createdPack.NodesB64, CancellationToken.None);

            SubmitMapDelete(bridge, roomId, "buttons", "button-1");
            var deletedPack = SubmitRequestServerPack(bridge, roomId);
            await dag.PersistInboundPackAsync(roomId, deletedPack.NodesB64, CancellationToken.None);

            for (var i = 0; i < 4; i += 1)
            {
                SubmitMapSet(bridge, roomId, "text", $"keep-{i}", JsonSerializer.SerializeToElement(new { value = i }));
                var keepAlivePack = SubmitRequestServerPack(bridge, roomId);
                await dag.PersistInboundPackAsync(roomId, keepAlivePack.NodesB64, CancellationToken.None);
                expectedRoot = keepAlivePack.RootHex;
                expectedCounts = ExtractEntityCountsFromPack(keepAlivePack.NodesB64);
            }
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, true, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            var postRestartPack = SubmitRequestServerPack(bridge, roomId);
            var postRestartCounts = ExtractEntityCountsFromPack(postRestartPack.NodesB64);

            Assert.NotNull(expectedCounts);
            Assert.Equal(expectedRoot, postRestartPack.RootHex);
            Assert.Equal(expectedCounts!, postRestartCounts);
            Assert.Equal(0, postRestartCounts["buttons"]);
        }
    }

    [Fact]
    public async Task Offline_delete_reconnect_converges_without_resurrection()
    {
        var roomId = "room-offline-delete";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string stalePreDeleteNodesB64;
        string? expectedRoot = null;
        Dictionary<string, int>? expectedCounts = null;

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomId);
            SubmitMapSet(bridge, roomId, "boards", "board-1", JsonSerializer.SerializeToElement(new { name = "Main" }));
            SubmitMapSet(bridge, roomId, "buttons", "button-1", JsonSerializer.SerializeToElement(new { label = "DeleteMe" }));

            var preDeletePack = SubmitRequestServerPack(bridge, roomId);
            stalePreDeleteNodesB64 = preDeletePack.NodesB64;
            await dag.PersistInboundPackAsync(roomId, preDeletePack.NodesB64, CancellationToken.None);
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);
            SubmitMapDelete(bridge, roomId, "buttons", "button-1");

            var deletePack = SubmitRequestServerPack(bridge, roomId);
            expectedRoot = deletePack.RootHex;
            expectedCounts = ExtractEntityCountsFromPack(deletePack.NodesB64);
            Assert.Equal(0, expectedCounts["buttons"]);
            await dag.PersistInboundPackAsync(roomId, deletePack.NodesB64, CancellationToken.None);
        }

        await using (var thirdProvider = BuildProvider(configuration))
        {
            var nodeStore = thirdProvider.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomId, CancellationToken.None);

            // Simulate delayed stale replay from offline peer after reconnect.
            await dag.PersistInboundPackAsync(roomId, stalePreDeleteNodesB64, CancellationToken.None);

            var finalPack = SubmitRequestServerPack(bridge, roomId);
            var finalCounts = ExtractEntityCountsFromPack(finalPack.NodesB64);

            Assert.NotNull(expectedCounts);
            Assert.Equal(expectedRoot, finalPack.RootHex);
            Assert.Equal(expectedCounts!, finalCounts);
            Assert.Equal(0, finalCounts["buttons"]);
        }
    }

    [Fact]
    public async Task Cross_device_multi_room_delete_propagation_converges_without_resurrection()
    {
        var roomA = "room-multi-a";
        var roomB = "room-multi-b";
        var tempRoot = Path.Combine(Path.GetTempPath(), "nodalmerge-host-restart-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var configuration = BuildSqliteNodeOnlyConfiguration(dbPath);

        string staleRoomAPack;
        string staleRoomBPack;
        string? expectedRoomARoot = null;
        string? expectedRoomBRoot = null;
        Dictionary<string, int>? expectedRoomACounts = null;
        Dictionary<string, int>? expectedRoomBCounts = null;

        await using (var deviceA = BuildProvider(configuration))
        {
            var nodeStore = deviceA.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            SubmitEnsureRoom(bridge, roomA);
            SubmitMapSet(bridge, roomA, "boards", "board-a", JsonSerializer.SerializeToElement(new { name = "A" }));
            SubmitMapSet(bridge, roomA, "buttons", "button-a", JsonSerializer.SerializeToElement(new { label = "A" }));
            var roomAPreDelete = SubmitRequestServerPack(bridge, roomA);
            staleRoomAPack = roomAPreDelete.NodesB64;
            await dag.PersistInboundPackAsync(roomA, roomAPreDelete.NodesB64, CancellationToken.None);

            SubmitEnsureRoom(bridge, roomB);
            SubmitMapSet(bridge, roomB, "boards", "board-b", JsonSerializer.SerializeToElement(new { name = "B" }));
            SubmitMapSet(bridge, roomB, "buttons", "button-b", JsonSerializer.SerializeToElement(new { label = "B" }));
            var roomBPreDelete = SubmitRequestServerPack(bridge, roomB);
            staleRoomBPack = roomBPreDelete.NodesB64;
            await dag.PersistInboundPackAsync(roomB, roomBPreDelete.NodesB64, CancellationToken.None);
        }

        await using (var deviceB = BuildProvider(configuration))
        {
            var nodeStore = deviceB.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomA, CancellationToken.None);
            SubmitMapDelete(bridge, roomA, "buttons", "button-a");
            var roomADeletePack = SubmitRequestServerPack(bridge, roomA);
            expectedRoomARoot = roomADeletePack.RootHex;
            expectedRoomACounts = ExtractEntityCountsFromPack(roomADeletePack.NodesB64);
            await dag.PersistInboundPackAsync(roomA, roomADeletePack.NodesB64, CancellationToken.None);

            await dag.HydrateRoomIfNeededAsync(roomB, CancellationToken.None);
            SubmitMapDelete(bridge, roomB, "buttons", "button-b");
            var roomBDeletePack = SubmitRequestServerPack(bridge, roomB);
            expectedRoomBRoot = roomBDeletePack.RootHex;
            expectedRoomBCounts = ExtractEntityCountsFromPack(roomBDeletePack.NodesB64);
            await dag.PersistInboundPackAsync(roomB, roomBDeletePack.NodesB64, CancellationToken.None);
        }

        await using (var deviceC = BuildProvider(configuration))
        {
            var nodeStore = deviceC.GetRequiredService<INodeStoreProvider>();
            var bridge = new FakeRuntimeCommandBridge();
            var dag = new RuntimeDagPersistenceService(
                nodeStore,
                bridge,
                NullLogger<RuntimeDagPersistenceService>.Instance,
                new RuntimeDagCompactionOptions(true, 4, false, TimeSpan.Zero)
            );

            await dag.HydrateRoomIfNeededAsync(roomA, CancellationToken.None);
            await dag.HydrateRoomIfNeededAsync(roomB, CancellationToken.None);

            // Stale offline replay from earlier pre-delete packs must not resurrect either room.
            await dag.PersistInboundPackAsync(roomA, staleRoomAPack, CancellationToken.None);
            await dag.PersistInboundPackAsync(roomB, staleRoomBPack, CancellationToken.None);

            var roomAFinalPack = SubmitRequestServerPack(bridge, roomA);
            var roomBFinalPack = SubmitRequestServerPack(bridge, roomB);
            var roomAFinalCounts = ExtractEntityCountsFromPack(roomAFinalPack.NodesB64);
            var roomBFinalCounts = ExtractEntityCountsFromPack(roomBFinalPack.NodesB64);

            Assert.NotNull(expectedRoomACounts);
            Assert.NotNull(expectedRoomBCounts);
            Assert.Equal(expectedRoomARoot, roomAFinalPack.RootHex);
            Assert.Equal(expectedRoomBRoot, roomBFinalPack.RootHex);
            Assert.Equal(expectedRoomACounts!, roomAFinalCounts);
            Assert.Equal(expectedRoomBCounts!, roomBFinalCounts);
            Assert.Equal(0, roomAFinalCounts["buttons"]);
            Assert.Equal(0, roomBFinalCounts["buttons"]);
        }
    }

    private static IConfiguration BuildSqliteNodeOnlyConfiguration(string dbPath)
    {
        return new ConfigurationBuilder()
            .AddInMemoryCollection(
                new Dictionary<string, string?>
                {
                    ["NodalMerge:Providers:NodeStorage"] = "Sqlite",
                    ["NodalMerge:Providers:BlobStorage"] = "File",
                    ["NodalMerge:Storage:Sqlite:DbPath"] = dbPath,
                    ["NodalMerge:Storage:FileBlobs:RootPath"] = Path.Combine(Path.GetDirectoryName(dbPath)!, "blobs")
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

    private static WebApplication BuildTestApp(string dbPath, string blobRoot, bool enablePruning = false)
    {
        return HostApplication.Build(
            [],
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg =>
            {
                cfg.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["NodalMerge:Providers:NodeStorage"] = "Sqlite",
                        ["NodalMerge:Providers:BlobStorage"] = "File",
                        ["NodalMerge:Storage:Sqlite:DbPath"] = dbPath,
                        ["NodalMerge:Storage:FileBlobs:RootPath"] = blobRoot,
                        ["NodalMerge:Runtime:Dag:Compaction:Enabled"] = "true",
                        ["NodalMerge:Runtime:Dag:Compaction:MinEligibleNodes"] = "4",
                        ["NodalMerge:Runtime:Dag:Compaction:EnablePruning"] = enablePruning ? "true" : "false"
                    }
                );
            }
        );
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

    private static void SubmitMapDelete(
        IRuntimeCommandBridge bridge,
        string roomId,
        string @namespace,
        string key)
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

    private static Dictionary<string, int> ExtractEntityCountsFromPack(string nodesB64)
    {
        var payload = Convert.FromBase64String(nodesB64);
        var state = JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(payload)
            ?? new Dictionary<string, JsonElement>(StringComparer.Ordinal);

        var counts = new Dictionary<string, int>(StringComparer.Ordinal)
        {
            ["boards"] = 0,
            ["buttons"] = 0,
            ["positions"] = 0,
            ["text"] = 0,
            ["list"] = 0
        };

        foreach (var key in state.Keys)
        {
            if (key.StartsWith("boards/", StringComparison.Ordinal)) counts["boards"] += 1;
            else if (key.StartsWith("buttons/", StringComparison.Ordinal)) counts["buttons"] += 1;
            else if (key.StartsWith("positions/", StringComparison.Ordinal)) counts["positions"] += 1;
            else if (key.StartsWith("text/", StringComparison.Ordinal)) counts["text"] += 1;
            else if (key.StartsWith("list/", StringComparison.Ordinal)) counts["list"] += 1;
        }

        return counts;
    }

    private sealed class FakeRuntimeCommandBridge : IRuntimeCommandBridge
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
