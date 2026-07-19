using NodalMerge.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging;
using System.Net.Http.Json;
using System.Text.Json.Serialization;

namespace NodalMerge.Host.Composition;

internal sealed class S3DelegatedBlobUrlResolverProvider : IBlobUrlResolverProvider
{
    private const string HttpClientName = "NodalMerge.S3DelegatedBlobResolver";

    private readonly IHttpClientFactory _httpClientFactory;
    private readonly S3DelegatedBlobOptions _options;
    private readonly ILogger<S3DelegatedBlobUrlResolverProvider> _logger;
    private readonly RetryCircuitBreakerPolicy _retryPolicy;

    public S3DelegatedBlobUrlResolverProvider(
        IHttpClientFactory httpClientFactory,
        S3DelegatedBlobOptions options,
        ILogger<S3DelegatedBlobUrlResolverProvider> logger
    )
    {
        _httpClientFactory = httpClientFactory;
        _options = options;
        _logger = logger;
        // Slice 7.2: this provider's option set. No 501 carve-out (a 501 is
        // any other 5xx here), and — unlike its two siblings — the breaker
        // verdict for a NON-transient response is CallerDecides: a 4xx (and
        // even a 2xx whose JSON body parses to null) counts as a breaker
        // FAILURE here, because the verdict depends on the body, not just
        // the status. Exhaustion maps to null (WS fallback), never a throw.
        // See RetryCircuitBreakerPolicy's class doc for the deliberately-
        // preserved drift between the three option sets.
        _retryPolicy = new RetryCircuitBreakerPolicy(new RetryCircuitBreakerPolicyOptions(
            MaxRetries: options.MaxRetries,
            CircuitBreakerFailureThreshold: options.CircuitBreakerFailureThreshold,
            CircuitBreakerOpenSeconds: options.CircuitBreakerOpenSeconds,
            TreatNotImplementedAsCapabilityDeclined: false,
            NonTransientResponseHandling: NonTransientResponseHandling.CallerDecides
        ));
    }

    public async ValueTask<PresignedBlobUrl?> ResolvePutUrlAsync(
        BlobPutUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        if (IsCircuitOpen())
        {
            _logger.LogWarning("S3 delegated PUT short-circuited by open circuit");
            return null;
        }

        var client = _httpClientFactory.CreateClient(HttpClientName);
        // Delegate presign protocol v1 — see docs/BLOB_STORAGE_LAYOUT.md §7.
        // `room`/`namespace` are metadata only; they must never influence
        // the app's key derivation.
        var payload = new
        {
            op = "put",
            room = request.RoomId,
            hash = request.HashHex,
            algorithm = "blake3",
            size = request.SizeBytes,
            ttl_seconds = _options.DefaultTtlSeconds,
            content_type = request.ContentType,
            @namespace = request.Namespace
        };

        var response = await TryPostWithPolicyAsync(client, _options.PresignPath, payload, cancellationToken);
        if (response is null)
        {
            _logger.LogWarning("S3 delegated PUT returned no presigned URL");
            return null;
        }

        return ToPresignedUrl(response);
    }

    public async ValueTask<PresignedBlobUrl?> ResolveGetUrlAsync(
        BlobGetUrlRequest request,
        CancellationToken cancellationToken = default
    )
    {
        if (IsCircuitOpen())
        {
            _logger.LogWarning("S3 delegated GET short-circuited by open circuit");
            return null;
        }

        var client = _httpClientFactory.CreateClient(HttpClientName);
        var payload = new
        {
            op = "get",
            room = request.RoomId,
            hash = request.HashHex,
            algorithm = "blake3",
            ttl_seconds = _options.DefaultTtlSeconds,
            @namespace = request.Namespace
        };

        var response = await TryPostWithPolicyAsync(client, _options.PresignPath, payload, cancellationToken);
        if (response is null)
        {
            _logger.LogWarning("S3 delegated GET returned no presigned URL");
            return null;
        }

        return ToPresignedUrl(response);
    }

    /// <summary>
    /// Protocol v1 responses are <c>{"url": "..."}</c> only — no expiry
    /// field. The resolver computes its own expiry from the TTL it sent,
    /// matching the Rust delegate's <c>PresignedUrl::with_ttl</c>.
    /// </summary>
    private PresignedBlobUrl? ToPresignedUrl(DelegatedUrlResponse response)
    {
        if (string.IsNullOrWhiteSpace(response.Url))
        {
            return null;
        }

        return new PresignedBlobUrl(response.Url, DateTimeOffset.UtcNow.AddSeconds(_options.DefaultTtlSeconds));
    }

    private async Task<DelegatedUrlResponse?> TryPostWithPolicyAsync(
        HttpClient client,
        string path,
        object payload,
        CancellationToken cancellationToken
    )
    {
        var maxAttempts = _options.MaxRetries + 1;

        var outcome = await _retryPolicy.SendWithRetryAsync(
            async (attempt, ct) =>
            {
                _logger.LogInformation(
                    "S3 delegated resolver attempt {Attempt}/{MaxAttempts} to path {Path}",
                    attempt,
                    maxAttempts,
                    path
                );
                return await client.PostAsJsonAsync(path, payload, ct);
            },
            cancellationToken
        );

        if (outcome.Kind == RetrySendOutcomeKind.Exhausted)
        {
            // Breaker failure already recorded by the policy.
            _logger.LogWarning(
                "S3 delegated resolver failed on attempt {Attempt} (transient={IsTransient}); returning fallback",
                outcome.Attempts,
                true
            );
            return null;
        }

        // CallerDecides mode: the policy recorded nothing for this
        // non-transient response — the breaker verdict below depends on the
        // status AND the parsed body, exactly as the pre-7.2 copy behaved.
        var response = outcome.Response!;
        using (response)
        {
            if (!response.IsSuccessStatusCode)
            {
                _retryPolicy.RecordFailure();
                _logger.LogWarning(
                    "S3 delegated resolver failed on attempt {Attempt} (transient={IsTransient}); returning fallback",
                    outcome.Attempts,
                    false
                );
                return null;
            }

            var parsed = await response.Content.ReadFromJsonAsync<DelegatedUrlResponse>(cancellationToken: cancellationToken);
            if (parsed is null)
            {
                // A 200 whose JSON body is the literal `null`: counted as a
                // breaker failure (pinned pre-extraction behavior).
                _retryPolicy.RecordFailure();
                _logger.LogWarning(
                    "S3 delegated resolver failed on attempt {Attempt} (transient={IsTransient}); returning fallback",
                    outcome.Attempts,
                    false
                );
                return null;
            }

            _retryPolicy.RecordSuccess();
            _logger.LogInformation("S3 delegated resolver succeeded on attempt {Attempt}", outcome.Attempts);
            return parsed;
        }
    }

    private bool IsCircuitOpen() => _retryPolicy.IsCircuitOpen();
}

internal sealed class DelegatedUrlResponse
{
    [JsonPropertyName("url")]
    public string Url { get; set; } = string.Empty;
}
