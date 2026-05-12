using Microsoft.Extensions.Configuration;

namespace ActiveSync.Host.Composition;

public sealed record SqliteNodeStorageOptions(string DbPath)
{
    public const string SectionName = "ActiveSync:Storage:Sqlite";

    public static SqliteNodeStorageOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
        var dbPath = section?["DbPath"] ?? "data/activesync-nodes.db";
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
