using ActiveSync.Host.Abstractions.Providers;
using Microsoft.Extensions.Logging;
using System.Diagnostics.Metrics;
using System.Text;
using System.Text.Json;

namespace ActiveSync.DotNetHost.Runtime;

public sealed class RuntimeTokenValidationService
{
    private static readonly Meter RuntimeAuthMeter = new("ActiveSync.DotNetHost.RuntimeAuth", "1.0.0");
    private static readonly Counter<long> RuntimeAuthValidationCounter = RuntimeAuthMeter.CreateCounter<long>(
        "runtime_auth_validation_total"
    );
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };

    private readonly IRoomTokenAuthProvider _tokenAuthProvider;
    private readonly ILogger<RuntimeTokenValidationService>? _logger;

    public RuntimeTokenValidationService(
        IRoomTokenAuthProvider tokenAuthProvider,
        ILogger<RuntimeTokenValidationService>? logger = null)
    {
        _tokenAuthProvider = tokenAuthProvider;
        _logger = logger;
    }

    public async ValueTask<RuntimeTokenValidationOutcome> ValidateInboundAsync(
        byte[] payload,
        RuntimeConnectionState state,
        CancellationToken cancellationToken = default
    )
    {
        var stateTraceId = EnsureStateTraceId(state);
        var requestResult = BuildRequest(payload, state);
        stateTraceId = EnsureStateTraceId(state);
        if (!requestResult.ShouldValidate)
        {
            RuntimeAuthValidationCounter.Add(
                1,
                KeyValuePair.Create<string, object?>("outcome", "skipped"),
                KeyValuePair.Create<string, object?>("room", state.RoomId ?? "<none>"),
                KeyValuePair.Create<string, object?>("trace", stateTraceId)
            );
            return RuntimeTokenValidationOutcome.AllowedResult;
        }

        if (requestResult.Request is null)
        {
            RuntimeAuthValidationCounter.Add(
                1,
                KeyValuePair.Create<string, object?>("outcome", "denied"),
                KeyValuePair.Create<string, object?>("reason", requestResult.ErrorMessage ?? "invalid token"),
                KeyValuePair.Create<string, object?>("room", state.RoomId ?? "<none>"),
                KeyValuePair.Create<string, object?>("trace", stateTraceId)
            );
            _logger?.LogInformation(
                "runtime auth validation outcome=denied reason={Reason} session={SessionId} trace={Trace}",
                requestResult.ErrorMessage ?? "invalid token",
                state.SessionId,
                stateTraceId
            );
            return RuntimeTokenValidationOutcome.Denied(
                requestResult.ErrorMessage ?? "invalid token"
            );
        }

        var validation = await _tokenAuthProvider.ValidateAsync(requestResult.Request, cancellationToken);
        if (validation.IsValid)
        {
            RuntimeAuthValidationCounter.Add(
                1,
                KeyValuePair.Create<string, object?>("outcome", "allowed"),
                KeyValuePair.Create<string, object?>("room", requestResult.Request.RoomId),
                KeyValuePair.Create<string, object?>("trace", stateTraceId)
            );
            _logger?.LogDebug(
                "runtime auth validation outcome=allowed session={SessionId} room={Room} trace={Trace}",
                state.SessionId,
                requestResult.Request.RoomId,
                stateTraceId
            );
            return RuntimeTokenValidationOutcome.AllowedResult;
        }

        var reason = string.IsNullOrWhiteSpace(validation.Reason)
            ? "token rejected"
            : $"token rejected: {validation.Reason}";
        RuntimeAuthValidationCounter.Add(
            1,
            KeyValuePair.Create<string, object?>("outcome", "denied"),
            KeyValuePair.Create<string, object?>("reason", validation.Reason ?? "token rejected"),
            KeyValuePair.Create<string, object?>("room", requestResult.Request.RoomId),
            KeyValuePair.Create<string, object?>("trace", stateTraceId)
        );
        _logger?.LogInformation(
            "runtime auth validation outcome=denied reason={Reason} session={SessionId} room={Room} trace={Trace}",
            validation.Reason ?? "token rejected",
            state.SessionId,
            requestResult.Request.RoomId,
            stateTraceId
        );
        return RuntimeTokenValidationOutcome.Denied(reason);
    }

    internal static RuntimeTokenRequestBuildResult BuildRequest(byte[] payload, RuntimeConnectionState state)
    {
        RuntimeInboundMessage? message;
        try
        {
            var incomingJson = Encoding.UTF8.GetString(payload);
            message = JsonSerializer.Deserialize<RuntimeInboundMessage>(incomingJson, JsonOptions);
        }
        catch
        {
            return RuntimeTokenRequestBuildResult.NoValidation;
        }

        if (message is null || string.IsNullOrWhiteSpace(message.Type))
        {
            return RuntimeTokenRequestBuildResult.NoValidation;
        }

        if (!string.IsNullOrWhiteSpace(message.TraceId))
        {
            state.TraceId = message.TraceId;
        }

        var type = message.Type.Trim();
        if (!string.Equals(type, "hello", StringComparison.OrdinalIgnoreCase)
            && !string.Equals(type, "client-hello", StringComparison.OrdinalIgnoreCase))
        {
            return RuntimeTokenRequestBuildResult.NoValidation;
        }

        var token = message.Token;
        if (token is null)
        {
            return RuntimeTokenRequestBuildResult.NoValidation;
        }

        var tokenPeer = token.GetPeerPubkey();
        if (string.IsNullOrWhiteSpace(tokenPeer))
        {
            return RuntimeTokenRequestBuildResult.Invalid("hello.token.peer_pubkey is required");
        }

        var tokenExpiry = token.GetExpiry();
        if (tokenExpiry is null)
        {
            return RuntimeTokenRequestBuildResult.Invalid("hello.token.expiry is required");
        }

        var tokenSig = token.GetSignature();
        if (string.IsNullOrWhiteSpace(tokenSig))
        {
            return RuntimeTokenRequestBuildResult.Invalid("hello.token.sig is required");
        }

        var room = message.Room;
        if (string.IsNullOrWhiteSpace(room))
        {
            room = state.RoomId;
        }

        if (string.IsNullOrWhiteSpace(room))
        {
            return RuntimeTokenRequestBuildResult.Invalid("hello.room is required");
        }

        var request = new RoomTokenValidationRequest(
            room,
            tokenPeer,
            (long)tokenExpiry.Value,
            token.GetCapabilities() ?? [],
            tokenSig
        );

        return RuntimeTokenRequestBuildResult.Valid(request);
    }

    private static string EnsureStateTraceId(RuntimeConnectionState state)
    {
        if (string.IsNullOrWhiteSpace(state.TraceId))
        {
            state.TraceId = $"sess-{state.SessionId}-{Guid.NewGuid():N}";
        }

        return state.TraceId;
    }
}

public sealed record RuntimeTokenValidationOutcome(bool Allowed, string? ErrorMessage)
{
    public static RuntimeTokenValidationOutcome AllowedResult { get; } = new(true, null);

    public static RuntimeTokenValidationOutcome Denied(string errorMessage) =>
        new(false, errorMessage);
}

internal sealed record RuntimeTokenRequestBuildResult(
    bool ShouldValidate,
    RoomTokenValidationRequest? Request,
    string? ErrorMessage
)
{
    public static RuntimeTokenRequestBuildResult NoValidation { get; } =
        new(false, null, null);

    public static RuntimeTokenRequestBuildResult Valid(RoomTokenValidationRequest request) =>
        new(true, request, null);

    public static RuntimeTokenRequestBuildResult Invalid(string error) =>
        new(true, null, error);
}
