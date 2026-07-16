using NodalMerge.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging;
using System.Net;
using System.Net.Http.Headers;

namespace NodalMerge.Host.Composition;

/// <summary>
/// HTTP client for the frozen blob origin surface (docs/BLOB_HTTP_SURFACE.md):
/// <c>GET/HEAD/PUT /blobs/{hash}</c>. Speaks the "client behavior" section of
/// that doc verbatim, with one deliberate exception — <see cref="TryGetBlobAsync"/>
/// does NOT verify the BLAKE3 hash of the fetched bytes. Verification lives in
/// exactly one place, <c>ChainedBlobStoreProvider</c>, which is the only
/// consumer that decides whether a fetched payload may be trusted and cached.
/// Any other caller of this provider gets the origin's bytes as-is.
///
/// Retry + circuit breaker policy mirrors <see cref="S3DelegatedBlobUrlResolverProvider"/>:
/// 5xx/429/timeouts are retryable (up to <see cref="RemoteBlobOriginOptions.MaxRetries"/>
/// extra attempts); 400/401/413/422 are not. A run of consecutive failures opens
/// the breaker for <see cref="RemoteBlobOriginOptions.CircuitBreakerOpenSeconds"/>.
///
/// Breaker-open behavior is intentionally asymmetric:
/// - <see cref="TryGetBlobAsync"/> treats an open breaker as a degraded miss
///   (logs and returns <see cref="BlobReadResult.Missing"/>) so a struggling
///   origin degrades to "nothing available from remote" rather than throwing —
///   the chain is local-first anyway, so a Missing here just means the local
///   cache is authoritative until the origin recovers.
/// - <see cref="PushAsync"/>/<see cref="ExistsAsync"/> throw when the breaker
///   is open, because silently swallowing a push means data loss the
///   reconcile sweep can't detect; the caller (chain, or the sweep itself)
///   decides how to handle that failure.
/// </summary>
public sealed class HttpRemoteBlobStoreProvider : IBlobStoreProvider, IRemoteBlobPushTarget
{
    public const string HttpClientName = "NodalMerge.RemoteBlobOrigin";

    private readonly IHttpClientFactory _httpClientFactory;
    private readonly RemoteBlobOriginOptions _options;
    private readonly ILogger<HttpRemoteBlobStoreProvider> _logger;
    private readonly object _circuitLock = new();

    private int _consecutiveFailures;
    private DateTimeOffset? _circuitOpenedUntil;

    public HttpRemoteBlobStoreProvider(
        IHttpClientFactory httpClientFactory,
        RemoteBlobOriginOptions options,
        ILogger<HttpRemoteBlobStoreProvider> logger
    )
    {
        _httpClientFactory = httpClientFactory;
        _options = options;
        _logger = logger;
    }

    public async ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        if (IsCircuitOpen())
        {
            _logger.LogWarning(
                "Remote blob origin GET short-circuited by open circuit breaker for {Hash}; fetch degraded to miss",
                hashHex
            );
            return BlobReadResult.Missing;
        }

        using var response = await SendWithRetryAsync(() => CreateRequest(HttpMethod.Get, hashHex), cancellationToken);

        if (response.StatusCode == HttpStatusCode.NotFound)
        {
            return BlobReadResult.Missing;
        }

        if (response.StatusCode == HttpStatusCode.OK)
        {
            var bytes = await response.Content.ReadAsByteArrayAsync(cancellationToken);
            var contentType = response.Content.Headers.ContentType?.MediaType;

            // Content encoding (reserved v1.1, docs/BLOB_HTTP_SURFACE.md): a
            // "Content-Encoding: zstd" response carries the stored zstd
            // frame; decompress before the chain verifies BLAKE3 of these
            // bytes — the invariant (hash of uncompressed bytes) is
            // preserved by doing this here rather than in the chain.
            if (BlobCompression.HasZstdContentEncoding(response))
            {
                var decompressed = BlobCompression.TryDecompress(bytes);
                if (decompressed is null)
                {
                    throw new InvalidOperationException(
                        $"remote blob origin GET /blobs/{hashHex} returned a corrupt zstd frame"
                    );
                }

                return BlobReadResult.Hit(decompressed, contentType);
            }

            return BlobReadResult.Hit(bytes, contentType);
        }

        throw new InvalidOperationException(
            $"remote blob origin GET /blobs/{hashHex} returned unexpected status {(int)response.StatusCode}"
        );
    }

    public ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        return PushAsync(hashHex, bytes, contentType, cancellationToken);
    }

    public async ValueTask PushAsync(string hashHex, byte[] bytes, string? contentType, CancellationToken ct = default)
    {
        if (IsCircuitOpen())
        {
            throw new InvalidOperationException(
                $"remote blob origin PUT /blobs/{hashHex} short-circuited by open circuit breaker"
            );
        }

        using var response = await SendWithRetryAsync(() => CreatePutRequest(hashHex, bytes, contentType), ct);

        if (response.StatusCode is HttpStatusCode.OK or HttpStatusCode.Created)
        {
            return;
        }

        if (response.StatusCode == HttpStatusCode.UnprocessableEntity)
        {
            // Hash mismatch: the ORIGIN computed a different BLAKE3 than
            // hashHex, which means our local bytes are corrupt. Never
            // retry — retrying would just resend the same bad bytes.
            throw new InvalidOperationException(
                $"remote blob origin rejected PUT /blobs/{hashHex} with 422 hash mismatch — local bytes are corrupt"
            );
        }

        throw new InvalidOperationException(
            $"remote blob origin PUT /blobs/{hashHex} returned unexpected status {(int)response.StatusCode}"
        );
    }

    /// <summary>
    /// A real origin <c>HEAD /blobs/{hash}</c> — no body transfer, no
    /// decompress, no BLAKE3 verify, no local write-back. Satisfies both
    /// <see cref="IRemoteBlobPushTarget.ExistsAsync"/> (the reconcile sweep)
    /// and, since slice 2.1, <see cref="IBlobStoreProvider.ExistsAsync"/> —
    /// same signature, same cheap-probe contract, one implementation. Note
    /// this provider is never registered as the top-level
    /// <see cref="IBlobStoreProvider"/> directly (always behind
    /// <see cref="ChainedBlobStoreProvider"/>), so the "throw on open
    /// breaker" behavior below is caught there and degrades to a miss —
    /// matching <see cref="TryGetBlobAsync"/>'s own breaker-open posture.
    /// </summary>
    public async ValueTask<bool> ExistsAsync(string hashHex, CancellationToken ct = default)
    {
        if (IsCircuitOpen())
        {
            throw new InvalidOperationException(
                $"remote blob origin HEAD /blobs/{hashHex} short-circuited by open circuit breaker"
            );
        }

        using var response = await SendWithRetryAsync(() => CreateRequest(HttpMethod.Head, hashHex), ct);

        if (response.StatusCode == HttpStatusCode.OK)
        {
            return true;
        }

        if (response.StatusCode == HttpStatusCode.NotFound)
        {
            return false;
        }

        throw new InvalidOperationException(
            $"remote blob origin HEAD /blobs/{hashHex} returned unexpected status {(int)response.StatusCode}"
        );
    }

    /// <summary>
    /// Sends one logical request with the retry classification from
    /// docs/BLOB_HTTP_SURFACE.md (5xx/429/timeouts retryable; everything
    /// else — including 404, which callers treat as a legitimate answer —
    /// returned as-is). Any response that comes back (including 4xx) counts
    /// as a circuit-breaker success: the origin is up and answering, even if
    /// the answer is an error for this particular request.
    /// </summary>
    private async Task<HttpResponseMessage> SendWithRetryAsync(
        Func<HttpRequestMessage> requestFactory,
        CancellationToken cancellationToken
    )
    {
        var client = _httpClientFactory.CreateClient(HttpClientName);
        var maxAttempts = _options.MaxRetries + 1;

        for (var attempt = 1; attempt <= maxAttempts; attempt++)
        {
            HttpResponseMessage response;
            try
            {
                using var request = requestFactory();
                response = await client.SendAsync(request, cancellationToken);
            }
            catch (TaskCanceledException ex) when (!cancellationToken.IsCancellationRequested)
            {
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    throw new InvalidOperationException(
                        $"remote blob origin request timed out after {maxAttempts} attempt(s)",
                        ex
                    );
                }
                continue;
            }
            catch (HttpRequestException ex)
            {
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    throw new InvalidOperationException(
                        $"remote blob origin request failed after {maxAttempts} attempt(s)",
                        ex
                    );
                }
                continue;
            }

            var statusCode = (int)response.StatusCode;
            var isTransient = statusCode >= 500 || statusCode == 429;
            if (!isTransient)
            {
                RecordSuccess();
                return response;
            }

            response.Dispose();
            if (attempt == maxAttempts)
            {
                RecordFailure();
                throw new InvalidOperationException(
                    $"remote blob origin returned transient status {statusCode} after {maxAttempts} attempt(s)"
                );
            }
        }

        // Unreachable: the loop above always returns or throws on its last
        // iteration.
        throw new InvalidOperationException("remote blob origin retry loop exited unexpectedly");
    }

    private HttpRequestMessage CreateRequest(HttpMethod method, string hashHex)
    {
        var request = new HttpRequestMessage(method, $"/blobs/{hashHex}");
        if (method == HttpMethod.Get)
        {
            // Content encoding (reserved v1.1, docs/BLOB_HTTP_SURFACE.md):
            // advertise zstd support so an origin that has the blob
            // zstd-encoded at rest can serve it as-is.
            request.Headers.AcceptEncoding.Add(new StringWithQualityHeaderValue("zstd"));
        }
        ApplyAuth(request);
        return request;
    }

    private HttpRequestMessage CreatePutRequest(string hashHex, byte[] bytes, string? contentType)
    {
        var request = new HttpRequestMessage(HttpMethod.Put, $"/blobs/{hashHex}")
        {
            Content = new ByteArrayContent(bytes)
        };

        if (!string.IsNullOrWhiteSpace(contentType))
        {
            request.Content.Headers.ContentType = new MediaTypeHeaderValue(contentType);
        }

        ApplyAuth(request);
        return request;
    }

    private void ApplyAuth(HttpRequestMessage request)
    {
        if (!string.IsNullOrWhiteSpace(_options.AuthToken))
        {
            request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", _options.AuthToken);
        }
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
