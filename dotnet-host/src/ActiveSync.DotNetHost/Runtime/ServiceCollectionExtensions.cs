using ActiveSync.DotNetHost.Ffi;

namespace ActiveSync.DotNetHost.Runtime;

public static class ServiceCollectionExtensions
{
    public static IServiceCollection AddActiveSyncRuntimeCore(this IServiceCollection services)
    {
        services.AddSingleton<HostFfiClient>();
        services.AddSingleton<FfiBridgeProcessor>();
        services.AddSingleton<IFfiHttpBridge, FfiHttpBridge>();
        services.AddSingleton<IFfiBinaryBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        services.AddSingleton<FfiWebSocketLoopRunner>();
        services.AddSingleton<IRuntimeCommandBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        services.AddSingleton<RuntimeProtocolMapper>();
        services.AddSingleton<RuntimeSessionIdAllocator>();
        services.AddSingleton<RuntimeRoomBroker>();
        services.AddSingleton<RuntimeDagPersistenceService>();
        services.AddSingleton<RuntimeTokenValidationService>();
        services.AddSingleton<RuntimeMessageProcessor>();
        services.AddSingleton<RuntimeFrameProcessor>();
        services.AddSingleton<RuntimeWebSocketLoopRunner>();

        return services;
    }
}
