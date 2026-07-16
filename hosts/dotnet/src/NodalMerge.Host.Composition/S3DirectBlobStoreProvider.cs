using NodalMerge.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging;
using System.Globalization;
using System.Net;
using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text.Json.Serialization;

namespace NodalMerge.Host.Composition;

/// <summary>
/// The s3-direct chain link (slice 4.3,
/// nodalmerge-studio/plans/cas-distribution-and-storage.md Phase 4): resolves
/// a presigned bucket URL from the origin's frozen "Blob URL resolution"
/// surface (docs/BLOB_HTTP_SURFACE.md, <c>GET /blobs/{hash}/url</c> +
/// <c>POST /blobs/{hash}/uploaded</c>), then GETs/PUTs the bucket itself —
/// bytes never round-trip through the origin process. Sibling of
/// <see cref="HttpRemoteBlobStoreProvider"/> (the relay link): same origin
/// surface, different endpoint family, same "does not verify" contract —
/// BLAKE3 verification of fetched bytes happens exactly once, in
/// <see cref="ChainedBlobStoreProvider"/> (directly, or through
/// <see cref="RemoteBlobLinkAggregator"/> when both links are configured).
///
/// ## Zstd at rest (client-side, since S3 won't compress for you)
///
/// <see cref="PutBlobAsync"/> reuses <see cref="BlobCompression"/>'s skip
/// heuristic (shared with <see cref="FileBlobStoreProvider"/>, not
/// duplicated) to decide whether to zstd-compress before uploading. The
/// encoding signal on the bucket side is the standard HTTP
/// <c>Content-Encoding</c> header, sent on the presigned PUT and read back
/// verbatim on a later presigned GET — S3-compatible object stores persist
/// and return this header as object metadata regardless of what the GET
/// request's own <c>Accept-Encoding</c> says (unlike an app server, a bucket
/// does not negotiate). This is the mechanism
/// docs/BLOB_STORAGE_LAYOUT.md §8's "S3 stores: ... with a contentEncoding
/// object metadata entry" describes; the doc's `.zst`-suffixed-key half of
/// that sentence is NOT implemented by today's reference presign backend
/// (<c>nodalmerge-s3-blobs::S3BlobStore::key_for</c> always signs the bare
/// hex key — see the note on <see cref="PutBlobAsync"/>), so this client
/// does not attempt to select a `.zst` key itself.
///
/// ## 501 capability probe
///
/// A server with no presign-capable backend (or a relay-only deployment)
/// answers <c>op=get</c>/<c>op=put</c> with 501 — see
/// <see cref="TryResolveUrlAsync"/>. Only the <c>op=get</c> 501 is cached
/// (a short cooldown, <see cref="S3DirectBlobOriginOptions.CapabilityProbeCooldownSeconds"/>)
/// to avoid hammering an origin that has plainly declared "no s3-direct
/// capability at all" on every single cold-cache read. <c>op=put</c>'s 501
/// is deliberately NEVER cached: the reference backend's
/// <c>direct_upload_threshold</c> means a small blob can legitimately 501
/// while a large one presigns fine, and a process-wide cache keyed only on
/// "put declined" would wrongly suppress every future put regardless of
/// size.
/// </summary>
public sealed class S3DirectBlobStoreProvider : IBlobStoreProvider
{
    /// <summary>
    /// Named HTTP client for the bucket itself: presigned URLs are
    /// self-authorizing absolute URLs, so this client carries no
    /// <c>BaseAddress</c> and — critically — no default headers. The
    /// origin's bearer token (if any) must never be forwarded to the
    /// bucket host.
    /// </summary>
    public const string BucketHttpClientName = "NodalMerge.S3DirectBucket";

    private readonly IHttpClientFactory _httpClientFactory;
    private readonly RemoteBlobOriginOptions _originOptions;
    private readonly S3DirectBlobOriginOptions _options;
    private readonly ILogger<S3DirectBlobStoreProvider> _logger;
    private readonly object _circuitLock = new();
    private readonly object _capabilityLock = new();

    private int _consecutiveFailures;
    private DateTimeOffset? _circuitOpenedUntil;
    private DateTimeOffset? _getCapabilityUnavailableUntil;

    public S3DirectBlobStoreProvider(
        IHttpClientFactory httpClientFactory,
        RemoteBlobOriginOptions originOptions,
        S3DirectBlobOriginOptions options,
        ILogger<S3DirectBlobStoreProvider> logger
    )
    {
        _httpClientFactory = httpClientFactory;
        _originOptions = originOptions;
        _options = options;
        _logger = logger;
    }

    public async ValueTask<BlobReadResult> TryGetBlobAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        if (IsGetCapabilityProbeCoolingDown())
        {
            _logger.LogDebug(
                "s3-direct GET URL resolution recently answered 501 for {Hash}; skipping probe during cooldown",
                hashHex
            );
            return BlobReadResult.Missing;
        }

        if (IsCircuitOpen())
        {
            _logger.LogWarning(
                "s3-direct origin URL resolution short-circuited by open breaker for {Hash}; degraded to miss",
                hashHex
            );
            return BlobReadResult.Missing;
        }

        var resolved = await TryResolveUrlAsync(hashHex, "get", sizeBytes: null, contentType: null, cancellationToken);
        if (resolved is null)
        {
            // Either a clean 501 (capability cooldown already recorded by
            // TryResolveUrlAsync) or an exhausted-retry transient failure
            // (already logged there) — either way, local-first
            // availability: this is a degraded miss, exactly like a remote
            // origin miss on the relay link.
            return BlobReadResult.Missing;
        }

        using var bucketResponse = await SendToBucketAsync(
            HttpMethod.Get,
            resolved.Url,
            body: null,
            contentType: null,
            contentEncoding: null,
            cancellationToken
        );

        if (bucketResponse.StatusCode == HttpStatusCode.NotFound)
        {
            return BlobReadResult.Missing;
        }

        if (bucketResponse.StatusCode != HttpStatusCode.OK)
        {
            throw new InvalidOperationException(
                $"s3-direct bucket GET for {hashHex} returned unexpected status {(int)bucketResponse.StatusCode}"
            );
        }

        var bytes = await bucketResponse.Content.ReadAsByteArrayAsync(cancellationToken);

        // Content encoding (docs/BLOB_STORAGE_LAYOUT.md §8 / BLOB_HTTP_SURFACE.md
        // "Content encoding"): the bucket returns whatever Content-Encoding
        // was stored with the object, unconditionally. Decompress here,
        // before this method returns — the chain's single verify gate
        // (ChainedBlobStoreProvider) must see uncompressed bytes.
        if (BlobCompression.HasZstdContentEncoding(bucketResponse))
        {
            var decompressed = BlobCompression.TryDecompress(bytes);
            if (decompressed is null)
            {
                // Corrupt frame. Throwing here (rather than returning
                // Missing directly) matches HttpRemoteBlobStoreProvider's
                // posture: whichever caller composes this link
                // (ChainedBlobStoreProvider directly, or
                // RemoteBlobLinkAggregator when the relay link is also
                // configured) catches this, logs it, and treats it as a
                // miss/fallthrough — it is never written to the local
                // cache and never served as wrong bytes.
                throw new InvalidOperationException(
                    $"s3-direct bucket GET for {hashHex} returned a corrupt zstd frame"
                );
            }

            return BlobReadResult.Hit(decompressed, contentType: null);
        }

        return BlobReadResult.Hit(bytes, contentType: null);
    }

    /// <summary>
    /// Cheap existence probe (slice 2.1): resolves the same presigned
    /// <c>op=get</c> URL <see cref="TryGetBlobAsync"/> would use, but issues
    /// an HTTP <c>HEAD</c> against the bucket instead of a <c>GET</c> — no
    /// body transfer, no decompress, no BLAKE3 verify. This still costs one
    /// origin round-trip (URL resolution — the reference presign backend has
    /// no dedicated "head" op, only <c>get</c>/<c>put</c>, see
    /// <c>server/s3-blobs/src/lib.rs:133</c>) plus one bucket round-trip, but
    /// avoids the expensive part entirely: downloading and verifying bytes
    /// that are then thrown away.
    ///
    /// ⚠ Unverified assumption, flagged rather than silently relied on: this
    /// depends on the bucket honoring an HTTP HEAD against a URL presigned
    /// for GET. This is standard, documented behavior for AWS S3 itself, but
    /// has not been verified here against a real S3-compatible backend
    /// (no MinIO/testcontainers harness in this environment) — if a
    /// deployment's object store rejects method-mismatched presigned
    /// requests, this call surfaces as a thrown
    /// <see cref="InvalidOperationException"/> (an unexpected bucket status),
    /// not a silent wrong answer.
    /// </summary>
    public async ValueTask<bool> ExistsAsync(string hashHex, CancellationToken cancellationToken = default)
    {
        if (IsGetCapabilityProbeCoolingDown())
        {
            _logger.LogDebug(
                "s3-direct GET URL resolution recently answered 501 for {Hash}; skipping existence probe during cooldown",
                hashHex
            );
            return false;
        }

        if (IsCircuitOpen())
        {
            _logger.LogWarning(
                "s3-direct origin URL resolution short-circuited by open breaker for {Hash}; existence probe degraded to absent",
                hashHex
            );
            return false;
        }

        var resolved = await TryResolveUrlAsync(hashHex, "get", sizeBytes: null, contentType: null, cancellationToken);
        if (resolved is null)
        {
            // Clean 501 (no s3-direct capability) or exhausted-retry
            // transient failure — either way, degraded absence, matching
            // TryGetBlobAsync's posture for the same two cases.
            return false;
        }

        using var bucketResponse = await SendToBucketAsync(
            HttpMethod.Head,
            resolved.Url,
            body: null,
            contentType: null,
            contentEncoding: null,
            cancellationToken
        );

        if (bucketResponse.StatusCode == HttpStatusCode.OK)
        {
            return true;
        }

        if (bucketResponse.StatusCode == HttpStatusCode.NotFound)
        {
            return false;
        }

        throw new InvalidOperationException(
            $"s3-direct bucket HEAD for {hashHex} returned unexpected status {(int)bucketResponse.StatusCode}"
        );
    }

    /// <summary>
    /// Compress (maybe) -> resolve a presigned PUT URL for the
    /// (possibly-compressed) byte count -> PUT to the bucket -> confirm the
    /// upload. Any failure along this path throws — an unconfirmed upload
    /// must never look like a success, since the GC lifecycle
    /// (docs/delegated-storage-gc.md) leaves it stuck at <c>Uploading</c>
    /// until confirmed, and the caller (ChainedBlobStoreProvider /
    /// RemoteBlobLinkAggregator) already treats a thrown remote PutBlobAsync
    /// as "local write is safe, remote push failed, reconcile sweep will
    /// heal it" — never as a fatal error for the whole call.
    ///
    /// Note on key derivation: the reference presign backend
    /// (<c>nodalmerge-s3-blobs::S3BlobStore::key_for</c>) always signs
    /// <c>{prefix}blake3/{hash}</c> — the bare hex key, never a `.zst`
    /// variant — for both GET and PUT, regardless of whether this client
    /// compresses. There is therefore no "pick the `.zst` key" step here;
    /// the single object at that key simply carries whichever encoding this
    /// PUT declared via its <c>Content-Encoding</c> header, and a later GET
    /// reads that same header back.
    /// </summary>
    public async ValueTask PutBlobAsync(
        string hashHex,
        byte[] bytes,
        string? contentType,
        CancellationToken cancellationToken = default
    )
    {
        var shouldCompress = string.Equals(_options.Compression, "Zstd", StringComparison.Ordinal)
            && BlobCompression.ShouldCompress(bytes, contentType, _options.CompressionMinBytes, _options.CompressionLevel);

        var payload = shouldCompress ? BlobCompression.Compress(bytes, _options.CompressionLevel) : bytes;
        var contentEncoding = shouldCompress ? "zstd" : null;

        if (IsCircuitOpen())
        {
            throw new InvalidOperationException(
                $"s3-direct origin URL resolution short-circuited by open breaker for PUT {hashHex}"
            );
        }

        // size is the UPLOADED byte count (i.e. the compressed size when we
        // compressed) — the presign backend may use it for a size cap, and
        // it must reflect what actually goes over the wire to the bucket.
        var resolved = await TryResolveUrlAsync(hashHex, "put", payload.Length, contentType, cancellationToken);
        if (resolved is null)
        {
            throw new InvalidOperationException(
                $"s3-direct origin declined to presign PUT for {hashHex} (no backend, or backend declined this request — e.g. below its direct-upload size threshold)"
            );
        }

        using var putResponse = await SendToBucketAsync(
            HttpMethod.Put,
            resolved.Url,
            payload,
            contentType,
            contentEncoding,
            cancellationToken
        );

        if (putResponse.StatusCode is not (HttpStatusCode.OK or HttpStatusCode.Created or HttpStatusCode.NoContent))
        {
            throw new InvalidOperationException(
                $"s3-direct bucket PUT for {hashHex} returned unexpected status {(int)putResponse.StatusCode}"
            );
        }

        await ConfirmUploadedAsync(hashHex, cancellationToken);
    }

    /// <summary>
    /// <c>POST /blobs/{hash}/uploaded</c> — any non-200 is a failed put
    /// (docs/BLOB_HTTP_SURFACE.md "Blob URL resolution": 409 verification
    /// failure, 501 no backend at all, or a transport failure).
    /// </summary>
    private async Task ConfirmUploadedAsync(string hashHex, CancellationToken cancellationToken)
    {
        var client = _httpClientFactory.CreateClient(HttpRemoteBlobStoreProvider.HttpClientName);
        using var request = new HttpRequestMessage(HttpMethod.Post, $"/blobs/{hashHex}/uploaded");
        ApplyOriginAuth(request);

        HttpResponseMessage response;
        try
        {
            response = await client.SendAsync(request, cancellationToken);
        }
        catch (Exception ex) when (ex is HttpRequestException or TaskCanceledException)
        {
            throw new InvalidOperationException(
                $"s3-direct upload confirmation POST /blobs/{hashHex}/uploaded failed",
                ex
            );
        }

        using (response)
        {
            if (response.StatusCode != HttpStatusCode.OK)
            {
                throw new InvalidOperationException(
                    $"s3-direct upload confirmation for {hashHex} returned unexpected status {(int)response.StatusCode}"
                );
            }
        }
    }

    /// <summary>
    /// <c>GET {origin}/blobs/{hash}/url?op=get|put[&amp;size=&amp;contentType=]</c>
    /// with the same 5xx/429/timeout retry classification as
    /// <see cref="HttpRemoteBlobStoreProvider"/>. Returns <c>null</c> for a
    /// clean 501 (capability declined — cached for <c>op=get</c> only, see
    /// the class doc) or an exhausted-retry transient failure (logged, not
    /// thrown, since both are "this link isn't available right now", not
    /// "this request is invalid"). Any other unexpected status throws.
    /// </summary>
    private async Task<PresignedUrlInfo?> TryResolveUrlAsync(
        string hashHex,
        string op,
        int? sizeBytes,
        string? contentType,
        CancellationToken cancellationToken
    )
    {
        var client = _httpClientFactory.CreateClient(HttpRemoteBlobStoreProvider.HttpClientName);
        var maxAttempts = _options.MaxRetries + 1;

        for (var attempt = 1; attempt <= maxAttempts; attempt++)
        {
            HttpResponseMessage response;
            try
            {
                using var request = CreateUrlResolveRequest(hashHex, op, sizeBytes, contentType);
                response = await client.SendAsync(request, cancellationToken);
            }
            catch (TaskCanceledException ex) when (!cancellationToken.IsCancellationRequested)
            {
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    _logger.LogWarning(
                        ex,
                        "s3-direct URL resolution timed out after {Attempts} attempt(s) for {Op} {Hash}",
                        maxAttempts,
                        op,
                        hashHex
                    );
                    return null;
                }
                continue;
            }
            catch (HttpRequestException ex)
            {
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    _logger.LogWarning(
                        ex,
                        "s3-direct URL resolution failed after {Attempts} attempt(s) for {Op} {Hash}",
                        maxAttempts,
                        op,
                        hashHex
                    );
                    return null;
                }
                continue;
            }

            if (response.StatusCode == HttpStatusCode.NotImplemented)
            {
                response.Dispose();
                // The origin answered (it's healthy) — it's just declining
                // s3-direct capability for this op, which is not a breaker
                // failure.
                RecordSuccess();
                if (string.Equals(op, "get", StringComparison.Ordinal))
                {
                    SetGetCapabilityCooldown();
                }
                return null;
            }

            var statusCode = (int)response.StatusCode;
            var isTransient = statusCode >= 500 || statusCode == 429;
            if (isTransient)
            {
                response.Dispose();
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    _logger.LogWarning(
                        "s3-direct URL resolution returned transient status {Status} after {Attempts} attempt(s) for {Op} {Hash}",
                        statusCode,
                        maxAttempts,
                        op,
                        hashHex
                    );
                    return null;
                }
                continue;
            }

            RecordSuccess();

            if (response.StatusCode != HttpStatusCode.OK)
            {
                response.Dispose();
                throw new InvalidOperationException(
                    $"s3-direct URL resolution for {op} {hashHex} returned unexpected status {statusCode}"
                );
            }

            using (response)
            {
                var body = await response.Content.ReadFromJsonAsync<BlobUrlResponse>(cancellationToken: cancellationToken);
                if (body is null || string.IsNullOrWhiteSpace(body.Url))
                {
                    throw new InvalidOperationException(
                        $"s3-direct URL resolution for {op} {hashHex} returned an empty body"
                    );
                }

                return new PresignedUrlInfo(body.Url, TryParseExpiry(body.ExpiresAtUtc));
            }
        }

        // Unreachable: the loop above always returns or throws on its last
        // iteration.
        throw new InvalidOperationException("s3-direct URL resolution retry loop exited unexpectedly");
    }

    private HttpRequestMessage CreateUrlResolveRequest(string hashHex, string op, int? sizeBytes, string? contentType)
    {
        var query = $"op={Uri.EscapeDataString(op)}";
        if (sizeBytes is not null)
        {
            query += $"&size={sizeBytes.Value}";
        }
        if (!string.IsNullOrWhiteSpace(contentType))
        {
            query += $"&contentType={Uri.EscapeDataString(contentType)}";
        }

        var request = new HttpRequestMessage(HttpMethod.Get, $"/blobs/{hashHex}/url?{query}");
        ApplyOriginAuth(request);
        return request;
    }

    /// <summary>
    /// PUT/GET the bucket itself via the presigned URL. Deliberately never
    /// calls <see cref="ApplyOriginAuth"/> — presigned URLs are
    /// self-authorizing, and forwarding the origin's own bearer token to an
    /// arbitrary bucket host would leak a credential the bucket has no use
    /// for (and, depending on the bucket, might reject the request over an
    /// unexpected header).
    /// </summary>
    private async Task<HttpResponseMessage> SendToBucketAsync(
        HttpMethod method,
        string url,
        byte[]? body,
        string? contentType,
        string? contentEncoding,
        CancellationToken cancellationToken
    )
    {
        var client = _httpClientFactory.CreateClient(BucketHttpClientName);
        using var request = new HttpRequestMessage(method, url);

        if (body is not null)
        {
            request.Content = new ByteArrayContent(body);
            if (!string.IsNullOrWhiteSpace(contentType))
            {
                request.Content.Headers.ContentType = new MediaTypeHeaderValue(contentType);
            }
            if (!string.IsNullOrWhiteSpace(contentEncoding))
            {
                request.Content.Headers.ContentEncoding.Add(contentEncoding);
            }
        }

        return await client.SendAsync(request, cancellationToken);
    }

    private void ApplyOriginAuth(HttpRequestMessage request)
    {
        if (!string.IsNullOrWhiteSpace(_originOptions.AuthToken))
        {
            request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", _originOptions.AuthToken);
        }
    }

    /// <summary>
    /// Lenient ISO-8601 parse: the Rust origin emits <c>expiresAtUtc</c> as
    /// <c>YYYY-MM-DDTHH:MM:SSZ</c> with no fractional seconds, which
    /// <see cref="DateTimeOffset"/>'s round-trip ("o") format does NOT
    /// accept — never assume that format here. A failed parse is logged
    /// and treated as "unknown expiry" (<c>null</c>), not a hard failure:
    /// this client fetches and uses a fresh URL immediately on every call
    /// rather than caching one across calls, so the expiry value is
    /// informational only today.
    /// </summary>
    private DateTimeOffset? TryParseExpiry(string? raw)
    {
        if (string.IsNullOrWhiteSpace(raw))
        {
            return null;
        }

        if (DateTimeOffset.TryParse(
            raw,
            CultureInfo.InvariantCulture,
            DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal,
            out var parsed))
        {
            return parsed;
        }

        _logger.LogWarning("s3-direct URL resolution returned an unparseable expiresAtUtc value {Raw}", raw);
        return null;
    }

    private bool IsGetCapabilityProbeCoolingDown()
    {
        lock (_capabilityLock)
        {
            if (_getCapabilityUnavailableUntil is null)
            {
                return false;
            }

            if (DateTimeOffset.UtcNow < _getCapabilityUnavailableUntil.Value)
            {
                return true;
            }

            _getCapabilityUnavailableUntil = null;
            return false;
        }
    }

    private void SetGetCapabilityCooldown()
    {
        if (_options.CapabilityProbeCooldownSeconds <= 0)
        {
            return;
        }

        lock (_capabilityLock)
        {
            _getCapabilityUnavailableUntil = DateTimeOffset.UtcNow.AddSeconds(_options.CapabilityProbeCooldownSeconds);
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

    private sealed record PresignedUrlInfo(string Url, DateTimeOffset? ExpiresAtUtc);

    private sealed class BlobUrlResponse
    {
        [JsonPropertyName("url")]
        public string Url { get; set; } = string.Empty;

        [JsonPropertyName("expiresAtUtc")]
        public string? ExpiresAtUtc { get; set; }
    }
}
