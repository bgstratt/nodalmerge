namespace ActiveSync.Host.Abstractions.Providers;

public interface IRoomTokenAuthProvider
{
    ValueTask<RoomTokenValidationResult> ValidateAsync(
        RoomTokenValidationRequest request,
        CancellationToken cancellationToken = default
    );

    ValueTask<RoomTokenMintResult> MintAsync(
        RoomTokenMintRequest request,
        CancellationToken cancellationToken = default
    );
}

public sealed record RoomTokenValidationRequest(
    string RoomId,
    string PeerPubkeyHex,
    long ExpiryUnixSeconds,
    IReadOnlyList<string> Capabilities,
    string SignatureHex
);

public sealed record RoomTokenValidationResult(bool IsValid, string? Reason)
{
    public static RoomTokenValidationResult Valid { get; } = new(true, null);

    public static RoomTokenValidationResult Invalid(string reason) => new(false, reason);
}

public sealed record RoomTokenMintRequest(
    string RoomId,
    string PeerPubkeyHex,
    int? LifetimeSeconds,
    IReadOnlyList<string>? RequestedCapabilities
);

public sealed record RoomTokenMintResult(
    bool IsSupported,
    string PeerPubkeyHex,
    long ExpiryUnixSeconds,
    IReadOnlyList<string> Capabilities,
    string SignatureHex
)
{
    public static RoomTokenMintResult NotSupported { get; } =
        new(false, string.Empty, 0, [], string.Empty);
}
