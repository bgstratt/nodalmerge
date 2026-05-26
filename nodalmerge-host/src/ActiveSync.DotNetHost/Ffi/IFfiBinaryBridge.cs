namespace NodalMerge.DotNetHost.Ffi;

public interface IFfiBinaryBridge
{
    FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload);
}