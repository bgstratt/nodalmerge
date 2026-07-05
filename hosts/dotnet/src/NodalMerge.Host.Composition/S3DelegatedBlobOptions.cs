using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

public sealed record S3DelegatedBlobOptions(
    string BaseUrl,
    int TimeoutSeconds,
    string PutPath,
    string GetPath,
    string? ApiKey,
    string ApiKeyHeader,
    int MaxRetries,
    int CircuitBreakerFailureThreshold,
    int CircuitBreakerOpenSeconds
)
{
    public const string SectionName = "NodalMerge:Storage:S3Delegated";
    public const string LegacySectionName = "NodalMerge:Storage:S3Delegated";

    public static S3DelegatedBlobOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = ConfigurationKeyFallback.GetSection(configuration, SectionName, LegacySectionName);

        var timeoutSeconds = 5;
        if (int.TryParse(section?["TimeoutSeconds"], out var parsedTimeout) && parsedTimeout > 0)
        {
            timeoutSeconds = parsedTimeout;
        }

        var maxRetries = 2;
        if (int.TryParse(section?["MaxRetries"], out var parsedMaxRetries) && parsedMaxRetries >= 0)
        {
            maxRetries = parsedMaxRetries;
        }

        var circuitFailureThreshold = 3;
        if (int.TryParse(section?["CircuitBreakerFailureThreshold"], out var parsedThreshold) && parsedThreshold > 0)
        {
            circuitFailureThreshold = parsedThreshold;
        }

        var circuitOpenSeconds = 30;
        if (int.TryParse(section?["CircuitBreakerOpenSeconds"], out var parsedOpenSeconds) && parsedOpenSeconds > 0)
        {
            circuitOpenSeconds = parsedOpenSeconds;
        }

        return new S3DelegatedBlobOptions(
            BaseUrl: section?["BaseUrl"] ?? string.Empty,
            TimeoutSeconds: timeoutSeconds,
            PutPath: section?["PutPath"] ?? "/v1/blobs/presign-put",
            GetPath: section?["GetPath"] ?? "/v1/blobs/presign-get",
            ApiKey: section?["ApiKey"],
            ApiKeyHeader: section?["ApiKeyHeader"] ?? "X-Api-Key",
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitFailureThreshold,
            CircuitBreakerOpenSeconds: circuitOpenSeconds
        );
    }

    public void Validate()
    {
        if (string.IsNullOrWhiteSpace(BaseUrl))
        {
            throw new InvalidOperationException(
                $"{SectionName}:BaseUrl is required when BlobStorage provider is S3Delegated"
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

        if (string.IsNullOrWhiteSpace(PutPath) || string.IsNullOrWhiteSpace(GetPath))
        {
            throw new InvalidOperationException(
                $"{SectionName}:PutPath and GetPath are required"
            );
        }

        if (string.IsNullOrWhiteSpace(ApiKeyHeader))
        {
            throw new InvalidOperationException(
                $"{SectionName}:ApiKeyHeader must not be empty"
            );
        }

        if (MaxRetries < 0 || MaxRetries > 10)
        {
            throw new InvalidOperationException(
                $"{SectionName}:MaxRetries must be between 0 and 10"
            );
        }

        if (CircuitBreakerFailureThreshold <= 0 || CircuitBreakerFailureThreshold > 100)
        {
            throw new InvalidOperationException(
                $"{SectionName}:CircuitBreakerFailureThreshold must be between 1 and 100"
            );
        }

        if (CircuitBreakerOpenSeconds <= 0 || CircuitBreakerOpenSeconds > 3600)
        {
            throw new InvalidOperationException(
                $"{SectionName}:CircuitBreakerOpenSeconds must be between 1 and 3600"
            );
        }
    }
}
