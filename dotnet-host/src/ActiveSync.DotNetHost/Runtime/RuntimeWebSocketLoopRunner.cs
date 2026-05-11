using System.Net.WebSockets;
using System.Text;
using System.Text.Json.Nodes;

namespace ActiveSync.DotNetHost.Runtime;

public sealed class RuntimeWebSocketLoopRunner
{
    public const int MaxInboundMessageBytes = 64 * 1024;

    public Task RunAsync(
        WebSocket socket,
        RuntimeFrameProcessor frameProcessor,
        RuntimeConnectionState state,
        CancellationToken cancellationToken
    )
    {
        return RunAsync(socket, frameProcessor, state, roomBroker: null, cancellationToken);
    }

    public async Task RunAsync(
        WebSocket socket,
        RuntimeFrameProcessor frameProcessor,
        RuntimeConnectionState state,
        RuntimeRoomBroker? roomBroker = null,
        CancellationToken cancellationToken = default
    )
    {
        roomBroker ??= new RuntimeRoomBroker();
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

                var processResult = frameProcessor.ProcessFrame(
                    result.MessageType,
                    messageBuffer.ToArray(),
                    state
                );

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
                                ["pubkey"] = state.PeerPubkeyHex
                            }.ToJsonString();

                            await roomBroker.BroadcastAsync(
                                state.RoomId,
                                peerJoined,
                                excludeSessionId: state.SessionId,
                                cancellationToken: cancellationToken
                            );
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
                    && TryBuildPackRelay(messageBuffer.ToArray(), state, out var packRelayJson))
                {
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
            if (registeredInRoom)
            {
                roomBroker.Unregister(state);

                if (!string.IsNullOrWhiteSpace(state.RoomId) && !string.IsNullOrWhiteSpace(state.PeerPubkeyHex))
                {
                    var peerLeft = new JsonObject
                    {
                        ["type"] = "peer-left",
                        ["room"] = state.RoomId,
                        ["from"] = state.PeerPubkeyHex
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

    private static bool TryBuildPackRelay(byte[] payload, RuntimeConnectionState state, out string relayJson)
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
            ["nodes"] = nodesB64
        }.ToJsonString();

        return true;
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