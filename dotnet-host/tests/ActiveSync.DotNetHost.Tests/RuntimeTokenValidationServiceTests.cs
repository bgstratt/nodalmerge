using ActiveSync.DotNetHost.Runtime;
using ActiveSync.Host.Abstractions.Providers;
using System.Diagnostics.Metrics;

namespace ActiveSync.DotNetHost.Tests;

public sealed class RuntimeTokenValidationServiceTests
{
    [Fact]
    public async Task ValidateInboundAsync_allows_when_no_token_present()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.True(result.Allowed);
        Assert.Null(result.ErrorMessage);
    }

    [Fact]
    public async Task ValidateInboundAsync_denies_when_token_missing_sig()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000}}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.False(result.Allowed);
        Assert.Equal("hello.token.sig is required", result.ErrorMessage);
    }

    [Fact]
    public async Task ValidateInboundAsync_denies_when_provider_rejects_token()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Invalid("expired"))
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.False(result.Allowed);
        Assert.Equal("token rejected: expired", result.ErrorMessage);
    }

    [Fact]
    public async Task ValidateInboundAsync_allows_when_provider_accepts_token()
    {
        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );

        var state = new RuntimeConnectionState(7);
        var payload = "{\"type\":\"client-hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"u8.ToArray();

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.True(result.Allowed);
        Assert.Null(result.ErrorMessage);
    }

    [Fact]
    public async Task ValidateInboundAsync_emits_runtime_auth_metrics_for_allowed_and_denied_paths()
    {
        using var metrics = new MeterCapture("ActiveSync.DotNetHost.RuntimeAuth", "runtime_auth_validation_total");

        var allowService = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );
        var denyService = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Invalid("expired"))
        );

        var state = new RuntimeConnectionState(77);
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"u8.ToArray();

        var beforeAllowed = metrics.GetTotal("allowed");
        var beforeDenied = metrics.GetTotal("denied");

        await allowService.ValidateInboundAsync(payload, state, CancellationToken.None);
        await denyService.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.True(metrics.GetTotal("allowed") >= beforeAllowed + 1);
        Assert.True(metrics.GetTotal("denied") >= beforeDenied + 1);
    }

    [Fact]
    public async Task ValidateInboundAsync_promotes_trace_id_from_payload_to_state_and_metrics()
    {
        using var metrics = new MeterCapture("ActiveSync.DotNetHost.RuntimeAuth", "runtime_auth_validation_total");

        var service = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Valid)
        );

        var state = new RuntimeConnectionState(99) { TraceId = "trace-old" };
        var payload = "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"trace_id\":\"trace-new\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"u8.ToArray();

        var beforeAllowedTrace = metrics.GetTotalByOutcomeAndTrace("allowed", "trace-new");

        var result = await service.ValidateInboundAsync(payload, state, CancellationToken.None);

        Assert.True(result.Allowed);
        Assert.Equal("trace-new", state.TraceId);
        Assert.True(metrics.GetTotalByOutcomeAndTrace("allowed", "trace-new") >= beforeAllowedTrace + 1);
    }

    [Fact]
    public async Task Simulated_auth_spike_emits_denied_reason_breakdown()
    {
        using var metrics = new MeterCapture("ActiveSync.DotNetHost.RuntimeAuth", "runtime_auth_validation_total");

        var expiredService = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Invalid("expired"))
        );
        var capsService = new RuntimeTokenValidationService(
            new RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult.Invalid("capability mismatch"))
        );

        var stateExpired = new RuntimeConnectionState(201) { RoomId = "room-auth" };
        var stateCaps = new RuntimeConnectionState(202) { RoomId = "room-auth" };
        var payload = "{\"type\":\"hello\",\"room\":\"room-auth\",\"pubkey\":\"peer-a\",\"trace_id\":\"trace-auth\",\"frontier\":[],\"token\":{\"peer_pubkey\":\"peer-a\",\"expiry\":1700000000,\"caps\":[\"read:**\"],\"sig\":\"beef\"}}"u8.ToArray();

        var beforeExpired = metrics.GetTotalByTag("reason", "expired");
        var beforeCaps = metrics.GetTotalByTag("reason", "capability mismatch");

        for (var i = 0; i < 8; i++)
        {
            await expiredService.ValidateInboundAsync(payload, stateExpired, CancellationToken.None);
            await capsService.ValidateInboundAsync(payload, stateCaps, CancellationToken.None);
        }

        Assert.True(metrics.GetTotalByTag("reason", "expired") >= beforeExpired + 8);
        Assert.True(metrics.GetTotalByTag("reason", "capability mismatch") >= beforeCaps + 8);
    }
}

internal sealed class RuntimeTokenValidationAuthProviderStub : IRoomTokenAuthProvider
{
    private readonly RoomTokenValidationResult _validationResult;

    public RuntimeTokenValidationAuthProviderStub(RoomTokenValidationResult validationResult)
    {
        _validationResult = validationResult;
    }

    public ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(_validationResult);
    }

    public ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    )
    {
        return ValueTask.FromResult(RoomTokenMintResult.NotSupported);
    }
}

internal sealed class MeterCapture : IDisposable
{
    private readonly MeterListener _listener;
    private readonly string _instrumentName;
    private readonly Dictionary<string, long> _totals = new(StringComparer.Ordinal);
    private readonly Dictionary<string, long> _totalsByOutcomeAndTrace = new(StringComparer.Ordinal);
    private readonly Dictionary<string, long> _totalsByTag = new(StringComparer.Ordinal);

    public MeterCapture(string meterName, string instrumentName)
    {
        _instrumentName = instrumentName;
        _listener = new MeterListener
        {
            InstrumentPublished = (instrument, listener) =>
            {
                if (string.Equals(instrument.Meter.Name, meterName, StringComparison.Ordinal)
                    && string.Equals(instrument.Name, instrumentName, StringComparison.Ordinal))
                {
                    listener.EnableMeasurementEvents(instrument);
                }
            }
        };

        _listener.SetMeasurementEventCallback<long>((_, measurement, tags, _) =>
        {
            var outcome = "unknown";
            var trace = "<none>";
            foreach (var tag in tags)
            {
                if (string.Equals(tag.Key, "outcome", StringComparison.Ordinal)
                    && tag.Value is string value
                    && !string.IsNullOrWhiteSpace(value))
                {
                    outcome = value;
                    continue;
                }

                if (string.Equals(tag.Key, "trace", StringComparison.Ordinal)
                    && tag.Value is string traceValue
                    && !string.IsNullOrWhiteSpace(traceValue))
                {
                    trace = traceValue;
                }
            }

            if (_totals.TryGetValue(outcome, out var current))
            {
                _totals[outcome] = current + measurement;
            }
            else
            {
                _totals[outcome] = measurement;
            }

            var outcomeTraceKey = $"{outcome}|{trace}";
            if (_totalsByOutcomeAndTrace.TryGetValue(outcomeTraceKey, out var byTraceCurrent))
            {
                _totalsByOutcomeAndTrace[outcomeTraceKey] = byTraceCurrent + measurement;
            }
            else
            {
                _totalsByOutcomeAndTrace[outcomeTraceKey] = measurement;
            }

            foreach (var tag in tags)
            {
                if (string.IsNullOrWhiteSpace(tag.Key))
                {
                    continue;
                }

                var value = tag.Value?.ToString();
                if (string.IsNullOrWhiteSpace(value))
                {
                    continue;
                }

                var tagKey = $"{tag.Key}|{value}";
                if (_totalsByTag.TryGetValue(tagKey, out var byTagCurrent))
                {
                    _totalsByTag[tagKey] = byTagCurrent + measurement;
                }
                else
                {
                    _totalsByTag[tagKey] = measurement;
                }
            }
        });

        _listener.Start();
    }

    public long GetTotal(string outcome)
    {
        return _totals.TryGetValue(outcome, out var total) ? total : 0;
    }

    public long GetTotalByOutcomeAndTrace(string outcome, string trace)
    {
        var key = $"{outcome}|{trace}";
        return _totalsByOutcomeAndTrace.TryGetValue(key, out var total) ? total : 0;
    }

    public long GetTotalByTag(string tagKey, string tagValue)
    {
        var key = $"{tagKey}|{tagValue}";
        return _totalsByTag.TryGetValue(key, out var total) ? total : 0;
    }

    public void Dispose()
    {
        _listener.Dispose();
    }
}
