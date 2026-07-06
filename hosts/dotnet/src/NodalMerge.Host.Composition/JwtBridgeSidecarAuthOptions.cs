using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

public sealed record JwtBridgeSidecarAuthOptions(
    string BaseUrl,
    int TimeoutSeconds,
    string? ApiKey,
    string ApiKeyHeader
)
{
    public const string SectionName = "NodalMerge:Auth:JwtBridgeSidecar";

    public static JwtBridgeSidecarAuthOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);

        var timeoutSeconds = 5;
        if (int.TryParse(section?["TimeoutSeconds"], out var parsedTimeout) && parsedTimeout > 0)
        {
            timeoutSeconds = parsedTimeout;
        }

        return new JwtBridgeSidecarAuthOptions(
            BaseUrl: section?["BaseUrl"] ?? string.Empty,
            TimeoutSeconds: timeoutSeconds,
            ApiKey: section?["ApiKey"],
            ApiKeyHeader: section?["ApiKeyHeader"] ?? "X-Api-Key"
        );
    }

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(BaseUrl))
        {
            throw new InvalidOperationException(
                $"{SectionName}:BaseUrl is required when Auth provider is JwtBridgeSidecar"
            );
        }

        if (!Uri.TryCreate(BaseUrl, UriKind.Absolute, out _))
        {
            throw new InvalidOperationException(
                $"{SectionName}:BaseUrl must be an absolute URI"
            );
        }

        if (TimeoutSeconds <= 0 || TimeoutSeconds > 120)
        {
            throw new InvalidOperationException(
                $"{SectionName}:TimeoutSeconds must be between 1 and 120"
            );
        }

        if (string.IsNullOrWhiteSpace(ApiKeyHeader))
        {
            throw new InvalidOperationException(
                $"{SectionName}:ApiKeyHeader must not be empty"
            );
        }
    }
}
