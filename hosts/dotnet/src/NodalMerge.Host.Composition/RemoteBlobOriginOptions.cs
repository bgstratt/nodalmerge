using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Config for <see cref="HttpRemoteBlobStoreProvider"/>, the HTTP client side
/// of the frozen blob origin surface (docs/BLOB_HTTP_SURFACE.md). Selected by
/// <c>NodalMerge:Providers:BlobStorage = ChainedRemote</c>, alongside
/// <see cref="FileBlobStorageOptions"/> for the local cache half of the chain.
/// </summary>
public sealed record RemoteBlobOriginOptions(
    string BaseUrl,
    string? AuthToken,
    int TimeoutSeconds,
    int MaxRetries,
    int CircuitBreakerFailureThreshold,
    int CircuitBreakerOpenSeconds
)
{
    public const string SectionName = "NodalMerge:Storage:RemoteOrigin";

    public static RemoteBlobOriginOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);

        var timeoutSeconds = 30;
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

        return new RemoteBlobOriginOptions(
            BaseUrl: section?["BaseUrl"] ?? string.Empty,
            AuthToken: section?["AuthToken"],
            TimeoutSeconds: timeoutSeconds,
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
                $"{SectionName}:BaseUrl is required when BlobStorage provider is ChainedRemote"
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
