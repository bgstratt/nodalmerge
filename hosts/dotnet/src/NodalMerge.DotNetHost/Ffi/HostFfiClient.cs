using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Ffi;

public sealed class HostFfiClient : IDisposable
{
    private nint _engine;
    private bool _disposed;
    private readonly object _sync = new();

    public HostFfiClient()
    {
        NativeLibraryResolver.Configure();

        AsStatus status;
        try
        {
            status = NativeMethods.HostEngineNew(out _engine);
        }
        catch (DllNotFoundException ex)
        {
            throw new InvalidOperationException(
                "Failed to load native host FFI library. Build host-ffi first " +
                "(for example: `cargo build -p nodalmerge-host-ffi`) or set " +
                "NODALMERGE_HOST_FFI_DLL (or legacy NODALMERGE_HOST_FFI_DLL) " +
                "to the full path of the compiled library.",
                ex
            );
        }

        if (status != AsStatus.Ok || _engine == nint.Zero)
        {
            throw new InvalidOperationException($"nm_host_engine_new failed: {status}");
        }
    }

    public (AsStatus status, byte[] eventsPayload, FfiDenyMetadata? denyMetadata) SubmitCommandWithDenyMetadata(byte[] commandPayload)
    {
        if (commandPayload is null)
        {
            throw new ArgumentNullException(nameof(commandPayload));
        }

        lock (_sync)
        {
            ThrowIfDisposed();

            GCHandle? pinned = null;
            try
            {
                var view = new AsBytesView
                {
                    Ptr = nint.Zero,
                    Len = (nuint)commandPayload.Length
                };

                if (commandPayload.Length > 0)
                {
                    pinned = GCHandle.Alloc(commandPayload, GCHandleType.Pinned);
                    view.Ptr = pinned.Value.AddrOfPinnedObject();
                }

                try
                {
                    var status = NativeMethods.HostSubmitCommandEx(
                        _engine,
                        view,
                        out var eventsOwned,
                        out var denyOwned
                    );

                    var events = CopyOwnedBytes(eventsOwned);
                    var denyMetadata = ParseDenyMetadata(denyOwned);
                    return (status, events, denyMetadata);
                }
                catch (EntryPointNotFoundException)
                {
                    var status = NativeMethods.HostSubmitCommand(_engine, view, out var eventsOwned);
                    var events = CopyOwnedBytes(eventsOwned);
                    return (status, events, null);
                }
            }
            finally
            {
                if (pinned.HasValue)
                {
                    pinned.Value.Free();
                }
            }
        }
    }

    public uint GetAbiVersion()
    {
        lock (_sync)
        {
            ThrowIfDisposed();
            return NativeMethods.AbiVersion();
        }
    }

    public (AsStatus status, byte[] eventsPayload) SubmitCommand(byte[] commandPayload)
    {
        if (commandPayload is null)
        {
            throw new ArgumentNullException(nameof(commandPayload));
        }

        lock (_sync)
        {
            ThrowIfDisposed();

            GCHandle? pinned = null;
            try
            {
                var view = new AsBytesView
                {
                    Ptr = nint.Zero,
                    Len = (nuint)commandPayload.Length
                };

                if (commandPayload.Length > 0)
                {
                    pinned = GCHandle.Alloc(commandPayload, GCHandleType.Pinned);
                    view.Ptr = pinned.Value.AddrOfPinnedObject();
                }

                var status = NativeMethods.HostSubmitCommand(_engine, view, out var owned);
                if (status != AsStatus.Ok)
                {
                    return (status, []);
                }

                if (owned.Ptr == nint.Zero || owned.Len == 0)
                {
                    return (status, []);
                }

                var events = new byte[(int)owned.Len];
                Marshal.Copy(owned.Ptr, events, 0, events.Length);
                NativeMethods.BytesOwnedFree(owned);
                return (status, events);
            }
            finally
            {
                if (pinned.HasValue)
                {
                    pinned.Value.Free();
                }
            }
        }
    }

    public (AsStatus status, string eventsJson) SubmitCommandJson(string commandJson)
    {
        if (commandJson is null)
        {
            throw new ArgumentNullException(nameof(commandJson));
        }

        var commandBytes = Encoding.UTF8.GetBytes(commandJson);

        lock (_sync)
        {
            ThrowIfDisposed();

            GCHandle? pinned = null;
            try
            {
                var view = new AsBytesView
                {
                    Ptr = nint.Zero,
                    Len = (nuint)commandBytes.Length
                };

                if (commandBytes.Length > 0)
                {
                    pinned = GCHandle.Alloc(commandBytes, GCHandleType.Pinned);
                    view.Ptr = pinned.Value.AddrOfPinnedObject();
                }

                var status = NativeMethods.HostSubmitCommandJson(_engine, view, out var owned);
                if (status != AsStatus.Ok)
                {
                    return (status, "[]");
                }

                if (owned.Ptr == nint.Zero || owned.Len == 0)
                {
                    return (status, "[]");
                }

                var events = new byte[(int)owned.Len];
                Marshal.Copy(owned.Ptr, events, 0, events.Length);
                NativeMethods.BytesOwnedFree(owned);
                return (status, Encoding.UTF8.GetString(events));
            }
            finally
            {
                if (pinned.HasValue)
                {
                    pinned.Value.Free();
                }
            }
        }
    }

    public (AsStatus status, string eventsJson, FfiDenyMetadata? denyMetadata) SubmitCommandJsonWithDenyMetadata(string commandJson)
    {
        if (commandJson is null)
        {
            throw new ArgumentNullException(nameof(commandJson));
        }

        var commandBytes = Encoding.UTF8.GetBytes(commandJson);

        lock (_sync)
        {
            ThrowIfDisposed();

            GCHandle? pinned = null;
            try
            {
                var view = new AsBytesView
                {
                    Ptr = nint.Zero,
                    Len = (nuint)commandBytes.Length
                };

                if (commandBytes.Length > 0)
                {
                    pinned = GCHandle.Alloc(commandBytes, GCHandleType.Pinned);
                    view.Ptr = pinned.Value.AddrOfPinnedObject();
                }

                try
                {
                    var status = NativeMethods.HostSubmitCommandJsonEx(
                        _engine,
                        view,
                        out var eventsOwned,
                        out var denyOwned
                    );

                    var eventsJson = CopyOwnedString(eventsOwned);
                    var denyMetadata = ParseDenyMetadata(denyOwned);
                    return (status, eventsJson, denyMetadata);
                }
                catch (EntryPointNotFoundException)
                {
                    var status = NativeMethods.HostSubmitCommandJson(_engine, view, out var eventsOwned);
                    var eventsJson = CopyOwnedString(eventsOwned);
                    return (status, eventsJson, null);
                }
            }
            finally
            {
                if (pinned.HasValue)
                {
                    pinned.Value.Free();
                }
            }
        }
    }

    public void Dispose()
    {
        lock (_sync)
        {
            if (_disposed)
            {
                return;
            }

            if (_engine != nint.Zero)
            {
                _ = NativeMethods.HostEngineFree(_engine);
                _engine = nint.Zero;
            }

            _disposed = true;
            GC.SuppressFinalize(this);
        }
    }

    private void ThrowIfDisposed()
    {
        if (_disposed)
        {
            throw new ObjectDisposedException(nameof(HostFfiClient));
        }
    }

    private static byte[] CopyOwnedBytes(AsBytesOwned owned)
    {
        try
        {
            if (owned.Ptr == nint.Zero || owned.Len == 0)
            {
                return [];
            }

            var bytes = new byte[(int)owned.Len];
            Marshal.Copy(owned.Ptr, bytes, 0, bytes.Length);
            return bytes;
        }
        finally
        {
            NativeMethods.BytesOwnedFree(owned);
        }
    }

    private static string CopyOwnedString(AsBytesOwned owned)
    {
        var bytes = CopyOwnedBytes(owned);
        return bytes.Length == 0 ? "[]" : Encoding.UTF8.GetString(bytes);
    }

    private static FfiDenyMetadata? ParseDenyMetadata(AsBytesOwned owned)
    {
        var bytes = CopyOwnedBytes(owned);
        if (bytes.Length == 0)
        {
            return null;
        }

        try
        {
            return JsonSerializer.Deserialize<FfiDenyMetadata>(bytes);
        }
        catch (JsonException)
        {
            return null;
        }
    }
}
