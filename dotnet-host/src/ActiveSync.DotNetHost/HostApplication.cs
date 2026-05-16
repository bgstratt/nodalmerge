using ActiveSync.DotNetHost.Ffi;
using ActiveSync.DotNetHost.Runtime;
using ActiveSync.Host.Abstractions.Providers;
using ActiveSync.Host.Composition;
using Microsoft.AspNetCore.Hosting;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using System.Text.Json.Serialization;

namespace ActiveSync.DotNetHost;

public static class HostApplication
{
    private sealed record SyncTokenMintRequest(
        string Room,
        [property: JsonPropertyName("peerPubkeyHex")] string PeerPubkeyHex,
        [property: JsonPropertyName("lifetimeSeconds")] int? LifetimeSeconds,
        IReadOnlyList<string>? Capabilities
    );

    private sealed record SyncTokenValidateRequest(
        string Room,
        [property: JsonPropertyName("peer_pubkey_hex")] string PeerPubkeyHex,
        [property: JsonPropertyName("expiry_secs")] long ExpirySecs,
        IReadOnlyList<string> Capabilities,
        [property: JsonPropertyName("sig_hex")] string SigHex
    );

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
        Action<IWebHostBuilder>? configureWebHost = null,
        Action<ConfigurationManager>? configureConfiguration = null
    )
    {
        var builder = WebApplication.CreateBuilder(args);
        configureWebHost?.Invoke(builder.WebHost);
        configureConfiguration?.Invoke(builder.Configuration);

        builder.Services.AddActiveSyncHostProviders(builder.Configuration);
        builder.Services.AddActiveSyncRuntimeCore(builder.Configuration);

        configureServices?.Invoke(builder.Services);

        var app = builder.Build();
        app.UseWebSockets();

        var startupLogger = app.Services.GetRequiredService<ILoggerFactory>().CreateLogger("ActiveSync.Startup");
        var providerOptions = app.Services.GetRequiredService<ActiveSyncHostProviderOptions>();
        startupLogger.LogInformation(
            "ActiveSync providers node={Node} blob={Blob} auth={Auth}",
            providerOptions.NodeStorageProvider,
            providerOptions.BlobStorageProvider,
            providerOptions.AuthProvider
        );
        startupLogger.LogWarning(
            "Runtime websocket path remains relay-first; experimental DAG pack persistence/hydration via configured node store is enabled"
        );
        EmitAuthReadinessChecks(app.Services, providerOptions, startupLogger);

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

        app.MapGet("/debug/providers", (IConfiguration cfg) =>
        {
            return Results.Ok(new
            {
                providers = new
                {
                    node = providerOptions.NodeStorageProvider,
                    blob = providerOptions.BlobStorageProvider,
                    auth = providerOptions.AuthProvider
                },
                mongo = new
                {
                    database = cfg["ActiveSync:Storage:Mongo:DatabaseName"]
                },
                delegated = new
                {
                    baseUrl = cfg["ActiveSync:Storage:S3Delegated:BaseUrl"],
                    putPath = cfg["ActiveSync:Storage:S3Delegated:PutPath"],
                    getPath = cfg["ActiveSync:Storage:S3Delegated:GetPath"]
                }
            });
        });

        static async Task<IResult> HandleTokenMintAsync(
            SyncTokenMintRequest request,
            IRoomTokenAuthProvider authProvider,
            CancellationToken cancellationToken
        )
        {
            if (string.IsNullOrWhiteSpace(request.Room))
            {
                return Results.BadRequest(new { error = "room is required" });
            }

            if (string.IsNullOrWhiteSpace(request.PeerPubkeyHex))
            {
                return Results.BadRequest(new { error = "peerPubkeyHex is required" });
            }

            try
            {
                var mintResult = await authProvider.MintAsync(
                    new RoomTokenMintRequest(
                        request.Room,
                        request.PeerPubkeyHex,
                        request.LifetimeSeconds,
                        request.Capabilities
                    ),
                    cancellationToken
                );

                if (!mintResult.IsSupported)
                {
                    return Results.StatusCode(StatusCodes.Status501NotImplemented);
                }

                return Results.Ok(new
                {
                    peer_pubkey_hex = mintResult.PeerPubkeyHex,
                    expiry_secs = mintResult.ExpiryUnixSeconds,
                    capabilities = mintResult.Capabilities,
                    sig_hex = mintResult.SignatureHex
                });
            }
            catch (SidecarAuthProviderException ex)
            {
                if (ex.IsTimeout)
                {
                    return Results.StatusCode(StatusCodes.Status504GatewayTimeout);
                }

                if (ex.StatusCode is >= 500)
                {
                    return Results.StatusCode(StatusCodes.Status502BadGateway);
                }

                return Results.BadRequest(new { error = ex.Message });
            }

        }

        static async Task<IResult> HandleTokenValidateAsync(
            SyncTokenValidateRequest request,
            IRoomTokenAuthProvider authProvider,
            CancellationToken cancellationToken
        )
        {
            if (string.IsNullOrWhiteSpace(request.Room))
            {
                return Results.BadRequest(new { error = "room is required" });
            }

            if (string.IsNullOrWhiteSpace(request.PeerPubkeyHex))
            {
                return Results.BadRequest(new { error = "peer_pubkey_hex is required" });
            }

            if (string.IsNullOrWhiteSpace(request.SigHex))
            {
                return Results.BadRequest(new { error = "sig_hex is required" });
            }

            try
            {
                var validation = await authProvider.ValidateAsync(
                    new RoomTokenValidationRequest(
                        request.Room,
                        request.PeerPubkeyHex,
                        request.ExpirySecs,
                        request.Capabilities,
                        request.SigHex
                    ),
                    cancellationToken
                );

                return Results.Ok(new { valid = validation.IsValid, reason = validation.Reason });
            }
            catch (SidecarAuthProviderException ex)
            {
                if (ex.IsTimeout)
                {
                    return Results.StatusCode(StatusCodes.Status504GatewayTimeout);
                }

                if (ex.StatusCode is >= 500)
                {
                    return Results.StatusCode(StatusCodes.Status502BadGateway);
                }

                return Results.BadRequest(new { error = ex.Message });
            }

        }

        static async Task<IResult> HandleBlobUrlAsync(
            [AsParameters] BlobUrlQuery query,
            IBlobUrlResolverProvider resolver,
            CancellationToken cancellationToken
        )
        {
            if (string.IsNullOrWhiteSpace(query.Hash))
            {
                return Results.BadRequest(new { error = "hash is required" });
            }

            var room = string.IsNullOrWhiteSpace(query.Room) ? "default" : query.Room;
            var scope = string.IsNullOrWhiteSpace(query.Namespace) ? "assets" : query.Namespace;
            var op = (query.Op ?? string.Empty).Trim().ToLowerInvariant();

            if (op == "put")
            {
                if (query.Size is null || query.Size <= 0)
                {
                    return Results.BadRequest(new { error = "size must be a positive integer for op=put" });
                }

                var url = await resolver.ResolvePutUrlAsync(
                    new BlobPutUrlRequest(
                        room,
                        scope,
                        query.Hash,
                        query.Size.Value,
                        query.ContentType
                    ),
                    cancellationToken
                );

                if (url is null)
                {
                    return Results.StatusCode(StatusCodes.Status404NotFound);
                }

                return Results.Ok(new
                {
                    url = url.Url,
                    expiresAt = url.ExpiresAtUtc.ToUnixTimeSeconds()
                });
            }

            if (op == "get")
            {
                var url = await resolver.ResolveGetUrlAsync(
                    new BlobGetUrlRequest(room, scope, query.Hash),
                    cancellationToken
                );

                if (url is null)
                {
                    return Results.StatusCode(StatusCodes.Status404NotFound);
                }

                return Results.Ok(new
                {
                    url = url.Url,
                    expiresAt = url.ExpiresAtUtc.ToUnixTimeSeconds()
                });
            }

            return Results.BadRequest(new { error = "op must be 'get' or 'put'" });
        }

        app.MapPost("/sync/token", HandleTokenMintAsync);
        app.MapPost("/api/sync/token", HandleTokenMintAsync);
        app.MapPost("/sync/token/validate", HandleTokenValidateAsync);
        app.MapPost("/api/sync/token/validate", HandleTokenValidateAsync);
        app.MapGet("/sync/blob-url", HandleBlobUrlAsync);
        app.MapGet("/api/sync/blob-url", HandleBlobUrlAsync);

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
            var logger = context.RequestServices
                .GetRequiredService<ILoggerFactory>()
                .CreateLogger("ActiveSync.RuntimeWs");

            if (!context.WebSockets.IsWebSocketRequest)
            {
                context.Response.StatusCode = StatusCodes.Status400BadRequest;
                await context.Response.WriteAsync("WebSocket upgrade required");
                return;
            }

            logger.LogInformation(
                "runtime ws open remote={Remote} path={Path}",
                context.Connection.RemoteIpAddress?.ToString(),
                context.Request.Path.ToString()
            );

            var frameProcessor = context.RequestServices.GetRequiredService<RuntimeFrameProcessor>();
            var loopRunner = context.RequestServices.GetRequiredService<RuntimeWebSocketLoopRunner>();
            var sessionIds = context.RequestServices.GetRequiredService<RuntimeSessionIdAllocator>();
            var roomBroker = context.RequestServices.GetRequiredService<RuntimeRoomBroker>();
            var dagPersistence = context.RequestServices.GetRequiredService<RuntimeDagPersistenceService>();
            var tokenValidationService = context.RequestServices.GetRequiredService<RuntimeTokenValidationService>();
            var state = new RuntimeConnectionState(sessionIds.Next());

            // Compatibility path `/ws/{roomId}` can carry the room only in the URL.
            // Seed state.RoomId early so hydration can run before hello processing.
            if (context.Request.RouteValues.TryGetValue("roomId", out var roomRouteValue)
                && roomRouteValue is not null)
            {
                var routeRoom = roomRouteValue.ToString();
                if (!string.IsNullOrWhiteSpace(routeRoom)
                    && !string.Equals(routeRoom, "runtime", StringComparison.OrdinalIgnoreCase))
                {
                    state.RoomId = Uri.UnescapeDataString(routeRoom);
                }
            }

            using var socket = await context.WebSockets.AcceptWebSocketAsync();
            try
            {
                await loopRunner.RunAsync(socket, frameProcessor, state, roomBroker, tokenValidationService, dagPersistence, context.RequestAborted);
            }
            finally
            {
                logger.LogInformation(
                    "runtime ws closed remote={Remote} path={Path} close={CloseStatus}",
                    context.Connection.RemoteIpAddress?.ToString(),
                    context.Request.Path.ToString(),
                    socket.CloseStatus?.ToString() ?? "none"
                );
            }
        }

        app.Map("/ws/runtime", HandleRuntimeWebSocketAsync);
        app.Map("/ws/{roomId}", HandleRuntimeWebSocketAsync);

        return app;
    }

    private static void EmitAuthReadinessChecks(
        IServiceProvider services,
        ActiveSyncHostProviderOptions providerOptions,
        ILogger startupLogger)
    {
        if (string.Equals(providerOptions.AuthProvider, "JwtBridgeEmbedded", StringComparison.Ordinal))
        {
            var options = services.GetRequiredService<JwtBridgeEmbeddedAuthOptions>();
            startupLogger.LogInformation(
                "Auth readiness provider=JwtBridgeEmbedded issuer={Issuer} audience={Audience} clock_skew_seconds={ClockSkewSeconds} previous_keys={PreviousKeyCount}",
                options.Issuer,
                options.Audience,
                options.ClockSkewSeconds,
                options.PreviousSigningKeys.Count
            );

            if (options.ClockSkewSeconds > 60)
            {
                startupLogger.LogWarning(
                    "Auth readiness provider=JwtBridgeEmbedded policy warning: ClockSkewSeconds={ClockSkewSeconds} exceeds recommended max=60",
                    options.ClockSkewSeconds
                );
            }

            return;
        }

        if (string.Equals(providerOptions.AuthProvider, "JwtBridgeSidecar", StringComparison.Ordinal))
        {
            var options = services.GetRequiredService<JwtBridgeSidecarAuthOptions>();
            startupLogger.LogInformation(
                "Auth readiness provider=JwtBridgeSidecar base_url={BaseUrl} timeout_seconds={TimeoutSeconds} api_key_configured={ApiKeyConfigured}",
                options.BaseUrl,
                options.TimeoutSeconds,
                !string.IsNullOrWhiteSpace(options.ApiKey)
            );

            if (options.TimeoutSeconds > 30)
            {
                startupLogger.LogWarning(
                    "Auth readiness provider=JwtBridgeSidecar policy warning: TimeoutSeconds={TimeoutSeconds} exceeds recommended max=30",
                    options.TimeoutSeconds
                );
            }
        }
    }

    private sealed class BlobUrlQuery
    {
        public string Hash { get; init; } = string.Empty;

        public string? Op { get; init; }

        public string? Room { get; init; }

        public string? Namespace { get; init; }

        [JsonPropertyName("contentType")]
        public string? ContentType { get; init; }

        public long? Size { get; init; }
    }
}
