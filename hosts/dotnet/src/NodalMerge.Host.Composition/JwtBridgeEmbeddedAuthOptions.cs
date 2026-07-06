using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

public sealed record JwtBridgeEmbeddedAuthOptions(
    string Issuer,
    string Audience,
    string SigningKey,
    int ClockSkewSeconds,
    IReadOnlyList<string> PreviousSigningKeys
)
{
    public const string SectionName = "NodalMerge:Auth:JwtBridgeEmbedded";

    public static JwtBridgeEmbeddedAuthOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);

        var clockSkewSeconds = 5;
        if (int.TryParse(section?["ClockSkewSeconds"], out var parsedClockSkew) && parsedClockSkew >= 0)
        {
            clockSkewSeconds = parsedClockSkew;
        }

        var previous = ParsePreviousSigningKeys(section);

        return new JwtBridgeEmbeddedAuthOptions(
            section?["Issuer"] ?? string.Empty,
            section?["Audience"] ?? string.Empty,
            section?["SigningKey"] ?? string.Empty,
            clockSkewSeconds,
            previous
        );
    }

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(Issuer))
        {
            throw new InvalidOperationException(
                $"{SectionName}:Issuer is required when Auth provider is JwtBridgeEmbedded"
            );
        }

        if (string.IsNullOrWhiteSpace(Audience))
        {
            throw new InvalidOperationException(
                $"{SectionName}:Audience is required when Auth provider is JwtBridgeEmbedded"
            );
        }

        if (string.IsNullOrWhiteSpace(SigningKey) || SigningKey.Length < 32)
        {
            throw new InvalidOperationException(
                $"{SectionName}:SigningKey is required and must be at least 32 characters when Auth provider is JwtBridgeEmbedded"
            );
        }

        if (ClockSkewSeconds < 0 || ClockSkewSeconds > 300)
        {
            throw new InvalidOperationException(
                $"{SectionName}:ClockSkewSeconds must be between 0 and 300 when Auth provider is JwtBridgeEmbedded"
            );
        }

        foreach (var previous in PreviousSigningKeys)
        {
            if (string.IsNullOrWhiteSpace(previous) || previous.Length < 32)
            {
                throw new InvalidOperationException(
                    $"{SectionName}:PreviousSigningKeys values must each be at least 32 characters when provided"
                );
            }
        }
    }

    private static IReadOnlyList<string> ParsePreviousSigningKeys(IConfigurationSection? section)
    {
        if (section is null)
        {
            return Array.Empty<string>();
        }

        var fromList = section.GetSection("PreviousSigningKeys")
            .GetChildren()
            .Select(child => child.Value)
            .Where(value => !string.IsNullOrWhiteSpace(value))
            .Cast<string>()
            .Distinct(StringComparer.Ordinal)
            .ToArray();

        if (fromList.Length > 0)
        {
            return fromList;
        }

        var flat = section["PreviousSigningKeys"];
        if (string.IsNullOrWhiteSpace(flat))
        {
            return Array.Empty<string>();
        }

        return flat
            .Split([';', ','], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
            .Distinct(StringComparer.Ordinal)
            .ToArray();
    }
}
