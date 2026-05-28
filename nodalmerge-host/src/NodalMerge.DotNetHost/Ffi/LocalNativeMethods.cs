using System.Runtime.InteropServices;

namespace NodalMerge.DotNetHost.Ffi;

public enum LocalStatus : uint
{
    Ok = 0,
    InvalidArg = 1,
    NotFound = 2,
    Unavailable = 3,
    Corruption = 4,
    TailConflict = 5,
    Quota = 6,
    ReadOnly = 7,
    Internal = 255
}

public static class LocalNativeMethods
{
    public const string LibraryName = "nodalmerge_runtime_local_ffi";

    [DllImport(LibraryName, EntryPoint = "nm_local_abi_version")]
    public static extern uint AbiVersion();

    [DllImport(LibraryName, EntryPoint = "nm_local_store_open")]
    public static extern LocalStatus StoreOpen(NmBytesView backend, NmBytesView dataDir, out nint store);

    [DllImport(LibraryName, EntryPoint = "nm_local_store_free")]
    public static extern LocalStatus StoreFree(nint store);

    [DllImport(LibraryName, EntryPoint = "nm_local_store_is_durable")]
    public static extern LocalStatus StoreIsDurable(nint store, out byte durable);

    [DllImport(LibraryName, EntryPoint = "nm_local_store_hydrate_json")]
    public static extern LocalStatus StoreHydrateJson(nint store, NmBytesView roomId, out NmBytesOwned outJson);

    [DllImport(LibraryName, EntryPoint = "nm_local_store_recover_json")]
    public static extern LocalStatus StoreRecoverJson(nint store, NmBytesView roomId, out NmBytesOwned outJson);

    [DllImport(LibraryName, EntryPoint = "nm_local_store_flush_json")]
    public static extern LocalStatus StoreFlushJson(nint store, NmBytesView roomId, out NmBytesOwned outJson);

    [DllImport(LibraryName, EntryPoint = "nm_local_store_append_nodes_json")]
    public static extern LocalStatus StoreAppendNodesJson(
        nint store,
        NmBytesView roomId,
        NmBytesView nodesJson,
        out NmBytesOwned outJson
    );

    [DllImport(LibraryName, EntryPoint = "nm_local_store_append_pack_b64")]
    public static extern LocalStatus StoreAppendPackB64(
        nint store,
        NmBytesView roomId,
        NmBytesView packNodesB64,
        out NmBytesOwned outJson
    );

    [DllImport(LibraryName, EntryPoint = "nm_local_store_canonical_hash_hex")]
    public static extern LocalStatus StoreCanonicalHashHex(
        nint store,
        NmBytesView roomId,
        out NmBytesOwned outHex
    );

    [DllImport(LibraryName, EntryPoint = "nm_bytes_owned_free")]
    public static extern void BytesOwnedFree(NmBytesOwned bytes);
}

[StructLayout(LayoutKind.Sequential)]
public struct NmBytesView
{
    public nint Ptr;
    public nuint Len;
}

[StructLayout(LayoutKind.Sequential)]
public struct NmBytesOwned
{
    public nint Ptr;
    public nuint Len;
}
