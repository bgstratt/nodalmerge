namespace ActiveSync.DotNetHost.Ffi;

public interface IFfiBinaryBridge
{
    FfiBridgeResult ProcessBinaryCommand(byte[] commandPayload);
}