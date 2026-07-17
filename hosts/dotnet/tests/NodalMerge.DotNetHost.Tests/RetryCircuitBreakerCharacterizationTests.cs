using System.Net;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.Configuration;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging.Abstractions;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Tests;

/// <summary>
/// Slice 7.2 (nodalmerge-studio/plans/blob-cas-remediation.md) —
/// characterization tests written against the UNCHANGED three copies of the
/// retry-classification + circuit-breaker logic
/// (<see cref="HttpRemoteBlobStoreProvider"/>,
/// <see cref="S3DirectBlobStoreProvider"/>,
/// <c>S3DelegatedBlobUrlResolverProvider</c>), run green BEFORE the shared
/// component extraction and kept green after it. They pin each provider's
/// CURRENT contract, including the three deliberate points of drift the
/// extraction must NOT unify:
///
///  1. Exhausted transient retries: HttpRemote THROWS; S3Direct and
///     S3Delegated return null/miss (each caller then maps that per method).
///  2. 501 Not Implemented: S3Direct carves it out as "capability declined"
///     (breaker SUCCESS + an op=get-only cooldown; never retried); HttpRemote
///     and S3Delegated classify it as any other 5xx (transient, retried).
///  3. A non-transient error response (4xx): HttpRemote and S3Direct count it
///     as a breaker SUCCESS (the origin answered); S3Delegated counts it as a
///     breaker FAILURE (and gives up without retrying). S3Delegated even
///     counts a 200 whose JSON body parses to null as a breaker failure.
///
/// The delegated provider is internal, so its half drives the real DI
/// composition through the legacy <c>/sync/blob-url</c> route exactly like
/// <see cref="ProviderS3DelegatedBlobResolverIntegrationTests"/> does.
/// These tests are structural characterization (they pin what the code DOES,
/// captured from the pre-extraction implementations), not behavioral REDs.
/// </summary>
public sealed class RetryCircuitBreakerCharacterizationTests
{
    private const string OriginBaseUrl = "https://origin.test";
    private static readonly string Hash = new('b', 64);
    private const string CanonicalHash =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    // ─────────────────────────────────────────────────────────────────────
    // HttpRemoteBlobStoreProvider — exhaustion THROWS; 4xx = breaker success;
    // 501 is a plain transient like any other 5xx.
    // ─────────────────────────────────────────────────────────────────────

    [Fact]
    public async Task HttpRemote_transient_500_exhaustion_throws_with_attempt_count()
    {
        var handler = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.InternalServerError)
        );
        var provider = BuildHttpRemote(handler, maxRetries: 1);

        var ex = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.TryGetBlobAsync(Hash).AsTask()
        );

        Assert.Contains("returned transient status 500 after 2 attempt(s)", ex.Message);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task HttpRemote_timeout_exhaustion_throws_timed_out_with_inner_exception()
    {
        var handler = new SequenceHandler(_ => throw new TaskCanceledException("simulated timeout"));
        var provider = BuildHttpRemote(handler, maxRetries: 1);

        var ex = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.TryGetBlobAsync(Hash).AsTask()
        );

        Assert.Contains("timed out after 2 attempt(s)", ex.Message);
        Assert.IsType<TaskCanceledException>(ex.InnerException);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task HttpRemote_connection_failure_exhaustion_throws_failed_with_inner_exception()
    {
        var handler = new SequenceHandler(_ => throw new HttpRequestException("connection refused"));
        var provider = BuildHttpRemote(handler, maxRetries: 1);

        var ex = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.TryGetBlobAsync(Hash).AsTask()
        );

        Assert.Contains("failed after 2 attempt(s)", ex.Message);
        Assert.IsType<HttpRequestException>(ex.InnerException);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task HttpRemote_429_is_transient_and_retried_to_success()
    {
        var handler = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.TooManyRequests),
            _ => OkBytes([7])
        );
        var provider = BuildHttpRemote(handler, maxRetries: 2);

        var result = await provider.TryGetBlobAsync(Hash);

        Assert.True(result.Found);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task HttpRemote_501_is_plain_transient_not_capability_declined()
    {
        // DRIFT PIN: unlike S3Direct, HttpRemote has no 501 carve-out — it is
        // >= 500, so it retries and (on exhaustion) throws like any 5xx.
        var retried = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.NotImplemented),
            _ => OkBytes([9])
        );
        var provider = BuildHttpRemote(retried, maxRetries: 1);
        var result = await provider.TryGetBlobAsync(Hash);
        Assert.True(result.Found);
        Assert.Equal(2, retried.CallCount);

        var exhausted = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.NotImplemented));
        var throwing = BuildHttpRemote(exhausted, maxRetries: 0);
        var ex = await Assert.ThrowsAsync<InvalidOperationException>(
            () => throwing.TryGetBlobAsync(Hash).AsTask()
        );
        Assert.Contains("returned transient status 501 after 1 attempt(s)", ex.Message);
    }

    [Fact]
    public async Task HttpRemote_404_counts_as_breaker_success_and_resets_consecutive_failures()
    {
        // DRIFT PIN: a non-transient response — even an error like 404 —
        // RESETS the breaker here (the origin is up and answering). The
        // delegated provider does the opposite (see the delegated half).
        var handler = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.InternalServerError), // call 1: failure #1
            _ => new HttpResponseMessage(HttpStatusCode.NotFound),            // call 2: resets
            _ => new HttpResponseMessage(HttpStatusCode.InternalServerError), // call 3: failure #1 again
            _ => new HttpResponseMessage(HttpStatusCode.InternalServerError)  // call 4: failure #2 -> opens
        );
        var provider = BuildHttpRemote(handler, maxRetries: 0, circuitThreshold: 2);

        await Assert.ThrowsAsync<InvalidOperationException>(() => provider.TryGetBlobAsync(Hash).AsTask());
        var missing = await provider.TryGetBlobAsync(Hash);
        Assert.False(missing.Found); // 404
        await Assert.ThrowsAsync<InvalidOperationException>(() => provider.TryGetBlobAsync(Hash).AsTask());

        // Breaker must still be CLOSED (the 404 reset the count): this call
        // reaches the network and is the second consecutive failure.
        await Assert.ThrowsAsync<InvalidOperationException>(() => provider.TryGetBlobAsync(Hash).AsTask());
        Assert.Equal(4, handler.CallCount);

        // NOW the breaker is open: GET degrades to a miss with no HTTP call.
        var degraded = await provider.TryGetBlobAsync(Hash);
        Assert.False(degraded.Found);
        Assert.Equal(4, handler.CallCount);
    }

    [Fact]
    public async Task HttpRemote_open_breaker_get_misses_but_exists_and_push_throw()
    {
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.InternalServerError));
        var provider = BuildHttpRemote(handler, maxRetries: 0, circuitThreshold: 1);

        await Assert.ThrowsAsync<InvalidOperationException>(() => provider.TryGetBlobAsync(Hash).AsTask());
        Assert.Equal(1, handler.CallCount);

        var degraded = await provider.TryGetBlobAsync(Hash);
        Assert.False(degraded.Found);

        var existsEx = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.ExistsAsync(Hash).AsTask()
        );
        Assert.Contains("short-circuited by open circuit breaker", existsEx.Message);

        var pushEx = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PushAsync(Hash, [1], null).AsTask()
        );
        Assert.Contains("short-circuited by open circuit breaker", pushEx.Message);

        Assert.Equal(1, handler.CallCount);
    }

    // ─────────────────────────────────────────────────────────────────────
    // S3DirectBlobStoreProvider — exhaustion returns null (degraded miss on
    // GET, "declined to presign" throw on PUT); 501 = capability declined
    // (breaker success + op=get-only cooldown).
    // ─────────────────────────────────────────────────────────────────────

    [Fact]
    public async Task S3Direct_get_501_returns_missing_and_caches_a_get_cooldown()
    {
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.NotImplemented));
        var provider = BuildS3Direct(handler, capabilityProbeCooldownSeconds: 60);

        var first = await provider.TryGetBlobAsync(Hash);
        Assert.False(first.Found);
        Assert.Equal(1, handler.CallCount);

        // Cooldown: the second read makes NO origin call at all.
        var second = await provider.TryGetBlobAsync(Hash);
        Assert.False(second.Found);
        Assert.Equal(1, handler.CallCount);

        // ExistsAsync shares the same cooldown gate.
        Assert.False(await provider.ExistsAsync(Hash));
        Assert.Equal(1, handler.CallCount);
    }

    [Fact]
    public async Task S3Direct_zero_cooldown_disables_the_501_cache()
    {
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.NotImplemented));
        var provider = BuildS3Direct(handler, capabilityProbeCooldownSeconds: 0);

        Assert.False((await provider.TryGetBlobAsync(Hash)).Found);
        Assert.False((await provider.TryGetBlobAsync(Hash)).Found);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task S3Direct_501_counts_as_breaker_success_and_put_501_is_never_cached()
    {
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.NotImplemented));
        // threshold 1: if a 501 counted as a breaker failure, the second call
        // below would be short-circuited without touching the network.
        var provider = BuildS3Direct(handler, maxRetries: 0, circuitThreshold: 1, capabilityProbeCooldownSeconds: 0);

        Assert.False((await provider.TryGetBlobAsync(Hash)).Found); // 501 -> breaker SUCCESS
        Assert.Equal(1, handler.CallCount);

        // PUT resolve still reaches the origin (breaker closed) and a
        // declined presign is a THROW on the put path (exhaustion mapping is
        // per-operation in this provider).
        var putEx = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1, 2, 3], null).AsTask()
        );
        Assert.Contains("declined to presign PUT", putEx.Message);
        Assert.Equal(2, handler.CallCount);

        // And op=put's 501 is deliberately never cached: a second put probes
        // the origin again.
        await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1, 2, 3], null).AsTask()
        );
        Assert.Equal(3, handler.CallCount);
    }

    [Fact]
    public async Task S3Direct_transient_exhaustion_is_a_degraded_miss_not_a_throw()
    {
        // DRIFT PIN: same classification as HttpRemote (5xx transient), but
        // the exhausted outcome maps to null -> Missing instead of throwing.
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.InternalServerError));
        var provider = BuildS3Direct(handler, maxRetries: 1, circuitThreshold: 1);

        var result = await provider.TryGetBlobAsync(Hash);
        Assert.False(result.Found);
        Assert.Equal(2, handler.CallCount);

        // The exhaustion recorded a breaker failure (threshold 1 -> open):
        // the next read short-circuits without a network call.
        var degraded = await provider.TryGetBlobAsync(Hash);
        Assert.False(degraded.Found);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task S3Direct_put_resolve_exhaustion_throws_declined_to_presign()
    {
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.InternalServerError));
        var provider = BuildS3Direct(handler, maxRetries: 1);

        var ex = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1], null).AsTask()
        );
        Assert.Contains("declined to presign PUT", ex.Message);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task S3Direct_unexpected_non_transient_status_throws_and_counts_as_breaker_success()
    {
        var handler = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.Forbidden),
            _ => new HttpResponseMessage(HttpStatusCode.Forbidden)
        );
        var provider = BuildS3Direct(handler, maxRetries: 0, circuitThreshold: 1);

        var ex = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.TryGetBlobAsync(Hash).AsTask()
        );
        Assert.Contains("unexpected status 403", ex.Message);

        // 403 recorded a breaker SUCCESS (the origin answered): the next call
        // must still reach the network despite threshold 1.
        await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.TryGetBlobAsync(Hash).AsTask()
        );
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task S3Direct_open_breaker_get_and_exists_miss_but_put_throws()
    {
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.InternalServerError));
        var provider = BuildS3Direct(handler, maxRetries: 0, circuitThreshold: 1);

        Assert.False((await provider.TryGetBlobAsync(Hash)).Found); // opens breaker
        Assert.Equal(1, handler.CallCount);

        Assert.False((await provider.TryGetBlobAsync(Hash)).Found);
        Assert.False(await provider.ExistsAsync(Hash));

        var putEx = await Assert.ThrowsAsync<InvalidOperationException>(
            () => provider.PutBlobAsync(Hash, [1], null).AsTask()
        );
        Assert.Contains("short-circuited by open breaker", putEx.Message);
        Assert.Equal(1, handler.CallCount);
    }

    // ─────────────────────────────────────────────────────────────────────
    // S3DelegatedBlobUrlResolverProvider (internal — driven through the real
    // DI composition + the legacy /sync/blob-url route) — exhaustion returns
    // null (route: 404); a non-transient failure is a breaker FAILURE and is
    // never retried; 501 is plain-transient.
    // ─────────────────────────────────────────────────────────────────────

    [Fact]
    public async Task Delegated_4xx_is_never_retried_and_records_a_breaker_failure()
    {
        // DRIFT PIN (the inverse of HttpRemote's 404 test): a non-transient
        // delegate answer counts AGAINST the breaker here. With threshold 2
        // and generous retries configured, two 404 responses open the
        // breaker; the third request makes no delegate call at all.
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.NotFound));
        await using var app = BuildDelegatedApp(handler, maxRetries: 3, circuitThreshold: 2);
        await app.StartAsync();
        var client = app.GetTestClient();

        var first = await client.GetAsync(SyncBlobUrl("get"));
        Assert.Equal(HttpStatusCode.NotFound, first.StatusCode);
        Assert.Equal(1, handler.CallCount); // no retry despite MaxRetries=3

        var second = await client.GetAsync(SyncBlobUrl("get"));
        Assert.Equal(HttpStatusCode.NotFound, second.StatusCode);
        Assert.Equal(2, handler.CallCount); // breaker now open

        var third = await client.GetAsync(SyncBlobUrl("get"));
        Assert.Equal(HttpStatusCode.NotFound, third.StatusCode);
        Assert.Equal(2, handler.CallCount); // short-circuited
    }

    [Fact]
    public async Task Delegated_501_is_plain_transient_and_retried_to_success()
    {
        // DRIFT PIN: no 501 capability carve-out here (unlike S3Direct) — it
        // is >= 500, so it retries and can succeed on the next attempt.
        var handler = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.NotImplemented),
            _ => JsonUrlResponse("https://upload.example/presigned")
        );
        await using var app = BuildDelegatedApp(handler, maxRetries: 2, circuitThreshold: 10);
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync(SyncBlobUrl("get"));
        Assert.Equal(HttpStatusCode.OK, response.StatusCode);
        Assert.Equal(2, handler.CallCount);
    }

    [Fact]
    public async Task Delegated_transient_exhaustion_returns_null_mapped_to_404()
    {
        var handler = new SequenceHandler(_ => new HttpResponseMessage(HttpStatusCode.InternalServerError));
        await using var app = BuildDelegatedApp(handler, maxRetries: 2, circuitThreshold: 10);
        await app.StartAsync();
        var client = app.GetTestClient();

        var response = await client.GetAsync(SyncBlobUrl("get"));
        Assert.Equal(HttpStatusCode.NotFound, response.StatusCode);
        Assert.Equal(3, handler.CallCount); // 1 + 2 retries, then null (never a throw)
    }

    [Fact]
    public async Task Delegated_200_with_null_json_body_records_a_breaker_failure()
    {
        // Pin the quirk exactly as implemented: a 200 whose JSON body is the
        // literal `null` parses to a null DelegatedUrlResponse, which the
        // provider counts as a breaker FAILURE (threshold 1 -> open), not a
        // success — even though the delegate answered 200.
        var handler = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.OK)
            {
                Content = new StringContent("null", System.Text.Encoding.UTF8, "application/json")
            }
        );
        await using var app = BuildDelegatedApp(handler, maxRetries: 0, circuitThreshold: 1);
        await app.StartAsync();
        var client = app.GetTestClient();

        var first = await client.GetAsync(SyncBlobUrl("get"));
        Assert.Equal(HttpStatusCode.NotFound, first.StatusCode);
        Assert.Equal(1, handler.CallCount);

        // Breaker opened by the parse-null "failure": no second delegate call.
        var second = await client.GetAsync(SyncBlobUrl("get"));
        Assert.Equal(HttpStatusCode.NotFound, second.StatusCode);
        Assert.Equal(1, handler.CallCount);
    }

    [Fact]
    public async Task Delegated_success_resets_the_breaker()
    {
        var handler = new SequenceHandler(
            _ => new HttpResponseMessage(HttpStatusCode.InternalServerError), // failure #1
            _ => JsonUrlResponse("https://upload.example/presigned"),         // resets
            _ => new HttpResponseMessage(HttpStatusCode.InternalServerError), // failure #1 again
            _ => new HttpResponseMessage(HttpStatusCode.InternalServerError)  // failure #2 -> opens
        );
        await using var app = BuildDelegatedApp(handler, maxRetries: 0, circuitThreshold: 2);
        await app.StartAsync();
        var client = app.GetTestClient();

        Assert.Equal(HttpStatusCode.NotFound, (await client.GetAsync(SyncBlobUrl("get"))).StatusCode);
        Assert.Equal(HttpStatusCode.OK, (await client.GetAsync(SyncBlobUrl("get"))).StatusCode);
        Assert.Equal(HttpStatusCode.NotFound, (await client.GetAsync(SyncBlobUrl("get"))).StatusCode);

        // Breaker still closed (the success reset the count): call 4 reaches
        // the delegate and is consecutive failure #2.
        Assert.Equal(HttpStatusCode.NotFound, (await client.GetAsync(SyncBlobUrl("get"))).StatusCode);
        Assert.Equal(4, handler.CallCount);

        // Open now: no further delegate call.
        Assert.Equal(HttpStatusCode.NotFound, (await client.GetAsync(SyncBlobUrl("get"))).StatusCode);
        Assert.Equal(4, handler.CallCount);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Builders / fakes
    // ─────────────────────────────────────────────────────────────────────

    private static HttpRemoteBlobStoreProvider BuildHttpRemote(
        HttpMessageHandler handler,
        int maxRetries = 2,
        int circuitThreshold = 3
    )
    {
        var client = new HttpClient(handler) { BaseAddress = new Uri(OriginBaseUrl) };
        var options = new RemoteBlobOriginOptions(
            BaseUrl: OriginBaseUrl,
            AuthToken: null,
            TimeoutSeconds: 30,
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitThreshold,
            CircuitBreakerOpenSeconds: 60
        );
        return new HttpRemoteBlobStoreProvider(
            new SingleClientFactory(client),
            options,
            NullLogger<HttpRemoteBlobStoreProvider>.Instance
        );
    }

    private static S3DirectBlobStoreProvider BuildS3Direct(
        HttpMessageHandler handler,
        int maxRetries = 2,
        int circuitThreshold = 3,
        int capabilityProbeCooldownSeconds = 60
    )
    {
        var client = new HttpClient(handler) { BaseAddress = new Uri(OriginBaseUrl) };
        var originOptions = new RemoteBlobOriginOptions(
            BaseUrl: OriginBaseUrl,
            AuthToken: null,
            TimeoutSeconds: 30,
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitThreshold,
            CircuitBreakerOpenSeconds: 60
        );
        var s3Options = new S3DirectBlobOriginOptions(
            Enabled: true,
            TimeoutSeconds: 30,
            MaxRetries: maxRetries,
            CircuitBreakerFailureThreshold: circuitThreshold,
            CircuitBreakerOpenSeconds: 60,
            CapabilityProbeCooldownSeconds: capabilityProbeCooldownSeconds,
            Compression: "Off",
            CompressionLevel: 3,
            CompressionMinBytes: 4096
        );
        return new S3DirectBlobStoreProvider(
            new SingleClientFactory(client),
            originOptions,
            s3Options,
            NullLogger<S3DirectBlobStoreProvider>.Instance
        );
    }

    private static WebApplication BuildDelegatedApp(
        HttpMessageHandler handler,
        int maxRetries,
        int circuitThreshold
    )
    {
        var client = new HttpClient(handler) { BaseAddress = new Uri("https://delegate.test") };
        return HostApplication.Build(
            [],
            configureServices: services =>
            {
                services.AddSingleton<IHttpClientFactory>(new SingleClientFactory(client));
            },
            configureWebHost: webHost => webHost.UseTestServer(),
            configureConfiguration: config =>
            {
                config.AddInMemoryCollection(
                    new Dictionary<string, string?>
                    {
                        ["NodalMerge:Providers:BlobStorage"] = "S3Delegated",
                        ["NodalMerge:Storage:S3Delegated:BaseUrl"] = "https://delegate.test",
                        ["NodalMerge:Storage:S3Delegated:MaxRetries"] = maxRetries.ToString(),
                        ["NodalMerge:Storage:S3Delegated:CircuitBreakerFailureThreshold"] = circuitThreshold.ToString(),
                        ["NodalMerge:Storage:S3Delegated:CircuitBreakerOpenSeconds"] = "60"
                    }
                );
            }
        );
    }

    private static string SyncBlobUrl(string op) =>
        $"/sync/blob-url?op={op}&room=room-a&namespace=assets&hash={CanonicalHash}";

    private static HttpResponseMessage OkBytes(byte[] bytes) =>
        new(HttpStatusCode.OK) { Content = new ByteArrayContent(bytes) };

    private static HttpResponseMessage JsonUrlResponse(string url) =>
        new(HttpStatusCode.OK)
        {
            Content = new StringContent(
                $"{{\"url\":\"{url}\"}}",
                System.Text.Encoding.UTF8,
                "application/json"
            )
        };

    private sealed class SingleClientFactory(HttpClient client) : IHttpClientFactory
    {
        public HttpClient CreateClient(string name) => client;
    }

    /// <summary>
    /// Responds from an ordered list of factories; the LAST factory repeats
    /// forever (so a single-element sequence behaves like "always answer X").
    /// </summary>
    private sealed class SequenceHandler(params Func<HttpRequestMessage, HttpResponseMessage>[] responders)
        : HttpMessageHandler
    {
        public int CallCount { get; private set; }

        protected override Task<HttpResponseMessage> SendAsync(
            HttpRequestMessage request,
            CancellationToken cancellationToken
        )
        {
            var index = Math.Min(CallCount, responders.Length - 1);
            CallCount++;
            return Task.FromResult(responders[index](request));
        }
    }
}
