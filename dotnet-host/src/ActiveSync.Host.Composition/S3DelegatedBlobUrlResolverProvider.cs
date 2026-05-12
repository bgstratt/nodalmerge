using ActiveSync.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging;
using System.Net.Http.Json;
using System.Text.Json.Serialization;

namespace ActiveSync.Host.Composition;

internal sealed class S3DelegatedBlobUrlResolverProvider : IBlobUrlResolverProvider
{
    private const string HttpClientName = "ActiveSync.S3DelegatedBlobResolver";

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
        var payload = new
        {
            room = request.RoomId,
            @namespace = request.Namespace,
            hash = request.HashHex,
            size = request.SizeBytes,
            contentType = request.ContentType
        };

        var response = await TryPostWithPolicyAsync(client, _options.PutPath, payload, cancellationToken);
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
            room = request.RoomId,
            @namespace = request.Namespace,
            hash = request.HashHex
        };

        var response = await TryPostWithPolicyAsync(client, _options.GetPath, payload, cancellationToken);
        if (response is null)
        {
            _logger.LogWarning("S3 delegated GET returned no presigned URL");
            return null;
        }

        return ToPresignedUrl(response);
    }

    private static PresignedBlobUrl? ToPresignedUrl(DelegatedUrlResponse response)
    {
        if (string.IsNullOrWhiteSpace(response.Url))
        {
            return null;
        }

        var expiry = response.ExpiresAtEpochSeconds ?? response.ExpiresAtEpochSecondsAlt;
        var expiresAtUtc = response.ExpiresAtUtc ?? response.ExpiresAtUtcAlt;
        if (expiry is null && expiresAtUtc is not null)
        {
            expiry = new DateTimeOffset(expiresAtUtc.Value).ToUnixTimeSeconds();
        }

        if (expiry is null)
        {
            return null;
        }

        return new PresignedBlobUrl(response.Url, DateTimeOffset.FromUnixTimeSeconds(expiry.Value));
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

    [JsonPropertyName("expiresAt")]
    public DateTime? ExpiresAtUtc { get; set; }

    [JsonPropertyName("expires_at")]
    public DateTime? ExpiresAtUtcAlt { get; set; }

    [JsonPropertyName("expiresAtEpochSeconds")]
    public long? ExpiresAtEpochSeconds { get; set; }

    [JsonPropertyName("expires_at_epoch_seconds")]
    public long? ExpiresAtEpochSecondsAlt { get; set; }
}
