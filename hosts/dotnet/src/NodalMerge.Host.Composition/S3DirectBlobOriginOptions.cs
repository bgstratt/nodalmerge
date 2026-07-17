using Microsoft.Extensions.Configuration;

namespace NodalMerge.Host.Composition;

/// <summary>
/// Config for <see cref="S3DirectBlobStoreProvider"/> — the s3-direct chain
/// link added in slice 4.3 (nodalmerge-studio/plans/cas-distribution-and-storage.md
/// Phase 4). Selected alongside <c>NodalMerge:Providers:BlobStorage = ChainedRemote</c>
/// via <see cref="Enabled"/>; when disabled (the default) the chain is
/// exactly what it was before this slice: <c>local -> server-relay</c>.
/// When enabled, the chain grows to <c>local -> server-relay -> s3-direct</c>.
///
/// Deliberately does NOT carry its own <c>BaseUrl</c>/<c>AuthToken</c>: the
/// blob URL-resolution endpoints (<c>GET /blobs/{hash}/url</c>,
/// <c>POST /blobs/{hash}/uploaded</c>, docs/BLOB_HTTP_SURFACE.md "Blob URL
/// resolution") are a sibling extension of the same server-relay origin
/// surface, not a separate deployment — so this reuses
/// <see cref="RemoteBlobOriginOptions.BaseUrl"/> and
/// <see cref="RemoteBlobOriginOptions.AuthToken"/> rather than duplicating
/// them under a second config key. The presigned bucket URLs themselves are
/// wherever the origin's presign backend points — never configured here,
/// since that's the whole point of presigning (the peer never needs bucket
/// credentials or even the bucket's hostname ahead of time).
/// </summary>
public sealed record S3DirectBlobOriginOptions(
    bool Enabled,
    int TimeoutSeconds,
    int MaxRetries,
    int CircuitBreakerFailureThreshold,
    int CircuitBreakerOpenSeconds,
    int CapabilityProbeCooldownSeconds,
    string Compression,
    int CompressionLevel,
    int CompressionMinBytes
)
{
    public const string SectionName = "NodalMerge:Storage:S3Direct";

    private static readonly HashSet<string> SupportedCompressionModes = ["Off", "Zstd"];

    public static S3DirectBlobOriginOptions FromConfiguration(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);

        var enabled = false;
        if (bool.TryParse(section?["Enabled"], out var parsedEnabled))
        {
            enabled = parsedEnabled;
        }

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

        // Only op=get's 501 is cached (see S3DirectBlobStoreProvider) — a
        // reference presign backend (nodalmerge-s3-blobs's direct-upload
        // threshold) can legitimately 501 op=put based on size alone, so
        // caching that would wrongly suppress later, larger puts.
        var capabilityProbeCooldownSeconds = 60;
        if (int.TryParse(section?["CapabilityProbeCooldownSeconds"], out var parsedCooldown) && parsedCooldown >= 0)
        {
            capabilityProbeCooldownSeconds = parsedCooldown;
        }

        // Default OFF — compression is explicitly opt-in (blob-cas-remediation
        // slice 3.3, finding #6). With Zstd on, the bucket object is a zstd
        // frame the web SDK's presigned-GET reader could only use where the
        // browser transparently decodes Content-Encoding: zstd (Chrome 123+/
        // FF 126+ yes; Safari and Node/undici no) — correctness must not
        // depend on which client fetches. The SDK readers can now decode zstd
        // themselves (slice 3.3 step 2), but opting in stays a deliberate,
        // per-deployment choice.
        var compression = section?["Compression"];
        compression = string.IsNullOrWhiteSpace(compression) ? "Off" : compression;
        if (!SupportedCompressionModes.Contains(compression))
        {
            throw new InvalidOperationException(
                $"{SectionName}:Compression must be one of [{string.Join(", ", SupportedCompressionModes)}] (got '{compression}')"
            );
        }

        var compressionLevel = 3;
        if (int.TryParse(section?["CompressionLevel"], out var parsedLevel))
        {
            compressionLevel = parsedLevel;
        }

        var compressionMinBytes = 4096;
        if (int.TryParse(section?["CompressionMinBytes"], out var parsedMinBytes))
        {
            compressionMinBytes = parsedMinBytes;
        }

        return new S3DirectBlobOriginOptions(
            Enabled: enabled,
            TimeoutSeconds: timeoutSeconds,
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitFailureThreshold,
            CircuitBreakerOpenSeconds: circuitOpenSeconds,
            CapabilityProbeCooldownSeconds: capabilityProbeCooldownSeconds,
            Compression: compression,
            CompressionLevel: compressionLevel,
            CompressionMinBytes: compressionMinBytes
        );
    }

    public void Validate()
    {
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

        if (CapabilityProbeCooldownSeconds < 0 || CapabilityProbeCooldownSeconds > 3600)
        {
            throw new InvalidOperationException(
                $"{SectionName}:CapabilityProbeCooldownSeconds must be between 0 and 3600"
            );
        }

        if (!SupportedCompressionModes.Contains(Compression))
        {
            throw new InvalidOperationException(
                $"{SectionName}:Compression must be one of [{string.Join(", ", SupportedCompressionModes)}] (got '{Compression}')"
            );
        }
    }
}
