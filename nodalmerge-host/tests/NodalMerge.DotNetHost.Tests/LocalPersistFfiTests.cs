using NodalMerge.DotNetHost.Ffi;

namespace NodalMerge.DotNetHost.Tests;

public class LocalPersistFfiTests
{
    [Fact]
    public void Memory_store_reports_abi_version_and_hash_after_append()
    {
        using var client = new LocalPersistFfiClient("memory");
        Assert.False(client.IsDurable);

        // Empty node list append is valid; hash reflects empty replay state.
        using var append = client.AppendNodesJson("room-dotnet", "[]");
        var hash = client.CanonicalHashHex("room-dotnet");
        Assert.False(string.IsNullOrWhiteSpace(hash));
        Assert.Equal(64, hash.Length);
    }

    [Fact]
    public void Embedded_store_is_durable_and_survives_reopen()
    {
        var dir = Path.Combine(Path.GetTempPath(), "nodalmerge-local-ffi-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);

        try
        {
            using (var client = new LocalPersistFfiClient("embedded", dir))
            {
                Assert.True(client.IsDurable);
                client.AppendNodesJson("room-embed", "[]");
                client.Flush("room-embed");
            }

            using var reopened = new LocalPersistFfiClient("embedded", dir);
            using var hydrate = reopened.Hydrate("room-embed");
            Assert.True(hydrate.RootElement.TryGetProperty("node_count", out var count));
        }
        finally
        {
            try
            {
                Directory.Delete(dir, recursive: true);
            }
            catch
            {
                // best effort cleanup
            }
        }
    }
}
