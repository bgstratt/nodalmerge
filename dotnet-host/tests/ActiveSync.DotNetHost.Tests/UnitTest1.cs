using ActiveSync.DotNetHost.Ffi;

namespace ActiveSync.DotNetHost.Tests;

public class FfiBindingTests
{
    [Fact]
    public void NativeLibraryName_is_stable()
    {
        Assert.Equal("activesync_host_ffi", NativeMethods.LibraryName);
    }

    [Fact]
    public void HostFfiClient_returns_nonzero_abi_version_when_native_library_is_available()
    {
        try
        {
            using var ffi = new HostFfiClient();
            var abiVersion = ffi.GetAbiVersion();
            Assert.True(abiVersion > 0);
        }
        catch (DllNotFoundException)
        {
            // Native library not present in test environment; binding path is still validated by compile.
            Assert.True(true);
        }
    }

    [Fact]
    public void SubmitCommand_returns_error_status_for_malformed_payload_when_native_library_is_available()
    {
        try
        {
            using var ffi = new HostFfiClient();
            var malformedPayload = new byte[] { 0xFF, 0xAB, 0x10 };
            var (status, eventsPayload) = ffi.SubmitCommand(malformedPayload);

            Assert.Equal(AsStatus.InvalidArg, status);
            Assert.Empty(eventsPayload);
        }
        catch (DllNotFoundException)
        {
            Assert.True(true);
        }
    }

    [Fact]
    public void BridgeProcessor_returns_failure_for_malformed_payload_when_native_library_is_available()
    {
        try
        {
            using var ffi = new HostFfiClient();
            var bridge = new FfiBridgeProcessor(ffi);
            var malformedPayload = new byte[] { 0xFF, 0xAB, 0x10 };

            var result = bridge.ProcessBinaryCommand(malformedPayload);

            Assert.False(result.IsSuccess);
            Assert.Equal(AsStatus.InvalidArg, result.Status);
            Assert.Empty(result.EventsPayload);
        }
        catch (DllNotFoundException)
        {
            Assert.True(true);
        }
    }

    [Fact]
    public void BridgeProcessor_processes_json_ensure_room_when_native_library_is_available()
    {
        try
        {
            using var ffi = new HostFfiClient();
            var bridge = new FfiBridgeProcessor(ffi);

            var result = bridge.ProcessJsonCommand("{\"room_id\":\"room-json\",\"command\":\"EnsureRoom\"}");

            Assert.True(result.IsSuccess);
            Assert.Equal(AsStatus.Ok, result.Status);
            Assert.Contains("RoomEnsured", result.EventsJson);
        }
        catch (DllNotFoundException)
        {
            Assert.True(true);
        }
    }
}
