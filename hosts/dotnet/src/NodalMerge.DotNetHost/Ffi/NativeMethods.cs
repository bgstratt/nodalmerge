using System.Runtime.InteropServices;

namespace NodalMerge.DotNetHost.Ffi;

public enum AsStatus : uint
{
    Ok = 0,
    InvalidArg = 1,
    NotFound = 2,
    Auth = 3,
    Policy = 4,
    Protocol = 5,
    Internal = 255
}

[StructLayout(LayoutKind.Sequential)]
public struct AsBytesView
{
    public nint Ptr;
    public nuint Len;
}

[StructLayout(LayoutKind.Sequential)]
public struct AsBytesOwned
{
    public nint Ptr;
    public nuint Len;
}

public static class NativeMethods
{
    public const string LibraryName = "nodalmerge_host_ffi";

    [DllImport(LibraryName, EntryPoint = "nm_host_abi_version")]
    public static extern uint AbiVersion();

    [DllImport(LibraryName, EntryPoint = "nm_host_engine_new")]
    public static extern AsStatus HostEngineNew(out nint engine);

    [DllImport(LibraryName, EntryPoint = "nm_host_engine_free")]
    public static extern AsStatus HostEngineFree(nint engine);

    [DllImport(LibraryName, EntryPoint = "nm_host_submit_command")]
    public static extern AsStatus HostSubmitCommand(
        nint engine,
        AsBytesView commandBin,
        out AsBytesOwned outEventsBin
    );

    [DllImport(LibraryName, EntryPoint = "nm_host_submit_command_ex")]
    public static extern AsStatus HostSubmitCommandEx(
        nint engine,
        AsBytesView commandBin,
        out AsBytesOwned outEventsBin,
        out AsBytesOwned outDenyMetadataJson
    );

    [DllImport(LibraryName, EntryPoint = "nm_host_submit_command_json")]
    public static extern AsStatus HostSubmitCommandJson(
        nint engine,
        AsBytesView commandJson,
        out AsBytesOwned outEventsJson
    );

    [DllImport(LibraryName, EntryPoint = "nm_host_submit_command_json_ex")]
    public static extern AsStatus HostSubmitCommandJsonEx(
        nint engine,
        AsBytesView commandJson,
        out AsBytesOwned outEventsJson,
        out AsBytesOwned outDenyMetadataJson
    );

    [DllImport(LibraryName, EntryPoint = "nm_bytes_owned_free")]
    public static extern void BytesOwnedFree(AsBytesOwned bytes);
}
