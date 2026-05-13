namespace ActiveSync.Host.Abstractions.Providers;

public interface INodeStoreProvider
{
    ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default);

    ValueTask<CompactionSnapshot?> LoadCompactionSnapshotAsync(
        string roomId,
        CancellationToken cancellationToken = default
    );

    ValueTask PersistAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<AcceptedNodeRecord> nodes,
        CancellationToken cancellationToken = default
    );

    ValueTask DeleteAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<string> nodeIdHexes,
        CancellationToken cancellationToken = default
    );

    ValueTask PersistCompactionSnapshotAsync(
        string roomId,
        CompactionSnapshot snapshot,
        CancellationToken cancellationToken = default
    );
}

public static class AcceptedNodeKinds
{
    public const string Pack = "pack";
}

public sealed record AcceptedNodeRecord(
    string NodeIdHex,
    byte[] Payload,
    string PayloadKind = AcceptedNodeKinds.Pack,
    IReadOnlyList<string>? CausalParentNodeIds = null,
    string? FrontierHashHex = null,
    bool Applied = false,
    bool IsTombstone = false,
    DateTimeOffset? AcceptedAtUtc = null,
    DateTimeOffset? EligibleForCompactionAtUtc = null
);

public sealed record NodeSnapshot(string RoomId, IReadOnlyList<AcceptedNodeRecord> Nodes);

public sealed record CompactionSnapshot(
    string RoomId,
    byte[] SnapshotPayload,
    DateTimeOffset CreatedAtUtc,
    string? BoundaryNodeIdHex = null,
    string? FrontierHashHex = null,
    int SchemaVersion = 1
);
