using NodalMerge.DotNetHost;
using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.TestHost;
using Microsoft.Extensions.DependencyInjection;
using System.Net.WebSockets;
using System.Text;

namespace NodalMerge.DotNetHost.Tests;

public class DemoReadinessSmokeTests
{
    [Fact]
    public async Task Demo_smoke_runtime_and_ffi_success_failure_paths_are_stable()
    {
        var ffiSuccessPayload = new byte[] { 0xAB, 0xCD };

        await using var app = BuildTestApp(services =>
        {
            services.AddSingleton<IRuntimeCommandBridge>(new FakeRuntimeCommandBridge(
                FfiJsonBridgeResult.Success("[]"),           // hydrate pre-state EnsureRoom
                FfiJsonBridgeResult.Success("[]"),           // hydrate pre-state RequestServerPack
                FfiJsonBridgeResult.Success("[]"),           // hydrate main EnsureRoom
                FfiJsonBridgeResult.Success("[]"),           // hello EnsureRoom
                FfiJsonBridgeResult.Success("[]"),           // hello OpenSession
                FfiJsonBridgeResult.Success("[]"),           // hello ClientHello
                FfiJsonBridgeResult.Success("[]"),           // hello RequestServerPack
                FfiJsonBridgeResult.Success("[]"),           // catch-up EnsureRoom
                FfiJsonBridgeResult.Success("[]"),           // catch-up RequestServerPack
                FfiJsonBridgeResult.Success("[\"NoopAck\"]"),
                FfiJsonBridgeResult.Failure(AsStatus.Protocol),
                FfiJsonBridgeResult.Success("[\"NoopAck\"]")
            ));

            services.AddSingleton<IFfiBinaryBridge>(new FakeFfiBinaryBridge(
                FfiBridgeResult.Success(ffiSuccessPayload),
                FfiBridgeResult.Failure(AsStatus.Protocol),
                FfiBridgeResult.Success(ffiSuccessPayload)
            ));
        });
        await app.StartAsync();

        using (var runtime = await ConnectWebSocketAsync(app, "/ws/runtime"))
        {
            await SendTextAsync(runtime, "{\"type\":\"hello\",\"room\":\"demo-room\",\"pubkey\":\"demo-peer\",\"frontier\":[]}");
            await SendTextAsync(runtime, "{\"type\":\"noop\"}");
            var runtimeAck = await ReceiveFrameAsync(runtime);
            Assert.Equal(WebSocketMessageType.Text, runtimeAck.MessageType);
            Assert.Contains("\"type\":\"noop-ack\"", runtimeAck.Text);

            await SendTextAsync(runtime, "{\"type\":\"close-session\"}");
            var runtimeStatusError = await ReceiveFrameAsync(runtime);
            Assert.Equal(WebSocketMessageType.Text, runtimeStatusError.MessageType);
            Assert.Contains("\"type\":\"error\"", runtimeStatusError.Text);
            Assert.Contains("\"status\":\"Protocol\"", runtimeStatusError.Text);

            await SendTextAsync(runtime, "{\"type\":\"noop\"}");
            var runtimeRecoveryAck = await ReceiveFrameAsync(runtime);
            Assert.Equal(WebSocketMessageType.Text, runtimeRecoveryAck.MessageType);
            Assert.Contains("\"type\":\"noop-ack\"", runtimeRecoveryAck.Text);

            await runtime.CloseAsync(WebSocketCloseStatus.NormalClosure, "demo-runtime-done", CancellationToken.None);
        }

        using (var ffi = await ConnectWebSocketAsync(app, "/ws/ffi"))
        {
            await SendBinaryAsync(ffi, [0x01]);
            var ffiSuccess = await ReceiveFrameAsync(ffi);
            Assert.Equal(WebSocketMessageType.Binary, ffiSuccess.MessageType);
            Assert.Equal(ffiSuccessPayload, ffiSuccess.Bytes);

            await SendBinaryAsync(ffi, [0x02]);
            var ffiStatusError = await ReceiveFrameAsync(ffi);
            Assert.Equal(WebSocketMessageType.Text, ffiStatusError.MessageType);
            Assert.Contains("\"type\":\"error\"", ffiStatusError.Text);
            Assert.Contains("\"status\":\"Protocol\"", ffiStatusError.Text);

            await SendBinaryAsync(ffi, [0x03]);
            var ffiRecovery = await ReceiveFrameAsync(ffi);
            Assert.Equal(WebSocketMessageType.Binary, ffiRecovery.MessageType);
            Assert.Equal(ffiSuccessPayload, ffiRecovery.Bytes);

            await ffi.CloseAsync(WebSocketCloseStatus.NormalClosure, "demo-ffi-done", CancellationToken.None);
        }
    }

    private static WebApplication BuildTestApp(Action<IServiceCollection>? configureServices = null)
    {
        return HostApplication.Build(
            [],
            configureServices: configureServices,
            configureWebHost: webHost => webHost.UseTestServer()
        );
    }

    private static async Task<WebSocket> ConnectWebSocketAsync(WebApplication app, string route)
    {
        var client = app.GetTestServer().CreateWebSocketClient();
        return await client.ConnectAsync(new Uri($"ws://localhost{route}"), CancellationToken.None);
    }

    private static async Task SendTextAsync(WebSocket ws, string text)
    {
        var bytes = Encoding.UTF8.GetBytes(text);
        await ws.SendAsync(bytes, WebSocketMessageType.Text, true, CancellationToken.None);
    }

    private static async Task SendBinaryAsync(WebSocket ws, byte[] payload)
    {
        await ws.SendAsync(payload, WebSocketMessageType.Binary, true, CancellationToken.None);
    }

    private static async Task<TestWebSocketFrame> ReceiveFrameAsync(WebSocket ws)
    {
        var buffer = new byte[8 * 1024];
        using var ms = new MemoryStream();
        WebSocketReceiveResult result;

        do
        {
            result = await ws.ReceiveAsync(buffer, CancellationToken.None);
            if (result.Count > 0)
            {
                await ms.WriteAsync(buffer.AsMemory(0, result.Count));
            }
        }
        while (!result.EndOfMessage);

        var bytes = ms.ToArray();
        return new TestWebSocketFrame(
            result.MessageType,
            Encoding.UTF8.GetString(bytes),
            bytes
        );
    }

    private sealed record TestWebSocketFrame(
        WebSocketMessageType MessageType,
        string Text,
        byte[] Bytes
    );
}
