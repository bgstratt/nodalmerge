using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Config for the delegate presign protocol v1 (see
/// docs/BLOB_STORAGE_LAYOUT.md §7): a single endpoint, operation in the
/// request body. Breaking change from 0.1.x, which posted to two separate
/// paths (<c>PutPath</c>/<c>GetPath</c>) with a different payload shape —
/// 0.2.x deployments must point <see cref="PresignPath"/> at an endpoint
/// that speaks v1.
/// </summary>
public sealed record S3DelegatedBlobOptions(
    string BaseUrl,
    int TimeoutSeconds,
    string PresignPath,
    string? ApiKey,
    string ApiKeyHeader,
    int MaxRetries,
    int CircuitBreakerFailureThreshold,
    int CircuitBreakerOpenSeconds,
    int DefaultTtlSeconds
)
{
    public const string SectionName = "NodalMerge:Storage:S3Delegated";

    public static S3DelegatedBlobOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);

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

        var defaultTtlSeconds = 900;
        if (int.TryParse(section?["DefaultTtlSeconds"], out var parsedTtl) && parsedTtl > 0)
        {
            defaultTtlSeconds = parsedTtl;
        }

        return new S3DelegatedBlobOptions(
            BaseUrl: section?["BaseUrl"] ?? string.Empty,
            TimeoutSeconds: timeoutSeconds,
            PresignPath: section?["PresignPath"] ?? "/v1/blobs/presign",
            ApiKey: section?["ApiKey"],
            ApiKeyHeader: section?["ApiKeyHeader"] ?? "X-Api-Key",
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitFailureThreshold,
            CircuitBreakerOpenSeconds: circuitOpenSeconds,
            DefaultTtlSeconds: defaultTtlSeconds
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

        if (string.IsNullOrWhiteSpace(PresignPath))
        {
            throw new InvalidOperationException(
                $"{SectionName}:PresignPath is required"
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

        if (DefaultTtlSeconds <= 0 || DefaultTtlSeconds > 3600)
        {
            throw new InvalidOperationException(
                $"{SectionName}:DefaultTtlSeconds must be between 1 and 3600"
            );
        }
    }
}
