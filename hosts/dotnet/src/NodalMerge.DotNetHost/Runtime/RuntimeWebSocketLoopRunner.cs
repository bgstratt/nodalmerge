using System.Net.WebSockets;
using System.Diagnostics.Metrics;
using System.Text;
using System.Text.Json.Nodes;
using Microsoft.Extensions.Logging;
using Microsoft.Extensions.Logging.Abstractions;
using NodalMerge.Host.Abstractions.Providers;

namespace NodalMerge.DotNetHost.Runtime;

public sealed class RuntimeWebSocketLoopRunner
{
    private static readonly Meter RuntimeWsMeter = new("NodalMerge.DotNetHost.RuntimeWs", "1.0.0");
    private static readonly Counter<long> RuntimeWsConnectionsOpenedCounter = RuntimeWsMeter.CreateCounter<long>(
        "runtime_ws_connections_opened_total"
    );
    private static readonly Counter<long> RuntimeWsConnectionsClosedCounter = RuntimeWsMeter.CreateCounter<long>(
        "runtime_ws_connections_closed_total"
    );
    private static readonly Counter<long> RuntimeWsInboundMessagesCounter = RuntimeWsMeter.CreateCounter<long>(
        "runtime_ws_inbound_messages_total"
    );
    private static readonly Counter<long> RuntimeWsPackRelayCounter = RuntimeWsMeter.CreateCounter<long>(
        "runtime_ws_pack_relay_total"
    );

    private readonly ILogger<RuntimeWebSocketLoopRunner> _logger;
    private readonly IReadOnlyList<IInboundPackObserver> _inboundPackObservers;

    public RuntimeWebSocketLoopRunner()
        : this(NullLogger<RuntimeWebSocketLoopRunner>.Instance)
    {
    }

    public RuntimeWebSocketLoopRunner(ILogger<RuntimeWebSocketLoopRunner> logger)
        : this(logger, Array.Empty<IInboundPackObserver>())
    {
    }

    // S7.1: additive-only constructor overload. Resolved automatically by the DI container
    // (services.AddSingleton<RuntimeWebSocketLoopRunner>() in ServiceCollectionExtensions picks
    // the constructor with the most resolvable parameters) — IEnumerable<T> always resolves, to an
    // empty sequence when no IInboundPackObserver is registered, so unregistered hosts and every
    // existing direct `new RuntimeWebSocketLoopRunner(...)` call site (both existing ctors are
    // untouched) get byte-identical behavior to before this slice.
    public RuntimeWebSocketLoopRunner(
        ILogger<RuntimeWebSocketLoopRunner> logger,
        IEnumerable<IInboundPackObserver> inboundPackObservers)
    {
        _logger = logger;
        _inboundPackObservers = inboundPackObservers as IReadOnlyList<IInboundPackObserver>
            ?? inboundPackObservers.ToArray();
    }

    public const int MaxInboundMessageBytes = 64 * 1024;

    public Task RunAsync(
        WebSocket socket,
        RuntimeFrameProcessor frameProcessor,
        RuntimeConnectionState state,
        CancellationToken cancellationToken
    )
    {
        return RunAsync(
            socket,
            frameProcessor,
            state,
            roomBroker: null,
            tokenValidationService: null,
            dagPersistenceService: null,
            peerLocalPersistenceService: null,
            cancellationToken
        );
    }

    public async Task RunAsync(
        WebSocket socket,
        RuntimeFrameProcessor frameProcessor,
        RuntimeConnectionState state,
        RuntimeRoomBroker? roomBroker = null,
        RuntimeTokenValidationService? tokenValidationService = null,
        RuntimeDagPersistenceService? dagPersistenceService = null,
        RuntimePeerLocalPersistenceService? peerLocalPersistenceService = null,
        CancellationToken cancellationToken = default
    )
    {
        roomBroker ??= new RuntimeRoomBroker(NullLogger<RuntimeRoomBroker>.Instance);
        dagPersistenceService ??= null;
        peerLocalPersistenceService ??= null;
        var connectionTraceId = GetOrCreateTraceId(state);
        RuntimeWsConnectionsOpenedCounter.Add(
            1,
            KeyValuePair.Create<string, object?>("session", state.SessionId.ToString()),
            KeyValuePair.Create<string, object?>("trace", connectionTraceId)
        );
        var registeredInRoom = false;
        try
        {
            var frameBuffer = new byte[32 * 1024];

            while (socket.State == WebSocketState.Open)
            {
                using var messageBuffer = new MemoryStream();
                WebSocketReceiveResult? result;
                var messageTooLarge = false;

                do
                {
                    result = await socket.ReceiveAsync(frameBuffer, cancellationToken);

                    if (result.MessageType == WebSocketMessageType.Close)
                    {
                        await TryCloseNormalAsync(
                            socket,
                            "client requested close",
                            cancellationToken
                        );
                        return;
                    }

                    if (result.Count > 0)
                    {
                        if (messageBuffer.Length + result.Count > MaxInboundMessageBytes)
                        {
                            messageTooLarge = true;
                        }
                        else
                        {
                            await messageBuffer.WriteAsync(frameBuffer.AsMemory(0, result.Count), cancellationToken);
                        }
                    }
                }
                while (!result.EndOfMessage);

                if (messageTooLarge)
                {
                    var sentTooLarge = await TrySendTextAsync(
                        socket,
                        RuntimeErrorEnvelopeBuilder.BuildMessageError("message too large"),
                        cancellationToken
                    );
                    if (!sentTooLarge)
                    {
                        return;
                    }
                    continue;
                }

                if (result.MessageType == WebSocketMessageType.Text && tokenValidationService is not null)
                {
                    var tokenValidation = await tokenValidationService.ValidateInboundAsync(
                        messageBuffer.ToArray(),
                        state,
                        cancellationToken
                    );
                    if (!tokenValidation.Allowed)
                    {
                        var validationError = RuntimeFrameProcessResult.FromOutboundMessages(
                            dispatchSucceeded: false,
                            [RuntimeErrorEnvelopeBuilder.BuildMessageError(tokenValidation.ErrorMessage ?? "invalid token")],
                            shouldCloseConnection: false
                        );

                        foreach (var outbound in validationError.OutboundMessages)
                        {
                            var sent = await TrySendTextAsync(socket, outbound, cancellationToken);
                            if (!sent)
                            {
                                return;
                            }
                        }

                        continue;
                    }
                }

                var inboundJson = result.MessageType == WebSocketMessageType.Text
                    ? ParseJsonObject(messageBuffer.ToArray())
                    : null;
                var inboundType = inboundJson is not null
                    ? ReadString(inboundJson["type"])
                    : null;
                var inboundRoom = inboundJson is not null
                    ? ReadString(inboundJson["room"])
                    : null;
                var inboundTraceId = inboundJson is not null
                    ? ReadString(inboundJson["trace_id"])
                    : null;

                if (!string.IsNullOrWhiteSpace(inboundTraceId))
                {
                    state.TraceId = inboundTraceId;
                }

                var traceId = GetOrCreateTraceId(state);

                if (!state.IsInitialized
                    && !string.IsNullOrWhiteSpace(inboundType)
                    && (string.Equals(inboundType, "hello", StringComparison.OrdinalIgnoreCase)
                        || string.Equals(inboundType, "client-hello", StringComparison.OrdinalIgnoreCase))
                    )
                {
                    var hydrateRoomId = !string.IsNullOrWhiteSpace(inboundRoom)
                        ? inboundRoom
                        : state.RoomId;
                    if (!string.IsNullOrWhiteSpace(hydrateRoomId))
                    {
                        if (dagPersistenceService is not null)
                        {
                            await dagPersistenceService.HydrateRoomIfNeededAsync(hydrateRoomId, cancellationToken);
                        }

                        if (peerLocalPersistenceService is not null)
                        {
                            await peerLocalPersistenceService.HydrateRoomIfNeededAsync(
                                hydrateRoomId,
                                cancellationToken
                            );
                        }
                    }
                }

                var processResult = frameProcessor.ProcessFrame(
                    result.MessageType,
                    messageBuffer.ToArray(),
                    state
                );

                if (result.MessageType == WebSocketMessageType.Text)
                {
                    RuntimeWsInboundMessagesCounter.Add(
                        1,
                        KeyValuePair.Create<string, object?>("room", state.RoomId ?? inboundRoom ?? "<uninitialized>"),
                        KeyValuePair.Create<string, object?>("type", inboundType ?? "<parse-error>"),
                        KeyValuePair.Create<string, object?>("trace", traceId)
                    );
                    _logger.LogInformation(
                        "runtime inbound room={Room} session={Session} trace={Trace} peer={Peer} type={Type} dispatch={Dispatch}",
                        state.RoomId ?? "<uninitialized>",
                        state.SessionId,
                        traceId,
                        state.PeerPubkeyHex ?? "<unknown>",
                        inboundType ?? "<parse-error>",
                        processResult.DispatchSucceeded
                    );

                    if (processResult.DispatchSucceeded
                        && dagPersistenceService is not null
                        && string.Equals(inboundType, "pack", StringComparison.OrdinalIgnoreCase)
                        && !string.IsNullOrWhiteSpace(state.RoomId)
                        && inboundJson is not null)
                    {
                        var nodesB64 = ReadString(inboundJson["nodes"]);
                        if (!string.IsNullOrWhiteSpace(nodesB64))
                        {
                            await dagPersistenceService.PersistInboundPackAsync(state.RoomId!, nodesB64, cancellationToken);
                            if (peerLocalPersistenceService is not null)
                            {
                                await peerLocalPersistenceService.PersistInboundPackAsync(
                                    state.RoomId!,
                                    nodesB64,
                                    cancellationToken
                                );
                            }

                            // S7.1: this is the genuinely peer-authored inbound-apply point on the
                            // server-side WS path (the frame just received from state's socket, of
                            // type "pack", after engine import + persistence above) — not the
                            // broadcast fan-out further down (TryBuildPackRelay/roomBroker.BroadcastAsync),
                            // which is this host's own echo to OTHER peers. Fire-and-forget-safe: each
                            // observer is isolated by its own try/catch so a throwing observer can
                            // never break the loop or undo the persistence that already succeeded.
                            await NotifyInboundPackObserversAsync(state.RoomId!, nodesB64, cancellationToken);

                            // Also persist the room's current server-pack snapshot.
                            // In practice most client writes arrive as `pack` messages,
                            // so relying only on non-pack mutation hooks can leave
                            // persistence with delta-only history that doesn't always
                            // hydrate deterministically on fresh reconnects.
                            await dagPersistenceService.PersistRoomSnapshotAsync(state.RoomId!, cancellationToken);
                        }
                    }

                    if (processResult.DispatchSucceeded
                        && dagPersistenceService is not null
                        && !string.IsNullOrWhiteSpace(state.RoomId)
                        && ShouldPersistSnapshotForMutation(inboundType))
                    {
                        await dagPersistenceService.PersistRoomSnapshotAsync(state.RoomId!, cancellationToken);
                    }
                }

                if (!registeredInRoom && state.IsInitialized)
                {
                    var registration = roomBroker.Register(socket, state);
                    if (registration.IsRegistered)
                    {
                        registeredInRoom = true;
                        if (registration.ExistingPeers.Count > 0)
                        {
                            processResult = processResult with
                            {
                                OutboundMessages = processResult.OutboundMessages
                                    .Select(outbound => MergeWelcomePeers(outbound, registration.ExistingPeers))
                                    .ToArray()
                            };
                        }

                        if (!string.IsNullOrWhiteSpace(state.RoomId) && !string.IsNullOrWhiteSpace(state.PeerPubkeyHex))
                        {
                            var peerJoined = new JsonObject
                            {
                                ["type"] = "peer-joined",
                                ["room"] = state.RoomId,
                                ["from"] = state.PeerPubkeyHex,
                                ["pubkey"] = state.PeerPubkeyHex,
                                ["peer_id"] = state.PeerId,
                                ["peer_type"] = state.PeerType ?? "ui"
                            }.ToJsonString();

                            await roomBroker.BroadcastAsync(
                                state.RoomId,
                                peerJoined,
                                excludeSessionId: state.SessionId,
                                cancellationToken: cancellationToken
                            );
                        }

                        // Send the new peer a catch-up pack with the current server CRDT state so
                        // they are immediately in sync without needing a separate sync-diff round-trip.
                        if (dagPersistenceService is not null && !string.IsNullOrWhiteSpace(state.RoomId))
                        {
                            var nodesB64 = await dagPersistenceService.TryExportRoomPackB64Async(
                                state.RoomId!, cancellationToken);
                            if (!string.IsNullOrWhiteSpace(nodesB64))
                            {
                                var catchUpPack = new JsonObject
                                {
                                    ["type"] = "pack",
                                    ["room"] = state.RoomId,
                                    ["from"] = "server",
                                    ["nodes"] = nodesB64
                                }.ToJsonString();
                                processResult = processResult with
                                {
                                    OutboundMessages = [.. processResult.OutboundMessages, catchUpPack]
                                };
                            }
                        }
                    }
                }

                foreach (var outbound in processResult.OutboundMessages)
                {
                    var sent = await TrySendTextAsync(socket, outbound, cancellationToken);
                    if (!sent)
                    {
                        return;
                    }

                    if (registeredInRoom
                        && !string.IsNullOrWhiteSpace(state.RoomId)
                        && TryParseRelayDirective(outbound, out var relayTargetPeer))
                    {
                        await roomBroker.BroadcastAsync(
                            state.RoomId!,
                            outbound,
                            excludeSessionId: state.SessionId,
                            targetPeerPubkey: relayTargetPeer,
                            cancellationToken: cancellationToken
                        );
                    }
                }

                if (registeredInRoom
                    && processResult.DispatchSucceeded
                    && result.MessageType == WebSocketMessageType.Text
                    && !string.IsNullOrWhiteSpace(state.RoomId)
                    && !string.IsNullOrWhiteSpace(state.PeerPubkeyHex)
                    && TryBuildPackRelay(messageBuffer.ToArray(), state, traceId, out var packRelayJson))
                {
                    RuntimeWsPackRelayCounter.Add(
                        1,
                        KeyValuePair.Create<string, object?>("room", state.RoomId),
                        KeyValuePair.Create<string, object?>("trace", traceId)
                    );
                    _logger.LogInformation(
                        "runtime pack relay room={Room} from={Peer} session={Session} trace={Trace}",
                        state.RoomId,
                        state.PeerPubkeyHex,
                        state.SessionId,
                        traceId
                    );

                    await roomBroker.BroadcastAsync(
                        state.RoomId!,
                        packRelayJson,
                        excludeSessionId: state.SessionId,
                        cancellationToken: cancellationToken
                    );
                }

                if (processResult.ShouldCloseConnection)
                {
                    await TryCloseNormalAsync(
                        socket,
                        "session closed",
                        cancellationToken
                    );
                    return;
                }
            }
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
            // Host shutdown/request abort cancellation should end the loop cleanly.
        }
        catch (WebSocketException)
        {
            // Transport-level receive/send race should end the loop cleanly.
        }
        catch (ObjectDisposedException)
        {
            // Socket was disposed during shutdown/disconnect race.
        }
        finally
        {
            RuntimeWsConnectionsClosedCounter.Add(
                1,
                KeyValuePair.Create<string, object?>("session", state.SessionId.ToString()),
                KeyValuePair.Create<string, object?>("room", state.RoomId ?? "<uninitialized>"),
                KeyValuePair.Create<string, object?>("trace", GetOrCreateTraceId(state))
            );
            if (registeredInRoom)
            {
                var roomBecameEmpty = roomBroker.Unregister(state);

                if (roomBecameEmpty && dagPersistenceService is not null && !string.IsNullOrWhiteSpace(state.RoomId))
                {
                    dagPersistenceService.InvalidateHydration(state.RoomId);
                }

                if (peerLocalPersistenceService is not null && !string.IsNullOrWhiteSpace(state.RoomId))
                {
                    await peerLocalPersistenceService.FlushRoomAsync(state.RoomId, CancellationToken.None);
                }

                if (!string.IsNullOrWhiteSpace(state.RoomId) && !string.IsNullOrWhiteSpace(state.PeerPubkeyHex))
                {
                    var peerLeft = new JsonObject
                    {
                        ["type"] = "peer-left",
                        ["room"] = state.RoomId,
                        ["from"] = state.PeerPubkeyHex,
                        ["peer_id"] = state.PeerId,
                        ["peer_type"] = state.PeerType ?? "ui"
                    }.ToJsonString();

                    await roomBroker.BroadcastAsync(
                        state.RoomId,
                        peerLeft,
                        excludeSessionId: state.SessionId,
                        cancellationToken: CancellationToken.None
                    );
                }
            }
        }
    }

    private static string MergeWelcomePeers(string outbound, IReadOnlyList<string> peers)
    {
        if (peers.Count == 0)
        {
            return outbound;
        }

        JsonNode? parsed;
        try
        {
            parsed = JsonNode.Parse(outbound);
        }
        catch
        {
            return outbound;
        }

        if (parsed is not JsonObject root)
        {
            return outbound;
        }

        var type = root["type"]?.GetValue<string>();
        if (!string.Equals(type, "welcome", StringComparison.OrdinalIgnoreCase))
        {
            return outbound;
        }

        var merged = new HashSet<string>(StringComparer.Ordinal);
        if (root["peers"] is JsonArray existingPeers)
        {
            foreach (var existingPeer in existingPeers)
            {
                var value = existingPeer?.GetValue<string>();
                if (!string.IsNullOrWhiteSpace(value))
                {
                    merged.Add(value);
                }
            }
        }

        foreach (var peer in peers)
        {
            if (!string.IsNullOrWhiteSpace(peer))
            {
                merged.Add(peer);
            }
        }

        root["peers"] = new JsonArray(merged.Select(x => (JsonNode?)x).ToArray());
        return root.ToJsonString();
    }

    private static bool TryParseRelayDirective(string outbound, out string? targetPeerPubkey)
    {
        targetPeerPubkey = null;

        JsonNode? parsed;
        try
        {
            parsed = JsonNode.Parse(outbound);
        }
        catch
        {
            return false;
        }

        if (parsed is not JsonObject root)
        {
            return false;
        }

        var type = root["type"]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(type))
        {
            return false;
        }

        if (string.Equals(type, "presence", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "peer-joined", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "peer-left", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        if (string.Equals(type, "webrtc-offer", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "webrtc-answer", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "webrtc-ice", StringComparison.OrdinalIgnoreCase))
        {
            targetPeerPubkey = root["to"]?.GetValue<string>();
            return true;
        }

        return false;
    }

    private static bool TryBuildPackRelay(byte[] payload, RuntimeConnectionState state, string traceId, out string relayJson)
    {
        relayJson = string.Empty;

        JsonNode? parsed;
        try
        {
            parsed = JsonNode.Parse(payload);
        }
        catch
        {
            return false;
        }

        if (parsed is not JsonObject root)
        {
            return false;
        }

        var type = root["type"]?.GetValue<string>();
        if (!string.Equals(type, "pack", StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        var nodesB64 = root["nodes"]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(nodesB64) || string.IsNullOrWhiteSpace(state.RoomId) || string.IsNullOrWhiteSpace(state.PeerPubkeyHex))
        {
            return false;
        }

        relayJson = new JsonObject
        {
            ["type"] = "pack",
            ["room"] = state.RoomId,
            ["from"] = state.PeerPubkeyHex,
            ["nodes"] = nodesB64,
            ["trace_id"] = traceId
        }.ToJsonString();

        return true;
    }

    private static JsonObject? ParseJsonObject(byte[] payload)
    {
        try
        {
            return JsonNode.Parse(payload) as JsonObject;
        }
        catch
        {
            return null;
        }
    }

    private static string? ReadString(JsonNode? node)
    {
        if (node is JsonValue value && value.TryGetValue<string>(out var s))
        {
            return s;
        }

        return null;
    }

    private async ValueTask NotifyInboundPackObserversAsync(
        string roomId,
        string nodesB64,
        CancellationToken cancellationToken
    )
    {
        if (_inboundPackObservers.Count == 0)
        {
            return;
        }

        foreach (var observer in _inboundPackObservers)
        {
            try
            {
                await observer.OnInboundPackAppliedAsync(roomId, nodesB64, cancellationToken);
            }
            catch (Exception ex)
            {
                _logger.LogWarning(
                    ex,
                    "runtime ws inbound pack observer failed room={Room} observer={Observer}",
                    roomId,
                    observer.GetType().Name
                );
            }
        }
    }

    private static bool ShouldPersistSnapshotForMutation(string? inboundType)
    {
        if (string.IsNullOrWhiteSpace(inboundType))
        {
            return false;
        }

        return inboundType.Equals("map-set", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("map-delete", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("list-push", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("list-insert", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("list-delete", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("list-move", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("list-update", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("text-insert", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("text-delete", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("text-insert-at", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("text-delete-at", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("blob-set", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("set-policy", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("set-room-key", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("presence", StringComparison.OrdinalIgnoreCase)
            || inboundType.Equals("presence-set", StringComparison.OrdinalIgnoreCase);
    }

    private static string GetOrCreateTraceId(RuntimeConnectionState state)
    {
        if (string.IsNullOrWhiteSpace(state.TraceId))
        {
            state.TraceId = $"sess-{state.SessionId}-{Guid.NewGuid():N}";
        }

        return state.TraceId;
    }

    private static Task SendTextAsync(WebSocket socket, string text, CancellationToken cancellationToken)
    {
        var bytes = Encoding.UTF8.GetBytes(text);
        return socket.SendAsync(bytes, WebSocketMessageType.Text, true, cancellationToken);
    }

    private static async Task<bool> TrySendTextAsync(WebSocket socket, string text, CancellationToken cancellationToken)
    {
        if (!CanWrite(socket.State))
        {
            return false;
        }

        try
        {
            await SendTextAsync(socket, text, cancellationToken);
            return true;
        }
        catch (WebSocketException)
        {
            return false;
        }
        catch (ObjectDisposedException)
        {
            return false;
        }
    }

    private static async Task TryCloseNormalAsync(WebSocket socket, string description, CancellationToken cancellationToken)
    {
        if (!CanClose(socket.State))
        {
            return;
        }

        try
        {
            await socket.CloseAsync(WebSocketCloseStatus.NormalClosure, description, cancellationToken);
        }
        catch (WebSocketException)
        {
            // Connection already torn down by peer/transport.
        }
        catch (ObjectDisposedException)
        {
            // Socket was already disposed during shutdown race.
        }
    }

    private static bool CanWrite(WebSocketState state)
    {
        return state == WebSocketState.Open;
    }

    private static bool CanClose(WebSocketState state)
    {
        return state == WebSocketState.Open || state == WebSocketState.CloseReceived;
    }
}