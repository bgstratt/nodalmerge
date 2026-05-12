namespace ActiveSync.Host.Abstractions.Providers;

public interface INodeStoreProvider
{
    ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default);

    ValueTask PersistAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<AcceptedNodeRecord> nodes,
        CancellationToken cancellationToken = default
    );

    ValueTask PersistCompactionSnapshotAsync(
        string roomId,
        CompactionSnapshot snapshot,
        CancellationToken cancellationToken = default
    );
}

public sealed record AcceptedNodeRecord(string NodeIdHex, byte[] Payload);

public sealed record NodeSnapshot(string RoomId, IReadOnlyList<AcceptedNodeRecord> Nodes);

public sealed record CompactionSnapshot(string RoomId, byte[] SnapshotPayload, DateTimeOffset CreatedAtUtc);
