using ActiveSync.DotNetHost.Ffi;
using ActiveSync.Host.Abstractions.Providers;
using Microsoft.Extensions.Configuration;

namespace ActiveSync.DotNetHost.Runtime;

public static class ServiceCollectionExtensions
{
    public static IServiceCollection AddActiveSyncRuntimeCore(
        this IServiceCollection services,
        IConfiguration? configuration = null)
    {
        var compactionOptions = BuildCompactionOptions(configuration);

        services.AddSingleton<HostFfiClient>();
        services.AddSingleton<FfiBridgeProcessor>();
        services.AddSingleton<IFfiHttpBridge, FfiHttpBridge>();
        services.AddSingleton<IFfiBinaryBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        services.AddSingleton<FfiWebSocketLoopRunner>();
        services.AddSingleton<IRuntimeCommandBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        services.AddSingleton<RuntimeProtocolMapper>(_ =>
            new RuntimeProtocolMapper(
                configuration?.GetValue<string>("ActiveSync:Runtime:ServerPeerPubkeyHex")));
        services.AddSingleton<RuntimeSessionIdAllocator>();
        services.AddSingleton<RuntimeRoomBroker>();
        services.AddSingleton(compactionOptions);
        services.AddSingleton<RuntimeDagPersistenceService>(sp =>
            new RuntimeDagPersistenceService(
                sp.GetRequiredService<INodeStoreProvider>(),
                sp.GetRequiredService<IRuntimeCommandBridge>(),
                sp.GetRequiredService<ILogger<RuntimeDagPersistenceService>>(),
                sp.GetRequiredService<RuntimeDagCompactionOptions>()
            ));
        services.AddSingleton<RuntimeTokenValidationService>();
        services.AddSingleton<RuntimeMessageProcessor>();
        services.AddSingleton<RuntimeFrameProcessor>();
        services.AddSingleton<RuntimeWebSocketLoopRunner>();

        return services;
    }

    private static RuntimeDagCompactionOptions BuildCompactionOptions(IConfiguration? configuration)
    {
        var defaults = RuntimeDagCompactionOptions.Default;
        var section = configuration?.GetSection("ActiveSync:Runtime:Dag:Compaction");

        if (section is null || !section.Exists())
        {
            return defaults;
        }

        var retentionSeconds = section.GetValue<int?>("RetentionSeconds");

        return new RuntimeDagCompactionOptions(
            Enabled: section.GetValue<bool?>("Enabled") ?? defaults.Enabled,
            MinEligibleNodes: section.GetValue<int?>("MinEligibleNodes") ?? defaults.MinEligibleNodes,
            EnablePruning: section.GetValue<bool?>("EnablePruning") ?? defaults.EnablePruning,
            RetentionWindow: retentionSeconds.HasValue ? TimeSpan.FromSeconds(retentionSeconds.Value) : defaults.RetentionWindow
        );
    }
}
