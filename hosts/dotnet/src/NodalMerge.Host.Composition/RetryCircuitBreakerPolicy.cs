namespace NodalMerge.Host.Composition;

/// <summary>
/// Slice 7.2 (nodalmerge-studio/plans/blob-cas-remediation.md) — the ONE
/// retry-classification + circuit-breaker component that was previously
/// copied (and drifting) three times across
/// <see cref="HttpRemoteBlobStoreProvider"/>,
/// <see cref="S3DirectBlobStoreProvider"/> and
/// <c>S3DelegatedBlobUrlResolverProvider</c>.
///
/// It lives in Composition (not Host.Abstractions) deliberately: all three
/// consumers are Composition-internal provider implementations, and
/// Abstractions is the frozen provider contract surface — retry plumbing is
/// an implementation detail, not contract.
///
/// ## What is shared (identical in all three pre-extraction copies)
/// - The attempt loop: <c>MaxRetries + 1</c> total attempts, no backoff
///   delay between attempts (immediate retry — pinned behavior, do not add
///   sleeps here without a slice).
/// - Exception classification: <see cref="TaskCanceledException"/> when the
///   caller did NOT cancel (i.e. an HttpClient timeout) and
///   <see cref="HttpRequestException"/> are transient.
/// - Status classification: <c>&gt;= 500</c> or <c>429</c> is transient;
///   everything else is a final answer for this call.
/// - The breaker: <see cref="RecordFailure"/> increments a consecutive
///   counter under a lock and opens the circuit for
///   <c>CircuitBreakerOpenSeconds</c> once it reaches
///   <c>CircuitBreakerFailureThreshold</c>; <see cref="RecordSuccess"/>
///   resets both; <see cref="IsCircuitOpen"/> lazily closes an expired
///   circuit and resets the counter.
///
/// ## What is deliberately NOT unified (the drift, now visible as options)
/// The three providers configure three DIFFERENT option sets — that
/// difference is pre-existing behavior this slice must preserve, not smooth
/// over:
/// - <see cref="RetryCircuitBreakerPolicyOptions.TreatNotImplementedAsCapabilityDeclined"/>:
///   only S3Direct carves 501 out as "capability declined" (breaker SUCCESS,
///   never retried, surfaced as <see cref="RetrySendOutcomeKind.CapabilityDeclined"/>
///   so the caller can run its op=get cooldown). HttpRemote and S3Delegated
///   treat 501 as any other 5xx.
/// - <see cref="RetryCircuitBreakerPolicyOptions.NonTransientResponseHandling"/>:
///   HttpRemote and S3Direct record a breaker SUCCESS for ANY non-transient
///   response (even a 4xx — the origin answered); S3Delegated defers that
///   verdict to the caller (<see cref="NonTransientResponseHandling.CallerDecides"/>),
///   because its verdict depends on the response BODY: 2xx with a parseable
///   URL is a success, but a 4xx — and even a 2xx whose JSON parses to
///   null — counts as a breaker FAILURE there.
/// - The exhausted outcome is exposed (<see cref="RetrySendOutcomeKind.Exhausted"/>),
///   never mapped here: HttpRemote THROWS on exhaustion; S3Direct and
///   S3Delegated return null/miss. That mapping stays per-caller.
///
/// All of the above is pinned by RetryCircuitBreakerCharacterizationTests,
/// written against the pre-extraction copies and kept green across the
/// extraction.
/// </summary>
public sealed class RetryCircuitBreakerPolicy
{
    private readonly RetryCircuitBreakerPolicyOptions _options;
    private readonly object _circuitLock = new();

    private int _consecutiveFailures;
    private DateTimeOffset? _circuitOpenedUntil;

    public RetryCircuitBreakerPolicy(RetryCircuitBreakerPolicyOptions options)
    {
        _options = options;
    }

    /// <summary>
    /// The shared transient-status classification: 5xx or 429. (501 is
    /// subject to the capability carve-out BEFORE this check when
    /// <see cref="RetryCircuitBreakerPolicyOptions.TreatNotImplementedAsCapabilityDeclined"/>
    /// is set.)
    /// </summary>
    public static bool IsTransientStatus(int statusCode) => statusCode >= 500 || statusCode == 429;

    /// <summary>
    /// Runs one logical request with the shared retry loop and breaker
    /// accounting. <paramref name="sendAttempt"/> receives the 1-based
    /// attempt number (some callers log it) and must perform ONE HTTP send;
    /// this method owns classification, disposal of transient responses,
    /// and breaker recording per the configured options. A
    /// <see cref="TaskCanceledException"/> caused by the caller's own
    /// <paramref name="cancellationToken"/> propagates unchanged.
    /// </summary>
    public async Task<RetrySendOutcome> SendWithRetryAsync(
        Func<int, CancellationToken, Task<HttpResponseMessage>> sendAttempt,
        CancellationToken cancellationToken
    )
    {
        var maxAttempts = _options.MaxRetries + 1;

        for (var attempt = 1; attempt <= maxAttempts; attempt++)
        {
            HttpResponseMessage response;
            try
            {
                response = await sendAttempt(attempt, cancellationToken);
            }
            catch (TaskCanceledException ex) when (!cancellationToken.IsCancellationRequested)
            {
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    return RetrySendOutcome.Exhausted(RetryFailureKind.Timeout, ex, lastStatus: null, attempt);
                }
                continue;
            }
            catch (HttpRequestException ex)
            {
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    return RetrySendOutcome.Exhausted(RetryFailureKind.ConnectionFailure, ex, lastStatus: null, attempt);
                }
                continue;
            }

            var statusCode = (int)response.StatusCode;

            if (_options.TreatNotImplementedAsCapabilityDeclined && statusCode == 501)
            {
                response.Dispose();
                // The origin answered (it's healthy) — it's just declining
                // the capability, which is not a breaker failure.
                RecordSuccess();
                return RetrySendOutcome.CapabilityDeclined(attempt);
            }

            if (IsTransientStatus(statusCode))
            {
                response.Dispose();
                if (attempt == maxAttempts)
                {
                    RecordFailure();
                    return RetrySendOutcome.Exhausted(RetryFailureKind.TransientStatus, exception: null, statusCode, attempt);
                }
                continue;
            }

            if (_options.NonTransientResponseHandling == NonTransientResponseHandling.RecordBreakerSuccess)
            {
                RecordSuccess();
            }

            return RetrySendOutcome.NonTransientResponse(response, attempt);
        }

        // Unreachable: the loop above always returns on its last iteration.
        throw new InvalidOperationException("retry loop exited unexpectedly");
    }

    public bool IsCircuitOpen()
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

    /// <summary>
    /// Public because <see cref="NonTransientResponseHandling.CallerDecides"/>
    /// callers (the delegated resolver) record the breaker verdict for a
    /// non-transient response themselves, after inspecting the body.
    /// </summary>
    public void RecordSuccess()
    {
        lock (_circuitLock)
        {
            _consecutiveFailures = 0;
            _circuitOpenedUntil = null;
        }
    }

    /// <inheritdoc cref="RecordSuccess"/>
    public void RecordFailure()
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

/// <summary>
/// Options for <see cref="RetryCircuitBreakerPolicy"/>. Each provider builds
/// its own instance from its own config record — the three option sets are
/// intentionally different where the pre-extraction copies drifted (see the
/// policy's class doc).
/// </summary>
public sealed record RetryCircuitBreakerPolicyOptions(
    int MaxRetries,
    int CircuitBreakerFailureThreshold,
    int CircuitBreakerOpenSeconds,
    bool TreatNotImplementedAsCapabilityDeclined,
    NonTransientResponseHandling NonTransientResponseHandling
);

/// <summary>
/// What the policy does with a response it classified as non-transient
/// (anything that is not 5xx/429, after the optional 501 carve-out).
/// </summary>
public enum NonTransientResponseHandling
{
    /// <summary>
    /// Any answered response — including 4xx — resets the breaker (the
    /// origin is up and answering). HttpRemote + S3Direct.
    /// </summary>
    RecordBreakerSuccess,

    /// <summary>
    /// The policy records nothing; the caller inspects the response (status
    /// AND body) and calls <see cref="RetryCircuitBreakerPolicy.RecordSuccess"/> /
    /// <see cref="RetryCircuitBreakerPolicy.RecordFailure"/> itself.
    /// S3Delegated: 4xx and even a 2xx-with-null-JSON-body are breaker
    /// failures there.
    /// </summary>
    CallerDecides
}

public enum RetrySendOutcomeKind
{
    /// <summary>A non-transient response arrived; <see cref="RetrySendOutcome.Response"/> is set.</summary>
    NonTransientResponse,

    /// <summary>
    /// 501 under <see cref="RetryCircuitBreakerPolicyOptions.TreatNotImplementedAsCapabilityDeclined"/>:
    /// the origin declined the capability (breaker success already recorded).
    /// </summary>
    CapabilityDeclined,

    /// <summary>
    /// Every attempt failed transiently (breaker failure already recorded);
    /// the last failure's kind/status/exception are exposed for the caller's
    /// own exhaustion mapping (throw vs null stays per-provider).
    /// </summary>
    Exhausted
}

public enum RetryFailureKind
{
    /// <summary>HttpClient timeout (<see cref="TaskCanceledException"/> without caller cancellation).</summary>
    Timeout,

    /// <summary>Transport failure (<see cref="HttpRequestException"/>).</summary>
    ConnectionFailure,

    /// <summary>A transient HTTP status (5xx/429).</summary>
    TransientStatus
}

public sealed class RetrySendOutcome
{
    private RetrySendOutcome(
        RetrySendOutcomeKind kind,
        HttpResponseMessage? response,
        RetryFailureKind? lastFailureKind,
        Exception? lastException,
        int? lastTransientStatus,
        int attempts
    )
    {
        Kind = kind;
        Response = response;
        LastFailureKind = lastFailureKind;
        LastException = lastException;
        LastTransientStatus = lastTransientStatus;
        Attempts = attempts;
    }

    public RetrySendOutcomeKind Kind { get; }

    /// <summary>Set only for <see cref="RetrySendOutcomeKind.NonTransientResponse"/>; ownership transfers to the caller.</summary>
    public HttpResponseMessage? Response { get; }

    /// <summary>Set only for <see cref="RetrySendOutcomeKind.Exhausted"/>.</summary>
    public RetryFailureKind? LastFailureKind { get; }

    /// <summary>The last transient exception (timeout/connection kinds), for exhaustion messages/logs.</summary>
    public Exception? LastException { get; }

    /// <summary>The last transient HTTP status (<see cref="RetryFailureKind.TransientStatus"/> only).</summary>
    public int? LastTransientStatus { get; }

    /// <summary>How many attempts were actually made (== MaxRetries + 1 on exhaustion).</summary>
    public int Attempts { get; }

    internal static RetrySendOutcome NonTransientResponse(HttpResponseMessage response, int attempts) =>
        new(RetrySendOutcomeKind.NonTransientResponse, response, null, null, null, attempts);

    internal static RetrySendOutcome CapabilityDeclined(int attempts) =>
        new(RetrySendOutcomeKind.CapabilityDeclined, null, null, null, null, attempts);

    internal static RetrySendOutcome Exhausted(
        RetryFailureKind kind,
        Exception? exception,
        int? lastStatus,
        int attempts
    ) => new(RetrySendOutcomeKind.Exhausted, null, kind, exception, lastStatus, attempts);
}
