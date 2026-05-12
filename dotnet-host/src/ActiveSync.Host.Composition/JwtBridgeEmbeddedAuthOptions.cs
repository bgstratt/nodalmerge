using Microsoft.Extensions.Configuration;

namespace ActiveSync.Host.Composition;

public sealed record JwtBridgeEmbeddedAuthOptions(string Issuer, string Audience, string SigningKey)
{
    public const string SectionName = "ActiveSync:Auth:JwtBridgeEmbedded";

    public static JwtBridgeEmbeddedAuthOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
        return new JwtBridgeEmbeddedAuthOptions(
            section?["Issuer"] ?? string.Empty,
            section?["Audience"] ?? string.Empty,
            section?["SigningKey"] ?? string.Empty
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
    }
}
