using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

public sealed record NodalMergeHostProviderOptions(
    string NodeStorageProvider,
    string BlobStorageProvider,
    string AuthProvider
)
{
    public const string SectionName = "NodalMerge:Providers";
    public const string LegacySectionName = "NodalMerge:Providers";

    public static NodalMergeHostProviderOptions Defaults { get; } =
        new("InMemory", "WsOnly", "Default");

    public static NodalMergeHostProviderOptions FromConfiguration(IConfiguration? configuration)
    {
        if (configuration is null)
        {
            return Defaults;
        }

        var section = ConfigurationKeyFallback.GetSection(configuration, SectionName, LegacySectionName);
        if (!section.Exists())
        {
            return Defaults;
        }

        return new NodalMergeHostProviderOptions(
            section["NodeStorage"] ?? Defaults.NodeStorageProvider,
            section["BlobStorage"] ?? Defaults.BlobStorageProvider,
            section["Auth"] ?? Defaults.AuthProvider
        );
    }
}
