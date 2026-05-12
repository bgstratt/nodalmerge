using Microsoft.Extensions.Configuration;

namespace ActiveSync.Host.Composition;

public sealed record ActiveSyncHostProviderOptions(
    string NodeStorageProvider,
    string BlobStorageProvider,
    string AuthProvider
)
{
    public const string SectionName = "ActiveSync:Providers";

    public static ActiveSyncHostProviderOptions Defaults { get; } =
        new("InMemory", "WsOnly", "Default");

    public static ActiveSyncHostProviderOptions FromConfiguration(IConfiguration? configuration)
    {
        if (configuration is null)
        {
            return Defaults;
        }

        var section = configuration.GetSection(SectionName);
        if (!section.Exists())
        {
            return Defaults;
        }

        return new ActiveSyncHostProviderOptions(
            section["NodeStorage"] ?? Defaults.NodeStorageProvider,
            section["BlobStorage"] ?? Defaults.BlobStorageProvider,
            section["Auth"] ?? Defaults.AuthProvider
        );
    }
}
