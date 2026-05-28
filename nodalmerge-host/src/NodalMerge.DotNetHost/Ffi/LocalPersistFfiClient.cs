using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Ffi;

internal delegate LocalStatus LocalJsonInvoke(nint store, NmBytesView roomId, out NmBytesOwned outJson);

/// <summary>
/// In-process peer-local persistence via <c>nodalmerge-runtime-local-ffi</c>.
/// </summary>
public sealed class LocalPersistFfiClient : IDisposable
{
    private nint _store;
    private bool _disposed;

    public LocalPersistFfiClient(string backend = "memory", string? dataDir = null)
    {
        NativeLibraryResolver.Configure();

        var backendBytes = Encoding.UTF8.GetBytes(backend);
        var dirBytes = Encoding.UTF8.GetBytes(dataDir ?? string.Empty);

        GCHandle? pinBackend = null;
        GCHandle? pinDir = null;
        try
        {
            var backendView = PinUtf8(backendBytes, ref pinBackend);
            var dirView = PinUtf8(dirBytes, ref pinDir);
            var status = LocalNativeMethods.StoreOpen(backendView, dirView, out _store);
            if (status != LocalStatus.Ok || _store == nint.Zero)
            {
                throw new InvalidOperationException($"nm_local_store_open failed: {status}");
            }
        }
        finally
        {
            if (pinBackend.HasValue) pinBackend.Value.Free();
            if (pinDir.HasValue) pinDir.Value.Free();
        }
    }

    public bool IsDurable
    {
        get
        {
            ThrowIfDisposed();
            var status = LocalNativeMethods.StoreIsDurable(_store, out var durable);
            if (status != LocalStatus.Ok)
            {
                throw new InvalidOperationException($"nm_local_store_is_durable failed: {status}");
            }
            return durable != 0;
        }
    }

    public JsonDocument Hydrate(string roomId) => CallJson(roomId, LocalNativeMethods.StoreHydrateJson);

    public JsonDocument Recover(string roomId) => CallJson(roomId, LocalNativeMethods.StoreRecoverJson);

    public JsonDocument Flush(string roomId) => CallJson(roomId, LocalNativeMethods.StoreFlushJson);

    public JsonDocument AppendNodesJson(string roomId, string nodesJson)
    {
        ThrowIfDisposed();
        var roomBytes = Encoding.UTF8.GetBytes(roomId);
        var nodesBytes = Encoding.UTF8.GetBytes(nodesJson);
        GCHandle? pinRoom = null;
        GCHandle? pinNodes = null;
        try
        {
            var roomView = PinUtf8(roomBytes, ref pinRoom);
            var nodesView = PinUtf8(nodesBytes, ref pinNodes);
            var status = LocalNativeMethods.StoreAppendNodesJson(_store, roomView, nodesView, out var owned);
            return ParseOwnedJson(status, owned);
        }
        finally
        {
            if (pinRoom.HasValue) pinRoom.Value.Free();
            if (pinNodes.HasValue) pinNodes.Value.Free();
        }
    }

    public JsonDocument AppendPackB64(string roomId, string packNodesB64)
    {
        ThrowIfDisposed();
        var roomBytes = Encoding.UTF8.GetBytes(roomId);
        var packBytes = Encoding.UTF8.GetBytes(packNodesB64);
        GCHandle? pinRoom = null;
        GCHandle? pinPack = null;
        try
        {
            var roomView = PinUtf8(roomBytes, ref pinRoom);
            var packView = PinUtf8(packBytes, ref pinPack);
            var status = LocalNativeMethods.StoreAppendPackB64(_store, roomView, packView, out var owned);
            return ParseOwnedJson(status, owned);
        }
        finally
        {
            if (pinRoom.HasValue) pinRoom.Value.Free();
            if (pinPack.HasValue) pinPack.Value.Free();
        }
    }

    public string CanonicalHashHex(string roomId)
    {
        ThrowIfDisposed();
        var roomBytes = Encoding.UTF8.GetBytes(roomId);
        GCHandle? pinRoom = null;
        try
        {
            var roomView = PinUtf8(roomBytes, ref pinRoom);
            var status = LocalNativeMethods.StoreCanonicalHashHex(_store, roomView, out var owned);
            if (status != LocalStatus.Ok)
            {
                throw new InvalidOperationException($"nm_local_store_canonical_hash_hex failed: {status}");
            }
            return Encoding.UTF8.GetString(CopyOwned(owned));
        }
        finally
        {
            if (pinRoom.HasValue) pinRoom.Value.Free();
        }
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        if (_store != nint.Zero)
        {
            LocalNativeMethods.StoreFree(_store);
            _store = nint.Zero;
        }

        _disposed = true;
    }

    private JsonDocument CallJson(string roomId, LocalJsonInvoke invoke)
    {
        ThrowIfDisposed();
        var roomBytes = Encoding.UTF8.GetBytes(roomId);
        GCHandle? pinRoom = null;
        try
        {
            var roomView = PinUtf8(roomBytes, ref pinRoom);
            var status = invoke(_store, roomView, out var owned);
            return ParseOwnedJson(status, owned);
        }
        finally
        {
            if (pinRoom.HasValue) pinRoom.Value.Free();
        }
    }

    private static JsonDocument ParseOwnedJson(LocalStatus status, NmBytesOwned owned)
    {
        if (status != LocalStatus.Ok)
        {
            throw new InvalidOperationException($"local persist FFI call failed: {status}");
        }

        try
        {
            var bytes = CopyOwned(owned);
            return JsonDocument.Parse(bytes);
        }
        finally
        {
            LocalNativeMethods.BytesOwnedFree(owned);
        }
    }

    private static byte[] CopyOwned(NmBytesOwned owned)
    {
        if (owned.Ptr == nint.Zero || owned.Len == 0)
        {
            return Array.Empty<byte>();
        }

        var bytes = new byte[(int)owned.Len];
        Marshal.Copy(owned.Ptr, bytes, 0, bytes.Length);
        return bytes;
    }

    private static NmBytesView PinUtf8(byte[] bytes, ref GCHandle? handle)
    {
        if (bytes.Length == 0)
        {
            return new NmBytesView { Ptr = nint.Zero, Len = 0 };
        }

        handle = GCHandle.Alloc(bytes, GCHandleType.Pinned);
        return new NmBytesView { Ptr = handle.Value.AddrOfPinnedObject(), Len = (nuint)bytes.Length };
    }

    private void ThrowIfDisposed()
    {
        if (_disposed)
        {
            throw new ObjectDisposedException(nameof(LocalPersistFfiClient));
        }
    }
}
