using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

/// <summary>
/// File-store configuration, including the optional v3 at-rest zstd
/// encoding (docs/BLOB_STORAGE_LAYOUT.md §8). <see cref="Compression"/>
/// defaults to <c>"Off"</c>, which reproduces today's behavior exactly (a
/// v2 store is a valid v3 store — see the layout doc's preamble).
/// </summary>
public sealed record FileBlobStorageOptions(
    string RootPath,
    string Compression = "Off",
    int CompressionLevel = 3,
    int CompressionMinBytes = 4096
)
{
    public const string SectionName = "NodalMerge:Storage:FileBlobs";

    private static readonly HashSet<string> SupportedCompressionModes = ["Off", "Zstd"];

    public static FileBlobStorageOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
        var rootPath = section?["RootPath"] ?? "data/blobs";

        var compression = section?["Compression"];
        compression = string.IsNullOrWhiteSpace(compression) ? "Off" : compression;
        if (!SupportedCompressionModes.Contains(compression))
        {
            throw new InvalidOperationException(
                $"{SectionName}:Compression must be one of [{string.Join(", ", SupportedCompressionModes)}] (got '{compression}')"
            );
        }

        var compressionLevel = 3;
        var compressionLevelRaw = section?["CompressionLevel"];
        if (!string.IsNullOrWhiteSpace(compressionLevelRaw) && int.TryParse(compressionLevelRaw, out var parsedLevel))
        {
            compressionLevel = parsedLevel;
        }

        var compressionMinBytes = 4096;
        var compressionMinBytesRaw = section?["CompressionMinBytes"];
        if (!string.IsNullOrWhiteSpace(compressionMinBytesRaw) && int.TryParse(compressionMinBytesRaw, out var parsedMinBytes))
        {
            compressionMinBytes = parsedMinBytes;
        }

        return new FileBlobStorageOptions(rootPath, compression, compressionLevel, compressionMinBytes);
    }

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(RootPath))
        {
            throw new InvalidOperationException(
                $"{SectionName}:RootPath is required when BlobStorage provider is File"
            );
        }

        if (!SupportedCompressionModes.Contains(Compression))
        {
            throw new InvalidOperationException(
                $"{SectionName}:Compression must be one of [{string.Join(", ", SupportedCompressionModes)}] (got '{Compression}')"
            );
        }
    }
}
