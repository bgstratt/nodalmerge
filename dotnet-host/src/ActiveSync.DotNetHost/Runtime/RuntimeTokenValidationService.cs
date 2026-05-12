using ActiveSync.Host.Abstractions.Providers;
using System.Text;
using System.Text.Json;

namespace ActiveSync.DotNetHost.Runtime;

public sealed class RuntimeTokenValidationService
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };

    private readonly IRoomTokenAuthProvider _tokenAuthProvider;

    public RuntimeTokenValidationService(IRoomTokenAuthProvider tokenAuthProvider)
    {
        _tokenAuthProvider = tokenAuthProvider;
    }

    public async ValueTask<RuntimeTokenValidationOutcome> ValidateInboundAsync(
        byte[] payload,
        RuntimeConnectionState state,
        CancellationToken cancellationToken = default
    )
    {
        var requestResult = BuildRequest(payload, state);
        if (!requestResult.ShouldValidate)
        {
            return RuntimeTokenValidationOutcome.AllowedResult;
        }

        if (requestResult.Request is null)
        {
            return RuntimeTokenValidationOutcome.Denied(
                requestResult.ErrorMessage ?? "invalid token"
            );
        }

        var validation = await _tokenAuthProvider.ValidateAsync(requestResult.Request, cancellationToken);
        if (validation.IsValid)
        {
            return RuntimeTokenValidationOutcome.AllowedResult;
        }

        var reason = string.IsNullOrWhiteSpace(validation.Reason)
            ? "token rejected"
            : $"token rejected: {validation.Reason}";
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
