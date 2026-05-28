using NodalMerge.DotNetHost.Runtime;
using Microsoft.Extensions.Logging.Abstractions;

namespace NodalMerge.DotNetHost.Tests;

public class RuntimePeerLocalPersistenceServiceTests
{
    [Fact]
    public void Disabled_options_do_not_open_ffi_client()
    {
        using var service = new RuntimePeerLocalPersistenceService(
            RuntimePeerLocalPersistenceOptions.Disabled,
            NullLogger<RuntimePeerLocalPersistenceService>.Instance
        );

        Assert.False(service.IsEnabled);
    }

    [Fact]
    public async Task Disabled_service_noops_pack_and_flush()
    {
        using var service = new RuntimePeerLocalPersistenceService(
            RuntimePeerLocalPersistenceOptions.Disabled,
            NullLogger<RuntimePeerLocalPersistenceService>.Instance
        );

        await service.HydrateRoomIfNeededAsync("room", CancellationToken.None);
        await service.PersistInboundPackAsync("room", Convert.ToBase64String(new byte[] { 1, 2, 3 }), CancellationToken.None);
        await service.FlushRoomAsync("room", CancellationToken.None);
    }
}
