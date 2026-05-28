using NodalMerge.DotNetHost.Ffi;
using NodalMerge.Host.Abstractions.Providers;
using Microsoft.Extensions.Configuration;

namespace NodalMerge.DotNetHost.Runtime;

public static class ServiceCollectionExtensions
{
    public static IServiceCollection AddNodalMergeRuntimeCore(
        this IServiceCollection services,
        IConfiguration? configuration = null)
    {
        var compactionOptions = BuildCompactionOptions(configuration);
        var peerLocalOptions = BuildPeerLocalOptions(configuration);

        services.AddSingleton(peerLocalOptions);
        services.AddSingleton<RuntimePeerLocalPersistenceService>();

        services.AddSingleton<HostFfiClient>();
        services.AddSingleton<FfiBridgeProcessor>();
        services.AddSingleton<IFfiHttpBridge, FfiHttpBridge>();
        services.AddSingleton<IFfiBinaryBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        services.AddSingleton<FfiWebSocketLoopRunner>();
        services.AddSingleton<IRuntimeCommandBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        services.AddSingleton<RuntimeProtocolMapper>(_ =>
            new RuntimeProtocolMapper(
                GetConfigValue(configuration, "NodalMerge:Runtime:ServerPeerPubkeyHex")));
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

    private static RuntimePeerLocalPersistenceOptions BuildPeerLocalOptions(IConfiguration? configuration)
    {
        var section = GetConfigSection(configuration, RuntimePeerLocalPersistenceOptions.SectionName);
        if (section is null || !section.Exists())
        {
            return RuntimePeerLocalPersistenceOptions.Disabled;
        }

        return new RuntimePeerLocalPersistenceOptions
        {
            Enabled = section.GetValue<bool?>("Enabled") ?? false,
            Backend = section.GetValue<string>("Backend") ?? "memory",
            DataDir = section.GetValue<string>("DataDir")
        };
    }

    private static RuntimeDagCompactionOptions BuildCompactionOptions(IConfiguration? configuration)
    {
        var defaults = RuntimeDagCompactionOptions.Default;
        var section = GetConfigSection(configuration, "NodalMerge:Runtime:Dag:Compaction");

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

    private static IConfigurationSection? GetConfigSection(
        IConfiguration? configuration,
        string sectionName)
    {
        return configuration?.GetSection(sectionName);
    }

    private static string? GetConfigValue(
        IConfiguration? configuration,
        string key)
    {
        return configuration?[key];
    }
}
