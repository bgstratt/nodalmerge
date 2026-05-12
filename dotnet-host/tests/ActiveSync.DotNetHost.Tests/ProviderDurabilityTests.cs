using ActiveSync.Host.Abstractions.Providers;
using ActiveSync.Host.Composition;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;

namespace ActiveSync.DotNetHost.Tests;

public sealed class ProviderDurabilityTests
{
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
                    ["ActiveSync:Providers:NodeStorage"] = "Sqlite",
                    ["ActiveSync:Providers:BlobStorage"] = "File",
                    ["ActiveSync:Storage:Sqlite:DbPath"] = dbPath,
                    ["ActiveSync:Storage:FileBlobs:RootPath"] = blobRoot
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

    private static ServiceProvider BuildProvider(IConfiguration configuration)
    {
        var services = new ServiceCollection();
        services.AddActiveSyncHostProviders(configuration);
        return services.BuildServiceProvider();
    }
}
