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

    /// <summary>
    /// Resolves <see cref="BaseUrl"/> into the <see cref="Uri"/> used to build
    /// every request against the origin (<see cref="HttpRemoteBlobStoreProvider"/>
    /// and, for the URL-resolution/upload-confirm endpoints,
    /// <see cref="S3DirectBlobStoreProvider"/>), guaranteeing a trailing "/" on
    /// the path component.
    ///
    /// Slice 4.3 (blob-cas-remediation.md): a reverse-proxy path prefix in
    /// <c>BaseUrl</c> (e.g. <c>https://host/nodalmerge</c>) was previously
    /// discarded, because request paths were built root-relative
    /// (<c>/blobs/{hash}</c>) against an <c>HttpClient.BaseAddress</c> that
    /// itself never guaranteed a trailing slash. RFC 3986's relative-reference
    /// merge algorithm treats a base path with no trailing "/" as ending in a
    /// "file" segment that a relative reference REPLACES rather than extends —
    /// <c>new Uri(new Uri("https://h/nodalmerge"), "blobs/x")</c> resolves to
    /// <c>https://h/blobs/x</c>, silently dropping "nodalmerge" — so BOTH
    /// halves of the fix are required together: this method guarantees the
    /// base ends in "/", and callers must combine it with a RELATIVE path that
    /// has NO leading "/" (e.g. <c>new Uri(ResolveBaseUri(), $"blobs/{hash}")</c>,
    /// never <c>$"/blobs/{hash}"</c>). Building the absolute request Uri this
    /// way, rather than relying on <c>HttpClient.BaseAddress</c> + a relative
    /// <see cref="HttpRequestMessage"/> path, also makes request construction
    /// independent of how the caller's <c>HttpClient</c> happens to be
    /// configured.
    /// </summary>
    public Uri ResolveBaseUri()
    {
        var normalized = BaseUrl.EndsWith('/') ? BaseUrl : BaseUrl + "/";
        return new Uri(normalized, UriKind.Absolute);
    }
}
