using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

public sealed record FileBlobStorageOptions(string RootPath)
{
    public const string SectionName = "NodalMerge:Storage:FileBlobs";
    public const string LegacySectionName = "NodalMerge:Storage:FileBlobs";

    public static FileBlobStorageOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = ConfigurationKeyFallback.GetSection(configuration, SectionName, LegacySectionName);
        var rootPath = section?["RootPath"] ?? "data/blobs";
        return new FileBlobStorageOptions(rootPath);
    }

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(RootPath))
        {
            throw new InvalidOperationException(
                $"{SectionName}:RootPath is required when BlobStorage provider is File"
            );
        }
    }
}
