using System.Collections.Concurrent;
using System.Net.WebSockets;
using System.Text;
using Microsoft.Extensions.Logging;

namespace NodalMerge.DotNetHost.Runtime;

public sealed class RuntimeRoomBroker
{
    private readonly ILogger<RuntimeRoomBroker> _logger;

    public RuntimeRoomBroker(ILogger<RuntimeRoomBroker> logger)
    {
        _logger = logger;
    }

    private readonly ConcurrentDictionary<string, ConcurrentDictionary<ulong, RuntimeRoomConnection>> _rooms =
        new(StringComparer.Ordinal);

    // Secondary index: stable peer_id → current session_id, for reconnect targeting.
    private readonly ConcurrentDictionary<string, ulong> _peerIdToSession =
        new(StringComparer.Ordinal);

    public RuntimeRoomRegistrationResult Register(WebSocket socket, RuntimeConnectionState state)
    {
        if (!state.IsInitialized
            || string.IsNullOrWhiteSpace(state.RoomId)
            || string.IsNullOrWhiteSpace(state.PeerPubkeyHex))
        {
            return RuntimeRoomRegistrationResult.NotRegistered;
        }

        var roomId = state.RoomId;
        var room = _rooms.GetOrAdd(roomId, _ => new ConcurrentDictionary<ulong, RuntimeRoomConnection>());

        var connection = new RuntimeRoomConnection(state.SessionId, roomId, state.PeerPubkeyHex, state.PeerId, state.PeerType, socket);
        room[state.SessionId] = connection;

        if (!string.IsNullOrWhiteSpace(state.PeerId))
        {
            _peerIdToSession[state.PeerId] = state.SessionId;
        }

        var peers = room.Values
            .Where(candidate => candidate.SessionId != state.SessionId)
            .Select(candidate => candidate.PeerPubkeyHex)
            .Distinct(StringComparer.Ordinal)
            .ToArray();

        _logger.LogInformation(
            "runtime room register room={Room} session={Session} peer={Peer} peerId={PeerId} peerType={PeerType} roomPeers={PeerCount}",
            roomId,
            state.SessionId,
            state.PeerPubkeyHex,
            state.PeerId ?? "-",
            state.PeerType ?? "ui",
            room.Count
        );

        return RuntimeRoomRegistrationResult.Registered(peers);
    }

    public bool Unregister(RuntimeConnectionState state)
    {
        if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
        {
            return false;
        }

        var roomId = state.RoomId;
        if (!_rooms.TryGetValue(roomId, out var room))
        {
            return false;
        }

        room.TryRemove(state.SessionId, out _);

        if (!string.IsNullOrWhiteSpace(state.PeerId))
        {
            _peerIdToSession.TryRemove(state.PeerId, out _);
        }

        _logger.LogInformation(
            "runtime room unregister room={Room} session={Session} peer={Peer} peerId={PeerId} roomPeers={PeerCount}",
            roomId,
            state.SessionId,
            state.PeerPubkeyHex,
            state.PeerId ?? "-",
            room.Count
        );

        if (room.IsEmpty)
        {
            _rooms.TryRemove(roomId, out _);
            return true;
        }

        return false;
    }

    public bool TryGetSessionByPeerId(string peerId, out ulong sessionId) =>
        _peerIdToSession.TryGetValue(peerId, out sessionId);

    public async Task BroadcastAsync(
        string roomId,
        string outboundMessage,
        ulong? excludeSessionId = null,
        string? targetPeerPubkey = null,
        string? targetPeerId = null,
        CancellationToken cancellationToken = default
    )
    {
        if (!_rooms.TryGetValue(roomId, out var room))
        {
            _logger.LogInformation("runtime room broadcast skip room={Room} reason=no-room", roomId);
            return;
        }

        var snapshot = room.Values.ToArray();
        var attempted = 0;
        var delivered = 0;
        foreach (var connection in snapshot)
        {
            if (excludeSessionId.HasValue && connection.SessionId == excludeSessionId.Value)
            {
                continue;
            }

            if (!string.IsNullOrWhiteSpace(targetPeerPubkey)
                && !string.Equals(connection.PeerPubkeyHex, targetPeerPubkey, StringComparison.Ordinal))
            {
                continue;
            }

            if (!string.IsNullOrWhiteSpace(targetPeerId)
                && !string.Equals(connection.PeerId, targetPeerId, StringComparison.Ordinal))
            {
                continue;
            }

            attempted += 1;

            var sent = await connection.TrySendTextAsync(outboundMessage, cancellationToken);
            if (!sent)
            {
                room.TryRemove(connection.SessionId, out _);
            }
            else
            {
                delivered += 1;
            }
        }

        _logger.LogInformation(
            "runtime room broadcast room={Room} attempted={Attempted} delivered={Delivered} excludeSession={ExcludeSession} targetPeer={TargetPeer} targetPeerId={TargetPeerId}",
            roomId,
            attempted,
            delivered,
            excludeSessionId,
            targetPeerPubkey ?? "*",
            targetPeerId ?? "*"
        );

        if (room.IsEmpty)
        {
            _rooms.TryRemove(roomId, out _);
        }
    }
}

internal sealed class RuntimeRoomConnection
{
    private readonly WebSocket _socket;
    private readonly SemaphoreSlim _sendLock = new(1, 1);

    public RuntimeRoomConnection(ulong sessionId, string roomId, string peerPubkeyHex, string? peerId, string? peerType, WebSocket socket)
    {
        SessionId = sessionId;
        RoomId = roomId;
        PeerPubkeyHex = peerPubkeyHex;
        PeerId = peerId;
        PeerType = peerType;
        _socket = socket;
    }

    public ulong SessionId { get; }
    public string RoomId { get; }
    public string PeerPubkeyHex { get; }
    public string? PeerId { get; }
    public string? PeerType { get; }

    public async Task<bool> TrySendTextAsync(string text, CancellationToken cancellationToken)
    {
        if (!CanWrite(_socket.State))
        {
            return false;
        }

        await _sendLock.WaitAsync(cancellationToken);
        try
        {
            if (!CanWrite(_socket.State))
            {
                return false;
            }

            var payload = Encoding.UTF8.GetBytes(text);
            await _socket.SendAsync(payload, WebSocketMessageType.Text, true, cancellationToken);
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
        finally
        {
            _sendLock.Release();
        }
    }

    private static bool CanWrite(WebSocketState state)
    {
        return state == WebSocketState.Open;
    }
}

public sealed record RuntimeRoomRegistrationResult(bool IsRegistered, IReadOnlyList<string> ExistingPeers)
{
    public static RuntimeRoomRegistrationResult Registered(IReadOnlyList<string> existingPeers) =>
        new(true, existingPeers);

    public static RuntimeRoomRegistrationResult NotRegistered { get; } = new(false, []);
}