namespace NodalMerge.DotNetHost.Ffi;

public interface IFfiHttpBridge
{
    uint GetAbiVersion();

    (AsStatus status, byte[] eventsPayload) SubmitCommand(byte[] commandPayload);
}

public sealed class FfiHttpBridge : IFfiHttpBridge
{
    private readonly HostFfiClient _ffi;

    public FfiHttpBridge(HostFfiClient ffi)
    {
        _ffi = ffi;
    }

    public uint GetAbiVersion()
    {
        return _ffi.GetAbiVersion();
    }

    public (AsStatus status, byte[] eventsPayload) SubmitCommand(byte[] commandPayload)
    {
        return _ffi.SubmitCommand(commandPayload);
    }
}