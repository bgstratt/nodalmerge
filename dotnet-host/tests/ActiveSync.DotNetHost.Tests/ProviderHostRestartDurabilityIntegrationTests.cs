using ActiveSync.DotNetHost;
using ActiveSync.Host.Abstractions.Providers;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;

namespace ActiveSync.DotNetHost.Tests;

public sealed class ProviderHostRestartDurabilityIntegrationTests
{
    [Fact]
    public async Task Host_restart_preserves_nodes_and_blobs_in_sqlite_file_profile()
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "activesync-host-restart-durability", Guid.NewGuid().ToString("N"));
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

    private static WebApplication BuildTestApp(string dbPath, string blobRoot)
    {
        return HostApplication.Build(
            [],
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: cfg =>
            {
                cfg.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["ActiveSync:Providers:NodeStorage"] = "Sqlite",
                        ["ActiveSync:Providers:BlobStorage"] = "File",
                        ["ActiveSync:Storage:Sqlite:DbPath"] = dbPath,
                        ["ActiveSync:Storage:FileBlobs:RootPath"] = blobRoot
                    }
                );
            }
        );
    }
}
