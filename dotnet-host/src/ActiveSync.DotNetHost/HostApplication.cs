using ActiveSync.DotNetHost.Ffi;
using ActiveSync.DotNetHost.Runtime;
using Microsoft.AspNetCore.Hosting;
using Microsoft.Extensions.DependencyInjection;

namespace ActiveSync.DotNetHost;

public static class HostApplication
{
        private const string DemoHtml = """
<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>ActiveSync .NET Host Demo</title>
    <style>
        body { font-family: Segoe UI, Arial, sans-serif; margin: 0; background: #0f172a; color: #e2e8f0; }
        .wrap { max-width: 1000px; margin: 0 auto; padding: 20px; }
        .row { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
        .card { background: #111827; border: 1px solid #334155; border-radius: 10px; padding: 14px; }
        h1 { margin: 0 0 14px; font-size: 1.4rem; }
        h2 { margin: 0 0 10px; font-size: 1rem; }
        label { font-size: 0.85rem; color: #94a3b8; display: block; margin: 8px 0 4px; }
        input, textarea { width: 100%; box-sizing: border-box; background: #0b1220; border: 1px solid #334155; color: #e2e8f0; border-radius: 6px; padding: 8px; }
        textarea { min-height: 120px; font-family: Consolas, monospace; }
        .btns { display: flex; gap: 8px; flex-wrap: wrap; margin-top: 10px; }
        button { background: #2563eb; color: white; border: 0; border-radius: 6px; padding: 8px 10px; cursor: pointer; }
        button.alt { background: #374151; }
        button.warn { background: #b91c1c; }
        .mono { font-family: Consolas, monospace; font-size: 0.84rem; }
        .status { color: #93c5fd; margin-top: 8px; }
        .muted { color: #94a3b8; }
        @media (max-width: 900px) { .row { grid-template-columns: 1fr; } }
    </style>
</head>
<body>
    <div class="wrap">
        <h1>ActiveSync .NET Host Demo</h1>
        <div class="muted">This page is served by the host itself, so requests are same-origin.</div>

        <div class="card" style="margin-top:12px;">
            <h2>HTTP Health</h2>
            <div class="btns">
                <button id="btnRoot">GET /</button>
                <button id="btnAbi">GET /ffi/abi-version</button>
            </div>
            <label>HTTP output</label>
            <textarea id="httpOut" class="mono" readonly></textarea>
        </div>

        <div class="row" style="margin-top:12px;">
            <div class="card">
                <h2>Runtime WebSocket (/ws/runtime)</h2>
                <div class="btns">
                    <button id="rtConnect">Connect</button>
                    <button id="rtDisconnect" class="warn">Disconnect</button>
                </div>
                <div id="rtStatus" class="status mono">not connected</div>
                <label>JSON message</label>
                <input id="rtMsg" class="mono" value='{"type":"hello","room":"demo-room","pubkey":"demo-peer","frontier":[]}' />
                <div class="btns">
                    <button id="rtSend">Send JSON</button>
                    <button id="rtHello" class="alt">Preset hello</button>
                    <button id="rtNoop" class="alt">Preset noop</button>
                    <button id="rtCloseSession" class="alt">Preset close-session</button>
                </div>
                <label>Runtime log</label>
                <textarea id="rtLog" class="mono" readonly></textarea>
            </div>

            <div class="card">
                <h2>FFI WebSocket (/ws/ffi)</h2>
                <div class="btns">
                    <button id="ffiConnect">Connect</button>
                    <button id="ffiDisconnect" class="warn">Disconnect</button>
                </div>
                <div id="ffiStatus" class="status mono">not connected</div>
                <label>Binary payload (hex bytes, space-separated)</label>
                <input id="ffiHex" class="mono" value="01" />
                <div class="btns">
                    <button id="ffiSend">Send binary</button>
                </div>
                <label>FFI log</label>
                <textarea id="ffiLog" class="mono" readonly></textarea>
            </div>
        </div>
    </div>

    <script>
        const qs = (id) => document.getElementById(id);
        const append = (id, line) => {
            const el = qs(id);
            const ts = new Date().toISOString();
            el.value = `[${ts}] ${line}\n` + el.value;
        };

        const bytesToHex = (bytes) => Array.from(bytes).map(b => b.toString(16).padStart(2, '0')).join(' ');
        const hexToBytes = (hex) => {
            const parts = String(hex).trim().split(/\s+/).filter(Boolean);
            const out = new Uint8Array(parts.length);
            for (let i = 0; i < parts.length; i++) {
                const n = Number.parseInt(parts[i], 16);
                if (!Number.isFinite(n) || n < 0 || n > 255) throw new Error(`invalid hex byte: ${parts[i]}`);
                out[i] = n;
            }
            return out;
        };

        // HTTP
        qs('btnRoot').onclick = async () => {
            const res = await fetch('/');
            const json = await res.json();
            qs('httpOut').value = JSON.stringify(json, null, 2);
        };
        qs('btnAbi').onclick = async () => {
            const res = await fetch('/ffi/abi-version');
            const json = await res.json();
            qs('httpOut').value = JSON.stringify(json, null, 2);
        };

        // Runtime WS
        let rt;
        const setRtStatus = (s) => qs('rtStatus').textContent = s;
        qs('rtConnect').onclick = () => {
            if (rt && rt.readyState <= 1) return;
            const proto = location.protocol === 'https:' ? 'wss' : 'ws';
            rt = new WebSocket(`${proto}://${location.host}/ws/runtime`);
            setRtStatus('connecting...');
            rt.onopen = () => setRtStatus('connected');
            rt.onclose = (e) => setRtStatus(`closed code=${e.code}`);
            rt.onerror = () => setRtStatus('error');
            rt.onmessage = (e) => append('rtLog', `recv text: ${e.data}`);
        };
        qs('rtDisconnect').onclick = () => { if (rt) rt.close(); };
        qs('rtSend').onclick = () => {
            if (!rt || rt.readyState !== WebSocket.OPEN) return append('rtLog', 'not connected');
            const msg = qs('rtMsg').value;
            rt.send(msg);
            append('rtLog', `sent: ${msg}`);
        };
        qs('rtHello').onclick = () => { qs('rtMsg').value = '{"type":"hello","room":"demo-room","pubkey":"demo-peer","frontier":[]}'; };
        qs('rtNoop').onclick = () => { qs('rtMsg').value = '{"type":"noop"}'; };
        qs('rtCloseSession').onclick = () => { qs('rtMsg').value = '{"type":"close-session"}'; };

        // FFI WS
        let ffi;
        const setFfiStatus = (s) => qs('ffiStatus').textContent = s;
        qs('ffiConnect').onclick = () => {
            if (ffi && ffi.readyState <= 1) return;
            const proto = location.protocol === 'https:' ? 'wss' : 'ws';
            ffi = new WebSocket(`${proto}://${location.host}/ws/ffi`);
            ffi.binaryType = 'arraybuffer';
            setFfiStatus('connecting...');
            ffi.onopen = () => setFfiStatus('connected');
            ffi.onclose = (e) => setFfiStatus(`closed code=${e.code}`);
            ffi.onerror = () => setFfiStatus('error');
            ffi.onmessage = async (e) => {
                if (typeof e.data === 'string') {
                    append('ffiLog', `recv text: ${e.data}`);
                    return;
                }
                const buf = e.data instanceof ArrayBuffer ? new Uint8Array(e.data) : new Uint8Array(await e.data.arrayBuffer());
                append('ffiLog', `recv binary: ${bytesToHex(buf)}`);
            };
        };
        qs('ffiDisconnect').onclick = () => { if (ffi) ffi.close(); };
        qs('ffiSend').onclick = () => {
            if (!ffi || ffi.readyState !== WebSocket.OPEN) return append('ffiLog', 'not connected');
            try {
                const payload = hexToBytes(qs('ffiHex').value);
                ffi.send(payload);
                append('ffiLog', `sent binary: ${bytesToHex(payload)}`);
            } catch (err) {
                append('ffiLog', `invalid payload: ${err.message || err}`);
            }
        };
    </script>
</body>
</html>
""";

    public static WebApplication Build(
        string[] args,
        Action<IServiceCollection>? configureServices = null,
        Action<IWebHostBuilder>? configureWebHost = null
    )
    {
        var builder = WebApplication.CreateBuilder(args);
        configureWebHost?.Invoke(builder.WebHost);

        builder.Services.AddSingleton<HostFfiClient>();
        builder.Services.AddSingleton<FfiBridgeProcessor>();
        builder.Services.AddSingleton<IFfiHttpBridge, FfiHttpBridge>();
        builder.Services.AddSingleton<IFfiBinaryBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        builder.Services.AddSingleton<FfiWebSocketLoopRunner>();
        builder.Services.AddSingleton<IRuntimeCommandBridge>(sp => sp.GetRequiredService<FfiBridgeProcessor>());
        builder.Services.AddSingleton<RuntimeProtocolMapper>();
        builder.Services.AddSingleton<RuntimeSessionIdAllocator>();
        builder.Services.AddSingleton<RuntimeRoomBroker>();
        builder.Services.AddSingleton<RuntimeMessageProcessor>();
        builder.Services.AddSingleton<RuntimeFrameProcessor>();
        builder.Services.AddSingleton<RuntimeWebSocketLoopRunner>();

        configureServices?.Invoke(builder.Services);

        var app = builder.Build();
        app.UseWebSockets();

        app.MapGet("/", () => Results.Ok(new
        {
            service = "activesync-dotnet-host",
            mode = "prototype",
            ffiLibrary = NativeMethods.LibraryName
        }));

        app.MapGet("/demo", () => Results.Content(DemoHtml, "text/html"));

        app.MapGet("/ffi/abi-version", (IFfiHttpBridge ffi) =>
        {
            return Results.Ok(new { abiVersion = ffi.GetAbiVersion() });
        });

        app.MapPost("/ffi/submit", async (HttpRequest request, IFfiHttpBridge ffi) =>
        {
            using var buffer = new MemoryStream();
            await request.Body.CopyToAsync(buffer);
            var payload = buffer.ToArray();

            var (status, eventsPayload) = ffi.SubmitCommand(payload);
            if (status != AsStatus.Ok)
            {
                return Results.BadRequest(new
                {
                    status = status.ToString(),
                    eventsLength = 0
                });
            }

            return Results.File(eventsPayload, "application/octet-stream");
        });

        app.Map("/ws/ffi", async context =>
        {
            if (!context.WebSockets.IsWebSocketRequest)
            {
                context.Response.StatusCode = StatusCodes.Status400BadRequest;
                await context.Response.WriteAsync("WebSocket upgrade required");
                return;
            }

            var bridge = context.RequestServices.GetRequiredService<IFfiBinaryBridge>();
            var loopRunner = context.RequestServices.GetRequiredService<FfiWebSocketLoopRunner>();
            using var socket = await context.WebSockets.AcceptWebSocketAsync();
            await loopRunner.RunAsync(socket, bridge, context.RequestAborted);
        });

        static async Task HandleRuntimeWebSocketAsync(HttpContext context)
        {
            if (!context.WebSockets.IsWebSocketRequest)
            {
                context.Response.StatusCode = StatusCodes.Status400BadRequest;
                await context.Response.WriteAsync("WebSocket upgrade required");
                return;
            }

            var frameProcessor = context.RequestServices.GetRequiredService<RuntimeFrameProcessor>();
            var loopRunner = context.RequestServices.GetRequiredService<RuntimeWebSocketLoopRunner>();
            var sessionIds = context.RequestServices.GetRequiredService<RuntimeSessionIdAllocator>();
            var roomBroker = context.RequestServices.GetRequiredService<RuntimeRoomBroker>();
            var state = new RuntimeConnectionState(sessionIds.Next());

            using var socket = await context.WebSockets.AcceptWebSocketAsync();
            await loopRunner.RunAsync(socket, frameProcessor, state, roomBroker, context.RequestAborted);
        }

        app.Map("/ws/runtime", HandleRuntimeWebSocketAsync);
        app.Map("/ws/{roomId}", HandleRuntimeWebSocketAsync);

        return app;
    }
}