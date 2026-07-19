using System.Diagnostics;
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

    /// <summary>Keys the pre-0.2.0 two-path delegate presign protocol used, removed by this record's migration.</summary>
    private static readonly string[] RemovedKeys = ["PutPath", "GetPath"];

    /// <summary>
    /// Slice 4.3 (blob-cas-remediation.md): <c>PutPath</c>/<c>GetPath</c> were
    /// removed by the presign-protocol-v1 migration this record's own doc
    /// comment describes — but the options binder
    /// (<see cref="FromConfiguration"/>) silently drops unknown keys, so old
    /// config still setting them was ignored with no signal beyond
    /// <see cref="PresignPath"/> quietly defaulting to
    /// <c>/v1/blobs/presign</c>, degrading a mis-migrated deployment to the WS
    /// blob fallback. Warns LOUDLY — names every stale key found and the
    /// migration to make — rather than hard-failing: plan principle 3 (don't
    /// brick a running upgrade over stale-but-harmless config). Must read the
    /// raw <see cref="IConfiguration"/> section directly, past the binder:
    /// by the time <see cref="Validate"/> runs on the bound
    /// <see cref="S3DelegatedBlobOptions"/>, these keys are already gone.
    /// </summary>
    public static void WarnOnStaleKeys(IConfiguration? configuration)
    {
        var section = configuration?.GetSection(SectionName);
        if (section is null)
        {
            return;
        }

        var stale = RemovedKeys.Where(key => !string.IsNullOrEmpty(section[key])).ToList();
        if (stale.Count == 0)
        {
            return;
        }

        // This host has no logging abstraction at composition time; Trace is
        // the ambient channel (matches CapabilityProfileExpander.ClampToCeiling's
        // precedent) so an operator who never migrated learns it from the log,
        // not from a silently-degraded deployment.
        Trace.TraceWarning(
            $"{SectionName} still sets removed key(s) [{string.Join(", ", stale)}] from the pre-0.2.0 " +
            "two-path delegate presign protocol; they are silently ignored by the options binder. " +
            $"Migrate to a single {SectionName}:PresignPath endpoint speaking the v1 presign protocol " +
            "(one endpoint, `op` in the request body, response `{\"url\": ...}` only — see this " +
            $"file's doc comment). Until migrated, {SectionName}:PresignPath defaults to " +
            "/v1/blobs/presign, and a deployment still pointed at the old two-path backend silently " +
            "degrades to the WS blob fallback."
        );
    }
}
