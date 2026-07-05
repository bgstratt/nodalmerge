using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

public sealed record MongoNodeStorageOptions(string ConnectionString, string DatabaseName)
{
    public const string SectionName = "NodalMerge:Storage:Mongo";

    public static MongoNodeStorageOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
        return new MongoNodeStorageOptions(
            section?["ConnectionString"] ?? string.Empty,
            section?["DatabaseName"] ?? "nodalmerge"
        );
    }

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(ConnectionString))
        {
            throw new InvalidOperationException(
                $"{SectionName}:ConnectionString is required when NodeStorage provider is Mongo"
            );
        }

        if (string.IsNullOrWhiteSpace(DatabaseName))
        {
            throw new InvalidOperationException(
                $"{SectionName}:DatabaseName is required when NodeStorage provider is Mongo"
            );
        }
    }
}
