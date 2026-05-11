using System.Runtime.InteropServices;
using System.Text;

namespace ActiveSync.DotNetHost.Ffi;

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
                "(for example: `cargo build -p activesync-host-ffi`) or set " +
                "ACTIVESYNC_HOST_FFI_DLL to the full path of the compiled library.",
                ex
            );
        }

        if (status != AsStatus.Ok || _engine == nint.Zero)
        {
            throw new InvalidOperationException($"as_host_engine_new failed: {status}");
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
}
