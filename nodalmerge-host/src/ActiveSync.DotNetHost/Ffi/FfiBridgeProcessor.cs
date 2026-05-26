using System.Text.Json.Serialization;

namespace NodalMerge.DotNetHost.Ffi;

public sealed class FfiBridgeProcessor : IRuntimeCommandBridge, IFfiBinaryBridge
{
    private readonly HostFfiClient _ffi;

    public FfiBridgeProcessor(HostFfiClient ffi)
    {
        _ffi = ffi;
    }

    public FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload)
    {
        var (status, eventsPayload, denyMetadata) = _ffi.SubmitCommandWithDenyMetadata(commandPayload);
        return status == AsStatus.Ok
            ? FfiBridgeResult.Success(eventsPayload)
            : FfiBridgeResult.Failure(status, denyMetadata);
    }

    public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
    {
        var (status, eventsJson, denyMetadata) = _ffi.SubmitCommandJsonWithDenyMetadata(commandJson);
        return status == AsStatus.Ok
            ? FfiJsonBridgeResult.Success(eventsJson)
            : FfiJsonBridgeResult.Failure(status, denyMetadata);
    }
}

public sealed record FfiDenyMetadata(
    [property: JsonPropertyName("reason_class")]
    string ReasonClass,
    [property: JsonPropertyName("command")]
    string Command,
    [property: JsonPropertyName("required_capability")]
    string RequiredCapability,
    [property: JsonPropertyName("deny_message")]
    string? DenyMessage
);

public sealed record FfiBridgeResult(
    bool IsSuccess,
    AsStatus Status,
    byte[] EventsPayload,
    FfiDenyMetadata? DenyMetadata
)
{
    public static FfiBridgeResult Success(byte[] eventsPayload) =>
        new(true, AsStatus.Ok, eventsPayload, null);

    public static FfiBridgeResult Failure(AsStatus status, FfiDenyMetadata? denyMetadata = null) =>
        new(false, status, [], denyMetadata);
}

public sealed record FfiJsonBridgeResult(
    bool IsSuccess,
    AsStatus Status,
    string EventsJson,
    FfiDenyMetadata? DenyMetadata
)
{
    public static FfiJsonBridgeResult Success(string eventsJson) =>
        new(true, AsStatus.Ok, eventsJson, null);

    public static FfiJsonBridgeResult Failure(AsStatus status, FfiDenyMetadata? denyMetadata = null) =>
        new(false, status, "[]", denyMetadata);
}
