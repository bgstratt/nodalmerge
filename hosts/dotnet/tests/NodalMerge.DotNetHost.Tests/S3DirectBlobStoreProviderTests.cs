using System.Net;
using System.Net.Http.Headers;
using System.Text;
using Microsoft.Extensions.Logging.Abstractions;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Drives <see cref="S3DirectBlobStoreProvider"/> directly against two fake
/// handlers — one for the origin's URL-resolution endpoints
/// (<c>GET /blobs/{hash}/url</c>, <c>POST /blobs/{hash}/uploaded</c>) and one
/// for "the bucket" (whatever the presigned URL points at) — no DI, no real
/// network, mirroring <c>HttpRemoteBlobStoreProviderTests</c>'s style for the
/// relay link (slice 4.3,
/// nodalmerge-studio/plans/cas-distribution-and-storage.md Phase 4).
/// </summary>
public sealed class S3DirectBlobStoreProviderTests
{
    private const string OriginBaseUrl = "https://origin.test";
    private const string BucketUrl = "https://bucket.test/presigned-object";
    private static readonly string Hash = new('a', 64);

    [Fact]
    public async Task Get_hit_with_zstd_content_encoding_is_decompressed_before_returning()
    {
        var plaintext = Encoding.UTF8.GetBytes(new string('x', 8192));
        var compressed = CompressForTest(plaintext);

        var origin = new QueueHandler([UrlResolveOk(BucketUrl)]);
        var bucket = new QueueHandler([ZstdBucketResponse(compressed)]);
        var provider = BuildProvider(origin, bucket);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(plaintext, result.Bytes);
        Assert.Equal(1, origin.CallCount);
        Assert.Equal(1, bucket.CallCount);
    }

    [Fact]
    public async Task Get_hit_without_content_encoding_returns_raw_bytes()
    {
        var bytes = new byte[] { 1, 2, 3, 4 };
        var origin = new QueueHandler([UrlResolveOk(BucketUrl)]);
        var bucket = new QueueHandler([MakeResponse(HttpStatusCode.OK, bytes)]);
        var provider = BuildProvider(origin, bucket);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(bytes, result.Bytes);
    }

    [Fact]
    public async Task Get_url_resolution_501_falls_through_as_miss()
    {
        var origin = new QueueHandler([MakeResponse(HttpStatusCode.NotImplemented)]);
        var bucket = new QueueHandler([]);
        var provider = BuildProvider(origin, bucket);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.False(result.Found);
        Assert.Equal(0, bucket.CallCount);
    }

    [Fact]
    public async Task Get_url_resolution_501_is_capability_cached_for_subsequent_get_calls()
    {
        var origin = new QueueHandler([MakeResponse(HttpStatusCode.NotImplemented)]);
        var bucket = new QueueHandler([]);
        var provider = BuildProvider(origin, bucket, capabilityProbeCooldownSeconds: 300);

        var first = await provider.TryGetBlobAsync(Hash);
        var second = await provider.TryGetBlobAsync(Hash);

        Assert.False(first.Found);
        Assert.False(second.Found);
        // Only the first GET actually probed the origin; the second was
        // short-circuited by the capability cooldown.
        Assert.Equal(1, origin.CallCount);
    }

    [Fact]
    public async Task Corrupted_bucket_zstd_frame_throws_and_is_never_returned_as_a_hit()
    {
        var corrupt = new byte[] { 0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x01, 0x02 };
        var origin = new QueueHandler([UrlResolveOk(BucketUrl)]);
        var bucket = new QueueHandler([ZstdBucketResponse(corrupt)]);
        var provider = BuildProvider(origin, bucket);

        await Assert.ThrowsAsync<InvalidOperationException>(() => provider.TryGetBlobAsync(Hash).AsTask());
    }

    [Fact]
    public async Task Bucket_404_is_a_miss()
    {
        var origin = new QueueHandler([UrlResolveOk(BucketUrl)]);
        var bucket = new QueueHandler([MakeResponse(HttpStatusCode.NotFound)]);
        var provider = BuildProvider(origin, bucket);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.False(result.Found);
    }

    /// <summary>
    /// Slice 2.1 (nodalmerge-studio/plans/blob-cas-remediation.md):
    /// <see cref="S3DirectBlobStoreProvider.ExistsAsync"/> resolves the same
    /// presigned <c>op=get</c> URL as <see cref="S3DirectBlobStoreProvider.TryGetBlobAsync"/>
    /// but issues a bucket <c>HEAD</c> instead of a <c>GET</c> — no body is
    /// ever fetched.
    /// </summary>
    [Fact]
    public async Task ExistsAsync_bucket_200_is_true_and_never_fetches_a_body()
    {
        var origin = new QueueHandler([UrlResolveOk(BucketUrl)]);
        HttpRequestMessage? capturedBucketRequest = null;
        var bucket = new CapturingRequestHandler(req =>
        {
            capturedBucketRequest = req;
            return MakeResponse(HttpStatusCode.OK);
        });
        var provider = BuildProvider(origin, bucket);

        var exists = await provider.ExistsAsync(Hash);

        Assert.True(exists);
        Assert.NotNull(capturedBucketRequest);
        Assert.Equal(HttpMethod.Head, capturedBucketRequest!.Method);
    }

    [Fact]
    public async Task ExistsAsync_bucket_404_is_false()
    {
        var origin = new QueueHandler([UrlResolveOk(BucketUrl)]);
        var bucket = new QueueHandler([MakeResponse(HttpStatusCode.NotFound)]);
        var provider = BuildProvider(origin, bucket);

        Assert.False(await provider.ExistsAsync(Hash));
    }

    [Fact]
    public async Task ExistsAsync_url_resolution_501_is_false_and_does_not_touch_the_bucket()
    {
        var origin = new QueueHandler([MakeResponse(HttpStatusCode.NotImplemented)]);
        var bucket = new QueueHandler([]);
        var provider = BuildProvider(origin, bucket);

        Assert.False(await provider.ExistsAsync(Hash));
        Assert.Equal(0, bucket.CallCount);
    }

    [Fact]
    public async Task ExistsAsync_unexpected_bucket_status_throws()
    {
        var origin = new QueueHandler([UrlResolveOk(BucketUrl)]);
        var bucket = new QueueHandler([MakeResponse(HttpStatusCode.InternalServerError)]);
        var provider = BuildProvider(origin, bucket);

        await Assert.ThrowsAsync<InvalidOperationException>(() => provider.ExistsAsync(Hash).AsTask());
    }

    [Fact]
    public async Task Put_compresses_and_uploads_with_zstd_content_encoding_then_confirms()
    {
        var payload = Encoding.UTF8.GetBytes(new string('y', 8192));

        HttpRequestMessage? capturedUrlRequest = null;
        HttpRequestMessage? capturedBucketRequest = null;
        HttpRequestMessage? capturedConfirmRequest = null;
        byte[]? capturedBucketBody = null;

        var origin = new SequencedHandler(
            req =>
            {
                capturedUrlRequest = req;
                return UrlResolveOk(BucketUrl);
            },
            req =>
            {
                capturedConfirmRequest = req;
                return MakeResponse(HttpStatusCode.OK);
            }
        );
        var bucket = new CapturingRequestHandler(async req =>
        {
            capturedBucketRequest = req;
            capturedBucketBody = req.Content is null ? null : await req.Content.ReadAsByteArrayAsync();
            return MakeResponse(HttpStatusCode.OK);
        });
        var provider = BuildProvider(origin, bucket);

        await provider.PutBlobAsync(Hash, payload, "text/plain");

        Assert.NotNull(capturedUrlRequest);
        Assert.Contains("op=put", capturedUrlRequest!.RequestUri!.Query);
        Assert.Contains("size=", capturedUrlRequest.RequestUri.Query);

        Assert.NotNull(capturedBucketRequest);
        Assert.Equal("zstd", string.Join(",", capturedBucketRequest!.Content!.Headers.ContentEncoding));
        Assert.NotNull(capturedBucketBody);
        // Uploaded bytes are the compressed form — strictly smaller than the
        // (highly compressible) original payload.
        Assert.True(capturedBucketBody!.Length < payload.Length);
        Assert.Null(capturedBucketRequest.Headers.Authorization);

        Assert.NotNull(capturedConfirmRequest);
        Assert.Equal($"/blobs/{Hash}/uploaded", capturedConfirmRequest!.RequestUri!.AbsolutePath);
    }

    [Fact]
    public async Task Put_skips_compression_for_incompressible_payload_and_uploads_raw()
    {
        var random = new Random(42);
        var payload = new byte[8192];
        random.NextBytes(payload); // effectively incompressible

        byte[]? capturedBucketBody = null;
        HttpRequestMessage? capturedBucketRequest = null;

        var origin = new SequencedHandler(
            _ => UrlResolveOk(BucketUrl),
            _ => MakeResponse(HttpStatusCode.OK)
        );
        var bucket = new CapturingRequestHandler(async req =>
        {
            capturedBucketRequest = req;
            capturedBucketBody = req.Content is null ? null : await req.Content.ReadAsByteArrayAsync();
            return MakeResponse(HttpStatusCode.OK);
        });
        var provider = BuildProvider(origin, bucket);

        await provider.PutBlobAsync(Hash, payload, "application/octet-stream");

        Assert.NotNull(capturedBucketBody);
        Assert.Equal(payload, capturedBucketBody);
        Assert.Empty(capturedBucketRequest!.Content!.Headers.ContentEncoding);
    }

    [Fact]
    public async Task Put_url_resolution_501_throws_and_is_not_capability_cached()
    {
        // Distinct from GET: op=put's 501 must never be cached, since a real
        // backend can legitimately 501 based on size alone.
        var origin = new QueueHandler(() => MakeResponse(HttpStatusCode.NotImplemented));
        var bucket = new QueueHandler([]);
        var provider = BuildProvider(origin, bucket, capabilityProbeCooldownSeconds: 300);

        await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1, 2, 3], null).AsTask()
        );
        await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [4, 5, 6], null).AsTask()
        );

        // Both puts actually probed the origin — no capability cache
        // suppressed the second attempt.
        Assert.Equal(2, origin.CallCount);
    }

    [Fact]
    public async Task Put_bucket_failure_throws()
    {
        var origin = new SequencedHandler(_ => UrlResolveOk(BucketUrl));
        var bucket = new QueueHandler([MakeResponse(HttpStatusCode.InternalServerError)]);
        var provider = BuildProvider(origin, bucket);

        await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1, 2, 3], null).AsTask()
        );
    }

    [Fact]
    public async Task Put_confirm_failure_throws()
    {
        var origin = new SequencedHandler(
            _ => UrlResolveOk(BucketUrl),
            _ => MakeResponse(HttpStatusCode.Conflict)
        );
        var bucket = new QueueHandler([MakeResponse(HttpStatusCode.OK)]);
        var provider = BuildProvider(origin, bucket);

        await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1, 2, 3], null).AsTask()
        );
    }

    [Fact]
    public async Task Origin_bearer_token_applied_to_url_resolution_but_never_to_bucket()
    {
        AuthenticationHeaderValue? capturedOriginAuth = null;
        AuthenticationHeaderValue? capturedBucketAuth = null;

        var origin = new CapturingRequestHandler(req =>
        {
            capturedOriginAuth = req.Headers.Authorization;
            return Task.FromResult(UrlResolveOk(BucketUrl));
        });
        var bucket = new CapturingRequestHandler(req =>
        {
            capturedBucketAuth = req.Headers.Authorization;
            return Task.FromResult(MakeResponse(HttpStatusCode.OK, [1]));
        });
        var provider = BuildProvider(origin, bucket, authToken: "secret-origin-token");

        await provider.TryGetBlobAsync(Hash);

        Assert.NotNull(capturedOriginAuth);
        Assert.Equal("secret-origin-token", capturedOriginAuth!.Parameter);
        Assert.Null(capturedBucketAuth);
    }

    private static S3DirectBlobStoreProvider BuildProvider(
        HttpMessageHandler originHandler,
        HttpMessageHandler bucketHandler,
        int maxRetries = 2,
        int circuitThreshold = 3,
        int capabilityProbeCooldownSeconds = 0,
        string? authToken = null
    )
    {
        var originClient = new HttpClient(originHandler) { BaseAddress = new Uri(OriginBaseUrl) };
        var bucketClient = new HttpClient(bucketHandler);
        var factory = new NamedFakeHttpClientFactory(new Dictionary<string, HttpClient>
        {
            [HttpRemoteBlobStoreProvider.HttpClientName] = originClient,
            [S3DirectBlobStoreProvider.BucketHttpClientName] = bucketClient
        });

        var originOptions = new RemoteBlobOriginOptions(
            BaseUrl: OriginBaseUrl,
            AuthToken: authToken,
            TimeoutSeconds: 30,
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitThreshold,
            CircuitBreakerOpenSeconds: 30
        );
        var s3DirectOptions = new S3DirectBlobOriginOptions(
            Enabled: true,
            TimeoutSeconds: 30,
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitThreshold,
            CircuitBreakerOpenSeconds: 30,
            CapabilityProbeCooldownSeconds: capabilityProbeCooldownSeconds,
            Compression: "Zstd",
            CompressionLevel: 3,
            CompressionMinBytes: 4096
        );

        return new S3DirectBlobStoreProvider(
            factory,
            originOptions,
            s3DirectOptions,
            NullLogger<S3DirectBlobStoreProvider>.Instance
        );
    }

    private static byte[] CompressForTest(byte[] plaintext)
    {
        using var compressor = new ZstdSharp.Compressor(3);
        return compressor.Wrap(plaintext).ToArray();
    }

    private static HttpResponseMessage UrlResolveOk(string url)
    {
        return new HttpResponseMessage(HttpStatusCode.OK)
        {
            Content = new StringContent(
                $"{{\"url\":\"{url}\",\"expiresAtUtc\":\"2026-07-15T12:34:56Z\"}}",
                Encoding.UTF8,
                "application/json"
            )
        };
    }

    private static HttpResponseMessage ZstdBucketResponse(byte[] compressedBytes)
    {
        var response = MakeResponse(HttpStatusCode.OK, compressedBytes);
        response.Content.Headers.ContentEncoding.Add("zstd");
        return response;
    }

    private static HttpResponseMessage MakeResponse(HttpStatusCode status, byte[]? body = null)
    {
        var response = new HttpResponseMessage(status);
        if (body is not null)
        {
            response.Content = new ByteArrayContent(body);
        }
        return response;
    }

    private sealed class NamedFakeHttpClientFactory(IReadOnlyDictionary<string, HttpClient> clients) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name)
        {
            return clients.TryGetValue(name, out var client)
                ? client
                : throw new InvalidOperationException($"no fake HttpClient registered for '{name}'");
        }
    }

    private sealed class QueueHandler : HttpMessageHandler
    {
        private readonly Queue<HttpResponseMessage>? _queue;
        private readonly Func<HttpResponseMessage>? _factory;

        public int CallCount { get; private set; }

        public QueueHandler(IEnumerable<HttpResponseMessage> responses)
        {
            _queue = new Queue<HttpResponseMessage>(responses);
        }

        public QueueHandler(Func<HttpResponseMessage> factory)
        {
            _factory = factory;
        }

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            CallCount++;
            var response = _factory is not null ? _factory() : _queue!.Dequeue();
            return Task.FromResult(response);
        }
    }

    /// <summary>
    /// Returns one response per call from an ordered list of factories — one
    /// factory per expected call (e.g. URL-resolve then upload-confirm),
    /// each getting the request that produced it.
    /// </summary>
    private sealed class SequencedHandler : HttpMessageHandler
    {
        private readonly Queue<Func<HttpRequestMessage, HttpResponseMessage>> _steps;

        public SequencedHandler(params Func<HttpRequestMessage, HttpResponseMessage>[] steps)
        {
            _steps = new Queue<Func<HttpRequestMessage, HttpResponseMessage>>(steps);
        }

        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            var step = _steps.Dequeue();
            return Task.FromResult(step(request));
        }
    }

    private sealed class CapturingRequestHandler : HttpMessageHandler
    {
        private readonly Func<HttpRequestMessage, Task<HttpResponseMessage>>? _asyncRespond;
        private readonly Func<HttpRequestMessage, HttpResponseMessage>? _respond;

        public CapturingRequestHandler(Func<HttpRequestMessage, HttpResponseMessage> respond)
        {
            _respond = respond;
        }

        public CapturingRequestHandler(Func<HttpRequestMessage, Task<HttpResponseMessage>> respond)
        {
            _asyncRespond = respond;
        }

        protected override async Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            return _asyncRespond is not null ? await _asyncRespond(request) : _respond!(request);
        }
    }
}
