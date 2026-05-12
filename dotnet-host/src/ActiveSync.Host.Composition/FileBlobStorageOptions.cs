using Microsoft.Extensions.Configuration;

namespace ActiveSync.Host.Composition;

public sealed record FileBlobStorageOptions(string RootPath)
{
    public const string SectionName = "ActiveSync:Storage:FileBlobs";

    public static FileBlobStorageOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
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
