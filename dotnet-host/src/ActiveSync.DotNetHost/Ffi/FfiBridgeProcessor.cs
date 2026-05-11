namespace ActiveSync.DotNetHost.Ffi;

public sealed class FfiBridgeProcessor : IRuntimeCommandBridge, IFfiBinaryBridge
{
    private readonly HostFfiClient _ffi;

    public FfiBridgeProcessor(HostFfiClient ffi)
    {
        _ffi = ffi;
    }

    public FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload)
    {
        var (status, eventsPayload) = _ffi.SubmitCommand(commandPayload);
        return status == AsStatus.Ok
            ? FfiBridgeResult.Success(eventsPayload)
            : FfiBridgeResult.Failure(status);
    }

    public FfiJsonBridgeResult ProcessJsonCommand(string commandJson)
    {
        var (status, eventsJson) = _ffi.SubmitCommandJson(commandJson);
        return status == AsStatus.Ok
            ? FfiJsonBridgeResult.Success(eventsJson)
            : FfiJsonBridgeResult.Failure(status);
    }
}

public sealed record FfiBridgeResult(bool IsSuccess, AsStatus Status, byte[] EventsPayload)
{
    public static FfiBridgeResult Success(byte[] eventsPayload) =>
        new(true, AsStatus.Ok, eventsPayload);

    public static FfiBridgeResult Failure(AsStatus status) =>
        new(false, status, []);
}

public sealed record FfiJsonBridgeResult(bool IsSuccess, AsStatus Status, string EventsJson)
{
    public static FfiJsonBridgeResult Success(string eventsJson) =>
        new(true, AsStatus.Ok, eventsJson);

    public static FfiJsonBridgeResult Failure(AsStatus status) =>
        new(false, status, "[]");
}
