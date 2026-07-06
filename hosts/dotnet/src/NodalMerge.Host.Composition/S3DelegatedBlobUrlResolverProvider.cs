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
    private readonly object _circuitLock = new();

    private int _consecutiveFailures;
    private DateTimeOffset? _circuitOpenedUntil;

    public S3DelegatedBlobUrlResolverProvider(
        IHttpClientFactory httpClientFactory,
        S3DelegatedBlobOptions options,
        ILogger<S3DelegatedBlobUrlResolverProvider> logger
    )
    {
        _httpClientFactory = httpClientFactory;
        _options = options;
        _logger = logger;
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
        for (var attempt = 1; attempt <= maxAttempts; attempt++)
        {
            _logger.LogInformation(
                "S3 delegated resolver attempt {Attempt}/{MaxAttempts} to path {Path}",
                attempt,
                maxAttempts,
                path
            );

            var outcome = await TryPostOnceAsync(client, path, payload, cancellationToken);
            if (outcome.Response is not null)
            {
                RecordSuccess();
                _logger.LogInformation("S3 delegated resolver succeeded on attempt {Attempt}", attempt);
                return outcome.Response;
            }

            if (!outcome.IsTransient || attempt == maxAttempts)
            {
                RecordFailure();
                _logger.LogWarning(
                    "S3 delegated resolver failed on attempt {Attempt} (transient={IsTransient}); returning fallback",
                    attempt,
                    outcome.IsTransient
                );
                return null;
            }
        }

        RecordFailure();
        return null;
    }

    private static async Task<DelegatedPostOutcome> TryPostOnceAsync(
        HttpClient client,
        string path,
        object payload,
        CancellationToken cancellationToken
    )
    {
        HttpResponseMessage response;
        try
        {
            response = await client.PostAsJsonAsync(path, payload, cancellationToken);
        }
        catch (TaskCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            return new DelegatedPostOutcome(Response: null, IsTransient: true);
        }
        catch (HttpRequestException)
        {
            return new DelegatedPostOutcome(Response: null, IsTransient: true);
        }

        if (!response.IsSuccessStatusCode)
        {
            var statusCode = (int)response.StatusCode;
            var isTransient = statusCode >= 500 || statusCode == 429;
            return new DelegatedPostOutcome(Response: null, IsTransient: isTransient);
        }

        var parsed = await response.Content.ReadFromJsonAsync<DelegatedUrlResponse>(cancellationToken: cancellationToken);
        return new DelegatedPostOutcome(Response: parsed, IsTransient: false);
    }

    private bool IsCircuitOpen()
    {
        lock (_circuitLock)
        {
            if (_circuitOpenedUntil is null)
            {
                return false;
            }

            if (DateTimeOffset.UtcNow < _circuitOpenedUntil.Value)
            {
                return true;
            }

            _circuitOpenedUntil = null;
            _consecutiveFailures = 0;
            return false;
        }
    }

    private void RecordSuccess()
    {
        lock (_circuitLock)
        {
            _consecutiveFailures = 0;
            _circuitOpenedUntil = null;
        }
    }

    private void RecordFailure()
    {
        lock (_circuitLock)
        {
            _consecutiveFailures++;
            if (_consecutiveFailures >= _options.CircuitBreakerFailureThreshold)
            {
                _circuitOpenedUntil = DateTimeOffset.UtcNow.AddSeconds(_options.CircuitBreakerOpenSeconds);
            }
        }
    }
}

internal sealed record DelegatedPostOutcome(DelegatedUrlResponse? Response, bool IsTransient);

internal sealed class DelegatedUrlResponse
{
    [JsonPropertyName("url")]
    public string Url { get; set; } = string.Empty;
}
