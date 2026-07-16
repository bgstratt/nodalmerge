using Blake3;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Http;
using Microsoft.AspNetCore.Http.Features;
using Microsoft.Extensions.DependencyInjection;
using Microsoft.Extensions.Logging;
using NodalMerge.DotNetHost.Ffi;
using NodalMerge.DotNetHost.Runtime;
using NodalMerge.Host.Abstractions.Providers;
using NodalMerge.Host.Composition;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Serialization;

namespace NodalMerge.DotNetHost;

public static class WebApplicationExtensions
{
    private sealed record SyncTokenMintRequest(
        string Room,
        [property: JsonPropertyName("peerPubkeyHex")] string PeerPubkeyHex,
        [property: JsonPropertyName("lifetimeSeconds")] int? LifetimeSeconds,
        IReadOnlyList<string>? Capabilities,
        [property: JsonPropertyName("capabilityProfileVersion")] string? CapabilityProfileVersion
    );

    private sealed record SyncTokenValidateRequest(
        string Room,
        [property: JsonPropertyName("peer_pubkey_hex")] string PeerPubkeyHex,
        [property: JsonPropertyName("expiry_secs")] long ExpirySecs,
        IReadOnlyList<string> Capabilities,
        [property: JsonPropertyName("capability_profile_version")] string? CapabilityProfileVersion,
        [property: JsonPropertyName("sig_hex")] string SigHex
    );

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

    /// <summary>
    /// Query-string shape for the frozen <c>GET /blobs/{hash}/url</c> contract
    /// (docs/BLOB_HTTP_SURFACE.md "Blob URL resolution"): <c>op</c>, and
    /// <c>size</c>/<c>contentType</c> for <c>op=put</c>. No room/namespace —
    /// this surface is the global CAS, not the legacy per-room
    /// <c>/sync/blob-url</c> query shape.
    /// </summary>
    private sealed class BlobUrlOpQuery
    {
        public string? Op { get; init; }
        [JsonPropertyName("contentType")]
        public string? ContentType { get; init; }
        public long? Size { get; init; }
    }

    private const string DemoHtml = """
<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>NodalMerge .NET Host Demo</title>
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
        <h1>NodalMerge .NET Host Demo</h1>
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

    /// <summary>
    /// Registers the NodalMerge WebSocket middleware and maps all runtime endpoints.
    /// Call this after building the WebApplication and before app.Run().
    /// </summary>
    public static WebApplication MapNodalMergeEndpoints(this WebApplication app)
    {
        app.UseWebSockets();

        var startupLogger = app.Services.GetRequiredService<ILoggerFactory>().CreateLogger("NodalMerge.Startup");
        var providerOptions = app.Services.GetRequiredService<NodalMergeHostProviderOptions>();
        startupLogger.LogInformation(
            "NodalMerge providers node={Node} blob={Blob} auth={Auth}",
            providerOptions.NodeStorageProvider,
            providerOptions.BlobStorageProvider,
            providerOptions.AuthProvider
        );
        startupLogger.LogWarning(
            "Runtime websocket path remains relay-first; experimental DAG pack persistence/hydration via configured node store is enabled"
        );

        var peerLocal = app.Services.GetRequiredService<RuntimePeerLocalPersistenceService>();
        if (peerLocal.IsEnabled)
        {
            startupLogger.LogInformation(
                "Runtime peer-local persistence is enabled (in-process nodalmerge-runtime-local-ffi)"
            );
        }

        EmitAuthReadinessChecks(app.Services, providerOptions, startupLogger);

        // Blob origin surface (docs/BLOB_HTTP_SURFACE.md, slice S2.1a): bound once
        // here and closed over by the handlers below, per that doc's guidance that
        // this doesn't need to be a DI service. Declared before first use since
        // the URL-resolution routes (slice S4.1) and the legacy /sync/blob-url
        // alias both close over it too.
        var blobHttpOptions = BlobHttpOptions.FromConfiguration(app.Configuration);

        app.MapGet("/", () => Results.Ok(new
        {
            service = "nodalmerge-dotnet-host",
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
                    database = cfg["NodalMerge:Storage:Mongo:DatabaseName"]
                },
                delegated = new
                {
                    baseUrl = cfg["NodalMerge:Storage:S3Delegated:BaseUrl"],
                    putPath = cfg["NodalMerge:Storage:S3Delegated:PutPath"],
                    getPath = cfg["NodalMerge:Storage:S3Delegated:GetPath"]
                }
            });
        });

        app.MapPost("/sync/token", HandleTokenMintAsync);
        app.MapPost("/api/sync/token", HandleTokenMintAsync);
        app.MapPost("/sync/token/validate", HandleTokenValidateAsync);
        app.MapPost("/api/sync/token/validate", HandleTokenValidateAsync);

        // Legacy blob-url resolver (pre-dates slice S4.1's frozen contract).
        // Kept as an alias of GET /blobs/{hash}/url — same handler, same
        // response shape ({"url":..., "expiresAtUtc":...}) and status codes;
        // only the query-parameter surface (room/namespace) differs, per
        // docs/BLOB_HTTP_SURFACE.md's "Blob URL resolution" section.
        app.MapGet("/sync/blob-url", (HttpContext context, [AsParameters] BlobUrlQuery query, CancellationToken cancellationToken) =>
            HandleBlobUrlAsync(context, query, blobHttpOptions, cancellationToken));
        app.MapGet("/api/sync/blob-url", (HttpContext context, [AsParameters] BlobUrlQuery query, CancellationToken cancellationToken) =>
            HandleBlobUrlAsync(context, query, blobHttpOptions, cancellationToken));

        app.MapGet("/blobs/{hash}", (HttpContext context, string hash, IBlobStoreProvider blobStore, CancellationToken cancellationToken) =>
            HandleBlobGetAsync(context, hash, blobStore, blobHttpOptions, cancellationToken));
        app.MapGet("/api/blobs/{hash}", (HttpContext context, string hash, IBlobStoreProvider blobStore, CancellationToken cancellationToken) =>
            HandleBlobGetAsync(context, hash, blobStore, blobHttpOptions, cancellationToken));

        app.MapMethods("/blobs/{hash}", ["HEAD"], (HttpContext context, string hash, IBlobStoreProvider blobStore, CancellationToken cancellationToken) =>
            HandleBlobHeadAsync(context, hash, blobStore, blobHttpOptions, cancellationToken));
        app.MapMethods("/api/blobs/{hash}", ["HEAD"], (HttpContext context, string hash, IBlobStoreProvider blobStore, CancellationToken cancellationToken) =>
            HandleBlobHeadAsync(context, hash, blobStore, blobHttpOptions, cancellationToken));

        app.MapPut("/blobs/{hash}", (HttpContext context, string hash, IBlobStoreProvider blobStore, CancellationToken cancellationToken) =>
            HandleBlobPutAsync(context, hash, blobStore, blobHttpOptions, cancellationToken));
        app.MapPut("/api/blobs/{hash}", (HttpContext context, string hash, IBlobStoreProvider blobStore, CancellationToken cancellationToken) =>
            HandleBlobPutAsync(context, hash, blobStore, blobHttpOptions, cancellationToken));

        // Blob URL resolution (docs/BLOB_HTTP_SURFACE.md "Blob URL
        // resolution (optional capability)", slice S4.1). Frozen shape:
        // GET /blobs/{hash}/url?op=get|put[&size=&contentType=] ->
        // 200 {"url":..., "expiresAtUtc":...} | 501 (no backend) | 400
        // (malformed hash).
        app.MapGet("/blobs/{hash}/url", (HttpContext context, string hash, [AsParameters] BlobUrlOpQuery query, CancellationToken cancellationToken) =>
            HandleBlobUrlResolveAsync(context, hash, query.Op, query.Size, query.ContentType, "default", "blobs", blobHttpOptions, cancellationToken));
        app.MapGet("/api/blobs/{hash}/url", (HttpContext context, string hash, [AsParameters] BlobUrlOpQuery query, CancellationToken cancellationToken) =>
            HandleBlobUrlResolveAsync(context, hash, query.Op, query.Size, query.ContentType, "default", "blobs", blobHttpOptions, cancellationToken));

        // Upload confirmation (docs/BLOB_HTTP_SURFACE.md, same section).
        // The .NET host has no bucket visibility today, so it takes the
        // MAY-accept branch: 200 once a resolver is registered, 501
        // otherwise, 400 on malformed hash.
        app.MapPost("/blobs/{hash}/uploaded", (HttpContext context, string hash) =>
            HandleBlobUploadedConfirmAsync(context, hash, blobHttpOptions));
        app.MapPost("/api/blobs/{hash}/uploaded", (HttpContext context, string hash) =>
            HandleBlobUploadedConfirmAsync(context, hash, blobHttpOptions));

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

        app.Map("/ws/runtime", HandleRuntimeWebSocketAsync);
        app.Map("/ws/{roomId}", HandleRuntimeWebSocketAsync);

        return app;
    }

    private static async Task HandleRuntimeWebSocketAsync(HttpContext context)
    {
        var logger = context.RequestServices
            .GetRequiredService<ILoggerFactory>()
            .CreateLogger("NodalMerge.RuntimeWs");

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
        var peerLocalPersistence = context.RequestServices.GetRequiredService<RuntimePeerLocalPersistenceService>();
        var tokenValidationService = context.RequestServices.GetRequiredService<RuntimeTokenValidationService>();
        var state = new RuntimeConnectionState(sessionIds.Next());

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
            await loopRunner.RunAsync(
                socket,
                frameProcessor,
                state,
                roomBroker,
                tokenValidationService,
                dagPersistence,
                peerLocalPersistence,
                context.RequestAborted
            );
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

    private static async Task<IResult> HandleTokenMintAsync(
        SyncTokenMintRequest request,
        IRoomTokenAuthProvider authProvider,
        CancellationToken cancellationToken
    )
    {
        if (string.IsNullOrWhiteSpace(request.Room))
            return Results.BadRequest(new { error = "room is required" });

        if (string.IsNullOrWhiteSpace(request.PeerPubkeyHex))
            return Results.BadRequest(new { error = "peerPubkeyHex is required" });

        try
        {
            var mintResult = await authProvider.MintAsync(
                new RoomTokenMintRequest(
                    request.Room,
                    request.PeerPubkeyHex,
                    request.LifetimeSeconds,
                    request.Capabilities,
                    request.CapabilityProfileVersion
                ),
                cancellationToken
            );

            if (!mintResult.IsSupported)
                return Results.StatusCode(StatusCodes.Status501NotImplemented);

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
            if (ex.IsTimeout) return Results.StatusCode(StatusCodes.Status504GatewayTimeout);
            if (ex.StatusCode is >= 500) return Results.StatusCode(StatusCodes.Status502BadGateway);
            return Results.BadRequest(new { error = ex.Message });
        }
        catch (InvalidOperationException ex)
        {
            return Results.BadRequest(new { error = ex.Message });
        }
    }

    private static async Task<IResult> HandleTokenValidateAsync(
        SyncTokenValidateRequest request,
        IRoomTokenAuthProvider authProvider,
        CancellationToken cancellationToken
    )
    {
        if (string.IsNullOrWhiteSpace(request.Room))
            return Results.BadRequest(new { error = "room is required" });

        if (string.IsNullOrWhiteSpace(request.PeerPubkeyHex))
            return Results.BadRequest(new { error = "peer_pubkey_hex is required" });

        if (string.IsNullOrWhiteSpace(request.SigHex))
            return Results.BadRequest(new { error = "sig_hex is required" });

        try
        {
            var validation = await authProvider.ValidateAsync(
                new RoomTokenValidationRequest(
                    request.Room,
                    request.PeerPubkeyHex,
                    request.ExpirySecs,
                    request.Capabilities,
                    request.CapabilityProfileVersion,
                    request.SigHex
                ),
                cancellationToken
            );

            return Results.Ok(new { valid = validation.IsValid, reason = validation.Reason });
        }
        catch (SidecarAuthProviderException ex)
        {
            if (ex.IsTimeout) return Results.StatusCode(StatusCodes.Status504GatewayTimeout);
            if (ex.StatusCode is >= 500) return Results.StatusCode(StatusCodes.Status502BadGateway);
            return Results.BadRequest(new { error = ex.Message });
        }
        catch (InvalidOperationException ex)
        {
            return Results.BadRequest(new { error = ex.Message });
        }
    }

    /// <summary>
    /// Legacy alias of <c>GET /blobs/{hash}/url</c> (docs/BLOB_HTTP_SURFACE.md
    /// "Blob URL resolution"). Pre-S4.1 callers passed <c>hash</c>/<c>room</c>/
    /// <c>namespace</c> as query parameters instead of a route segment; this
    /// shim maps that shape onto the frozen handler so both routes share one
    /// implementation, one response shape, and one set of status codes.
    /// </summary>
    private static Task<IResult> HandleBlobUrlAsync(
        HttpContext context,
        BlobUrlQuery query,
        BlobHttpOptions options,
        CancellationToken cancellationToken
    )
    {
        var room = string.IsNullOrWhiteSpace(query.Room) ? "default" : query.Room;
        var scope = string.IsNullOrWhiteSpace(query.Namespace) ? "assets" : query.Namespace;

        return HandleBlobUrlResolveAsync(
            context,
            query.Hash,
            query.Op,
            query.Size,
            query.ContentType,
            room,
            scope,
            options,
            cancellationToken
        );
    }

    /// <summary>
    /// <c>GET /blobs/{hash}/url?op=get|put[&amp;size=&amp;contentType=]</c> —
    /// the frozen URL-resolution contract (docs/BLOB_HTTP_SURFACE.md "Blob URL
    /// resolution (optional capability)", slice S4.1). Shared by the new
    /// route and the legacy <c>/sync/blob-url</c> alias.
    /// </summary>
    private static async Task<IResult> HandleBlobUrlResolveAsync(
        HttpContext context,
        string hash,
        string? opRaw,
        long? size,
        string? contentType,
        string room,
        string ns,
        BlobHttpOptions options,
        CancellationToken cancellationToken)
    {
        var (validationStatus, validationError) = ValidateBlobRequest(context, hash, options);
        if (validationStatus != StatusCodes.Status200OK)
        {
            return Results.Json(new { error = validationError }, statusCode: validationStatus);
        }

        var op = (opRaw ?? string.Empty).Trim().ToLowerInvariant();
        if (op != "get" && op != "put")
        {
            return Results.Json(new { error = "op must be 'get' or 'put'" }, statusCode: StatusCodes.Status400BadRequest);
        }

        if (op == "put" && (size is null || size <= 0))
        {
            return Results.Json(
                new { error = "size must be a positive integer for op=put" },
                statusCode: StatusCodes.Status400BadRequest
            );
        }

        // A missing resolver and a resolver that returns null (no
        // presign-capable backend behind it — e.g. the WsOnly/File
        // compositions' always-null stub) are both "no delegated backend
        // configured" and both surface as 501; the frozen contract doesn't
        // require distinguishing them (docs/BLOB_HTTP_SURFACE.md).
        var resolver = context.RequestServices.GetService<IBlobUrlResolverProvider>();
        if (resolver is null)
        {
            return Results.StatusCode(StatusCodes.Status501NotImplemented);
        }

        var url = op == "put"
            ? await resolver.ResolvePutUrlAsync(
                new BlobPutUrlRequest(room, ns, hash, size ?? 0, contentType),
                cancellationToken)
            : await resolver.ResolveGetUrlAsync(
                new BlobGetUrlRequest(room, ns, hash),
                cancellationToken);

        if (url is null)
        {
            return Results.StatusCode(StatusCodes.Status501NotImplemented);
        }

        return Results.Ok(new
        {
            url = url.Url,
            expiresAtUtc = url.ExpiresAtUtc.UtcDateTime.ToString("o")
        });
    }

    /// <summary>
    /// <c>POST /blobs/{hash}/uploaded</c> — upload confirmation
    /// (docs/BLOB_HTTP_SURFACE.md "Blob URL resolution (optional
    /// capability)", slice S4.1). The .NET host has no bucket visibility
    /// today (<see cref="IBlobUrlResolverProvider"/> has no HEAD/verify
    /// hook), so it takes the contract's MAY-accept branch: accept without
    /// verification once a resolver is registered, mirroring the Rust
    /// delegate auth mode's <c>verify_uploaded</c> default of <c>Ok(())</c>
    /// (see docs/delegated-storage-gc.md's Uploading -&gt; Active lifecycle).
    /// </summary>
    private static IResult HandleBlobUploadedConfirmAsync(
        HttpContext context,
        string hash,
        BlobHttpOptions options)
    {
        var (validationStatus, validationError) = ValidateBlobRequest(context, hash, options);
        if (validationStatus != StatusCodes.Status200OK)
        {
            return Results.Json(new { error = validationError }, statusCode: validationStatus);
        }

        var resolver = context.RequestServices.GetService<IBlobUrlResolverProvider>();
        if (resolver is null)
        {
            return Results.StatusCode(StatusCodes.Status501NotImplemented);
        }

        return Results.Ok(new { accepted = true });
    }

    // -- Blob origin surface (docs/BLOB_HTTP_SURFACE.md) ---------------------

    private static async Task<IResult> HandleBlobGetAsync(
        HttpContext context,
        string hash,
        IBlobStoreProvider blobStore,
        BlobHttpOptions options,
        CancellationToken cancellationToken)
    {
        var (validationStatus, validationError) = ValidateBlobRequest(context, hash, options);
        if (validationStatus != StatusCodes.Status200OK)
        {
            return Results.Json(new { error = validationError }, statusCode: validationStatus);
        }

        // Content encoding (reserved v1.1, docs/BLOB_HTTP_SURFACE.md): serve
        // the stored zstd frame as-is — no recompress-on-serve — when the
        // client asked for it and the store can produce it directly.
        if (AcceptsZstdEncoding(context) && blobStore is IEncodedBlobSource encodedSource)
        {
            var encoded = await encodedSource.TryGetEncodedBlobAsync(hash, cancellationToken);
            if (encoded.Found && string.Equals(encoded.ContentEncoding, "zstd", StringComparison.Ordinal))
            {
                context.Response.Headers["ETag"] = $"\"{hash}\"";
                context.Response.Headers["Content-Encoding"] = "zstd";
                return Results.File(encoded.Bytes!, "application/octet-stream");
            }
        }

        var (status, bytes, error) = await ResolveBlobReadAsync(context, hash, blobStore, options, cancellationToken);
        if (status != StatusCodes.Status200OK)
        {
            return Results.Json(new { error }, statusCode: status);
        }

        context.Response.Headers["ETag"] = $"\"{hash}\"";
        return Results.File(bytes!, "application/octet-stream");
    }

    /// <summary>
    /// True when the request's <c>Accept-Encoding</c> header lists
    /// <c>zstd</c> as one of its (comma-separated, optionally
    /// <c>;q=</c>-weighted) tokens.
    /// </summary>
    private static bool AcceptsZstdEncoding(HttpContext context)
    {
        var header = context.Request.Headers.AcceptEncoding.ToString();
        if (string.IsNullOrWhiteSpace(header))
        {
            return false;
        }

        foreach (var rawToken in header.Split(','))
        {
            var token = rawToken.AsSpan().Trim();
            var semicolon = token.IndexOf(';');
            if (semicolon >= 0)
            {
                token = token[..semicolon].Trim();
            }

            if (token.Equals("zstd", StringComparison.OrdinalIgnoreCase))
            {
                return true;
            }
        }

        return false;
    }

    /// <summary>
    /// HEAD /blobs/{hash} (docs/BLOB_HTTP_SURFACE.md: "Exists for cheap
    /// existence checks"). Slice 2.1 — deliberately NOT a thin wrapper around
    /// <see cref="ResolveBlobReadAsync"/>/<c>TryGetBlobAsync</c> anymore: that
    /// forced a full remote download + BLAKE3 verify + local write-back under
    /// ChainedRemote/S3Direct just to answer "does this exist?". Answers via
    /// <see cref="IBlobStoreProvider.ExistsAsync"/> instead, mirroring the
    /// Rust reference's <c>head_blob</c> (<c>blob_http.rs:256</c>), which
    /// answers via <c>has_blob</c> rather than <c>get_blob</c> for the same
    /// reason.
    ///
    /// Also matches Rust in NOT setting <c>Content-Length</c>: <c>head_blob</c>
    /// only ever sets <c>Content-Type</c> + <c>ETag</c> on 200 (never
    /// Content-Length), and the frozen golden vector <c>head-found</c>
    /// (engine/commands/blob-http-surface-vectors.v1.json) doesn't require
    /// one either. Reporting an accurate Content-Length would require reading
    /// (and, for a zstd-encoded blob, decompressing) the bytes — exactly the
    /// cost this method exists to avoid.
    /// </summary>
    private static async Task<IResult> HandleBlobHeadAsync(
        HttpContext context,
        string hash,
        IBlobStoreProvider blobStore,
        BlobHttpOptions options,
        CancellationToken cancellationToken)
    {
        var (validationStatus, _) = ValidateBlobRequest(context, hash, options);
        if (validationStatus != StatusCodes.Status200OK)
        {
            // HEAD never carries a body, so error responses are status-only.
            return Results.StatusCode(validationStatus);
        }

        var exists = await blobStore.ExistsAsync(hash, cancellationToken);
        if (!exists)
        {
            return Results.StatusCode(StatusCodes.Status404NotFound);
        }

        context.Response.Headers["ETag"] = $"\"{hash}\"";
        context.Response.ContentType = "application/octet-stream";
        return Results.Empty;
    }

    private static (int Status, string? Error) ValidateBlobRequest(HttpContext context, string hash, BlobHttpOptions options)
    {
        if (!IsCanonicalBlobHash(hash))
        {
            return (StatusCodes.Status400BadRequest, "non-canonical hash");
        }

        if (!IsAuthorizedBlobRequest(context, options))
        {
            return (StatusCodes.Status401Unauthorized, "unauthorized");
        }

        return (StatusCodes.Status200OK, null);
    }

    private static async Task<(int Status, byte[]? Bytes, string? Error)> ResolveBlobReadAsync(
        HttpContext context,
        string hash,
        IBlobStoreProvider blobStore,
        BlobHttpOptions options,
        CancellationToken cancellationToken)
    {
        var (validationStatus, validationError) = ValidateBlobRequest(context, hash, options);
        if (validationStatus != StatusCodes.Status200OK)
        {
            return (validationStatus, null, validationError);
        }

        var result = await blobStore.TryGetBlobAsync(hash, cancellationToken);
        if (!result.Found)
        {
            return (StatusCodes.Status404NotFound, null, "not found");
        }

        return (StatusCodes.Status200OK, result.Bytes, null);
    }

    private static async Task<IResult> HandleBlobPutAsync(
        HttpContext context,
        string hash,
        IBlobStoreProvider blobStore,
        BlobHttpOptions options,
        CancellationToken cancellationToken)
    {
        if (!IsCanonicalBlobHash(hash))
        {
            return Results.Json(new { error = "non-canonical hash" }, statusCode: StatusCodes.Status400BadRequest);
        }

        if (!IsAuthorizedBlobRequest(context, options))
        {
            return Results.Json(new { error = "unauthorized" }, statusCode: StatusCodes.Status401Unauthorized);
        }

        var request = context.Request;
        if (request.ContentLength is { } contentLength && contentLength > options.MaxBlobBytes)
        {
            return Results.Json(new { error = "payload too large" }, statusCode: StatusCodes.Status413PayloadTooLarge);
        }

        // Raise Kestrel's default per-request body size cap (~30 MB) so bodies up
        // to MaxBlobBytes aren't rejected by the transport before our own cap
        // (ReadBlobBodyWithCapAsync, below) gets a chance to apply.
        var maxRequestBodySizeFeature = context.Features.Get<IHttpMaxRequestBodySizeFeature>();
        if (maxRequestBodySizeFeature is { IsReadOnly: false })
        {
            maxRequestBodySizeFeature.MaxRequestBodySize = options.MaxBlobBytes;
        }

        var (bytes, tooLarge) = await ReadBlobBodyWithCapAsync(request.Body, options.MaxBlobBytes, cancellationToken);
        if (tooLarge)
        {
            return Results.Json(new { error = "payload too large" }, statusCode: StatusCodes.Status413PayloadTooLarge);
        }

        var actualHash = Hasher.Hash(bytes!).ToString();
        if (!string.Equals(actualHash, hash, StringComparison.Ordinal))
        {
            return Results.Json(new { error = "hash mismatch" }, statusCode: StatusCodes.Status422UnprocessableEntity);
        }

        // Slice 2.1: idempotency's already-present check goes through the
        // cheap ExistsAsync probe, not a full TryGetBlobAsync read — matches
        // the Rust reference (blob_http.rs:515 uses has_blob, not get_blob).
        var alreadyExists = await blobStore.ExistsAsync(hash, cancellationToken);
        if (alreadyExists)
        {
            // Content-addressed: identical bytes are already stored — idempotent no-op.
            return Results.StatusCode(StatusCodes.Status200OK);
        }

        await blobStore.PutBlobAsync(hash, bytes!, request.ContentType, cancellationToken);
        return Results.StatusCode(StatusCodes.Status201Created);
    }

    /// <summary>
    /// Buffers a request body up to (and one byte past) <paramref name="maxBytes"/>.
    /// Chunked bodies carry no <c>Content-Length</c>, so this is the only
    /// enforcement point for them; bodies with a declared length are also caught
    /// earlier by the immediate <c>Content-Length</c> check in the caller.
    /// </summary>
    private static async Task<(byte[]? Bytes, bool TooLarge)> ReadBlobBodyWithCapAsync(
        Stream body,
        long maxBytes,
        CancellationToken cancellationToken)
    {
        using var buffer = new MemoryStream();
        var readBuffer = new byte[81920];
        long total = 0;
        int read;
        while ((read = await body.ReadAsync(readBuffer, cancellationToken)) > 0)
        {
            total += read;
            if (total > maxBytes)
            {
                return (null, true);
            }
            buffer.Write(readBuffer, 0, read);
        }

        return (buffer.ToArray(), false);
    }

    private static bool IsCanonicalBlobHash(string hash)
    {
        if (hash.Length != 64)
        {
            return false;
        }

        foreach (var c in hash)
        {
            if (c is < '0' or > '9' && c is < 'a' or > 'f')
            {
                return false;
            }
        }

        return true;
    }

    private static bool IsAuthorizedBlobRequest(HttpContext context, BlobHttpOptions options)
    {
        if (string.IsNullOrEmpty(options.AuthToken))
        {
            return true;
        }

        const string bearerPrefix = "Bearer ";
        var authHeader = context.Request.Headers.Authorization.ToString();
        if (!authHeader.StartsWith(bearerPrefix, StringComparison.Ordinal))
        {
            return false;
        }

        var providedBytes = Encoding.UTF8.GetBytes(authHeader[bearerPrefix.Length..]);
        var expectedBytes = Encoding.UTF8.GetBytes(options.AuthToken);
        return CryptographicOperations.FixedTimeEquals(providedBytes, expectedBytes);
    }

    private static void EmitAuthReadinessChecks(
        IServiceProvider services,
        NodalMergeHostProviderOptions providerOptions,
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
}
