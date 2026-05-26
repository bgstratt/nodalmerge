namespace NodalMerge.DotNetHost.Ffi;

public interface IRuntimeCommandBridge
{
    FfiJsonBridgeResult ProcessJsonCommand(string commandJson);
}