using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;

namespace NodalMerge.DotNetHost.Tests;

public sealed class ProviderDurabilityTests
{
    [Fact]
    public async Task SqliteFile_profile_supports_multi_device_blob_propagation_and_retrieval()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "activesync-provider-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");

        var configuration = new ConfigurationBuilder()
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

        const string hashA = "sha256:multi-device-a";
        const string hashB = "sha256:multi-device-b";
        var bytesA = new byte[] { 101, 102, 103 };
        var bytesB = new byte[] { 201, 202, 203, 204 };

        // Device A uploads the first blob.
        await using (var deviceA = BuildProvider(configuration))
        {
            var blobStore = deviceA.GetRequiredService<IBlobStoreProvider>();
            await blobStore.PutBlobAsync(hashA, bytesA, "application/octet-stream", CancellationToken.None);
        }

        // Device B reconnects, can read blob A, and uploads blob B.
        await using (var deviceB = BuildProvider(configuration))
        {
            var blobStore = deviceB.GetRequiredService<IBlobStoreProvider>();

            var readA = await blobStore.TryGetBlobAsync(hashA, CancellationToken.None);
            Assert.True(readA.Found);
            Assert.Equal(bytesA, readA.Bytes);

            await blobStore.PutBlobAsync(hashB, bytesB, "application/octet-stream", CancellationToken.None);
        }

        // Device C reconnects later and can read both blobs.
        await using (var deviceC = BuildProvider(configuration))
        {
            var blobStore = deviceC.GetRequiredService<IBlobStoreProvider>();

            var readA = await blobStore.TryGetBlobAsync(hashA, CancellationToken.None);
            var readB = await blobStore.TryGetBlobAsync(hashB, CancellationToken.None);

            Assert.True(readA.Found);
            Assert.True(readB.Found);
            Assert.Equal(bytesA, readA.Bytes);
            Assert.Equal(bytesB, readB.Bytes);
        }
    }

    [Fact]
    public async Task SqliteFile_profile_preserves_blob_retrieval_across_multiple_restart_reconnect_cycles()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "activesync-provider-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");

        var configuration = new ConfigurationBuilder()
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

        const string hash = "sha256:blob-reconnect";
        var expectedBytes = new byte[] { 21, 22, 23, 24, 25 };

        await using (var firstProvider = BuildProvider(configuration))
        {
            var blobStore = firstProvider.GetRequiredService<IBlobStoreProvider>();
            await blobStore.PutBlobAsync(hash, expectedBytes, "application/octet-stream", CancellationToken.None);
        }

        for (var cycle = 0; cycle < 3; cycle += 1)
        {
            await using var provider = BuildProvider(configuration);
            var blobStore = provider.GetRequiredService<IBlobStoreProvider>();
            var blob = await blobStore.TryGetBlobAsync(hash, CancellationToken.None);

            Assert.True(blob.Found);
            Assert.Equal(expectedBytes, blob.Bytes);
        }
    }

    [Fact]
    public async Task File_blob_provider_missing_result_is_deterministic_across_restarts()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "activesync-provider-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");

        var configuration = new ConfigurationBuilder()
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

        const string missingHash = "sha256:missing-blob";

        await using (var firstProvider = BuildProvider(configuration))
        {
            var blobStore = firstProvider.GetRequiredService<IBlobStoreProvider>();
            var missing = await blobStore.TryGetBlobAsync(missingHash, CancellationToken.None);

            Assert.False(missing.Found);
            Assert.Null(missing.Bytes);
            Assert.Null(missing.ContentType);
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var blobStore = secondProvider.GetRequiredService<IBlobStoreProvider>();
            var missing = await blobStore.TryGetBlobAsync(missingHash, CancellationToken.None);

            Assert.False(missing.Found);
            Assert.Null(missing.Bytes);
            Assert.Null(missing.ContentType);
        }
    }

    [Fact]
    public async Task SqliteFile_profile_persists_nodes_and_blobs_across_service_restarts()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "activesync-provider-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");

        var configuration = new ConfigurationBuilder()
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

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            var blobStore = firstProvider.GetRequiredService<IBlobStoreProvider>();

            await nodeStore.PersistAcceptedNodesAsync(
                "room-a",
                [new AcceptedNodeRecord("node-1", [1, 2, 3, 4])],
                CancellationToken.None
            );

            await blobStore.PutBlobAsync("sha256:abc", [8, 6, 7, 5, 3, 0, 9], "application/octet-stream", CancellationToken.None);
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var blobStore = secondProvider.GetRequiredService<IBlobStoreProvider>();

            var snapshot = await nodeStore.LoadRoomSnapshotAsync("room-a", CancellationToken.None);
            Assert.NotNull(snapshot);
            Assert.Single(snapshot!.Nodes);
            Assert.Equal("node-1", snapshot.Nodes[0].NodeIdHex);
            Assert.Equal([1, 2, 3, 4], snapshot.Nodes[0].Payload);

            var blob = await blobStore.TryGetBlobAsync("sha256:abc", CancellationToken.None);
            Assert.True(blob.Found);
            Assert.Equal([8, 6, 7, 5, 3, 0, 9], blob.Bytes);
        }
    }

    [Fact]
    public async Task SqliteFile_profile_persists_compaction_snapshot_metadata_across_service_restarts()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "activesync-provider-durability", Guid.NewGuid().ToString("N"));
        var dbPath = Path.Combine(tempRoot, "nodes.db");
        var blobRoot = Path.Combine(tempRoot, "blobs");

        var configuration = new ConfigurationBuilder()
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

        var expected = new CompactionSnapshot(
            RoomId: "room-a",
            SnapshotPayload: [9, 8, 7, 6],
            CreatedAtUtc: DateTimeOffset.UtcNow,
            BoundaryNodeIdHex: "pack:boundary-node",
            FrontierHashHex: "cafebabe",
            SchemaVersion: 2
        );

        await using (var firstProvider = BuildProvider(configuration))
        {
            var nodeStore = firstProvider.GetRequiredService<INodeStoreProvider>();
            await nodeStore.PersistCompactionSnapshotAsync("room-a", expected, CancellationToken.None);
        }

        await using (var secondProvider = BuildProvider(configuration))
        {
            var nodeStore = secondProvider.GetRequiredService<INodeStoreProvider>();
            var loaded = await nodeStore.LoadCompactionSnapshotAsync("room-a", CancellationToken.None);

            Assert.NotNull(loaded);
            Assert.Equal(expected.RoomId, loaded!.RoomId);
            Assert.Equal(expected.SnapshotPayload, loaded.SnapshotPayload);
            Assert.Equal(expected.BoundaryNodeIdHex, loaded.BoundaryNodeIdHex);
            Assert.Equal(expected.FrontierHashHex, loaded.FrontierHashHex);
            Assert.Equal(expected.SchemaVersion, loaded.SchemaVersion);
        }
    }

    private static ServiceProvider BuildProvider(IConfiguration configuration)
    {
        var services = new ServiceCollection();
        services.AddActiveSyncHostProviders(configuration);
        return services.BuildServiceProvider();
    }
}
