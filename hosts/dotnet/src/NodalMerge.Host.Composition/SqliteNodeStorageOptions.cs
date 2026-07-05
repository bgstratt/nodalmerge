using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

public sealed record SqliteNodeStorageOptions(string DbPath)
{
    public const string SectionName = "NodalMerge:Storage:Sqlite";
    public const string LegacySectionName = "NodalMerge:Storage:Sqlite";

    public static SqliteNodeStorageOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = ConfigurationKeyFallback.GetSection(configuration, SectionName, LegacySectionName);
        var dbPath = section?["DbPath"] ?? "data/nodalmerge-nodes.db";
        return new SqliteNodeStorageOptions(dbPath);
    }

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(DbPath))
        {
            throw new InvalidOperationException(
                $"{SectionName}:DbPath is required when NodeStorage provider is Sqlite"
            );
        }
    }
}
