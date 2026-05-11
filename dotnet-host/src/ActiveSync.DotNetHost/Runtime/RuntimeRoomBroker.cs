using System.Collections.Concurrent;
using System.Net.WebSockets;
using System.Text;

namespace ActiveSync.DotNetHost.Runtime;

public sealed class RuntimeRoomBroker
{
    private readonly ConcurrentDictionary<string, ConcurrentDictionary<ulong, RuntimeRoomConnection>> _rooms =
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

        var connection = new RuntimeRoomConnection(state.SessionId, roomId, state.PeerPubkeyHex, socket);
        room[state.SessionId] = connection;

        var peers = room.Values
            .Where(candidate => candidate.SessionId != state.SessionId)
            .Select(candidate => candidate.PeerPubkeyHex)
            .Distinct(StringComparer.Ordinal)
            .ToArray();

        return RuntimeRoomRegistrationResult.Registered(peers);
    }

    public void Unregister(RuntimeConnectionState state)
    {
        if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
        {
            return;
        }

        var roomId = state.RoomId;
        if (!_rooms.TryGetValue(roomId, out var room))
        {
            return;
        }

        room.TryRemove(state.SessionId, out _);

        if (room.IsEmpty)
        {
            _rooms.TryRemove(roomId, out _);
        }
    }

    public async Task BroadcastAsync(
        string roomId,
        string outboundMessage,
        ulong? excludeSessionId = null,
        string? targetPeerPubkey = null,
        CancellationToken cancellationToken = default
    )
    {
        if (!_rooms.TryGetValue(roomId, out var room))
        {
            return;
        }

        var snapshot = room.Values.ToArray();
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

            var sent = await connection.TrySendTextAsync(outboundMessage, cancellationToken);
            if (!sent)
            {
                room.TryRemove(connection.SessionId, out _);
            }
        }

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

    public RuntimeRoomConnection(ulong sessionId, string roomId, string peerPubkeyHex, WebSocket socket)
    {
        SessionId = sessionId;
        RoomId = roomId;
        PeerPubkeyHex = peerPubkeyHex;
        _socket = socket;
    }

    public ulong SessionId { get; }
    public string RoomId { get; }
    public string PeerPubkeyHex { get; }

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