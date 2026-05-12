using ActiveSync.Host.Abstractions.Providers;
using MongoDB.Bson;
using MongoDB.Driver;

namespace ActiveSync.Host.Composition;

internal sealed class MongoNodeStoreProvider : INodeStoreProvider
{
    private readonly IMongoCollection<BsonDocument> _acceptedNodes;
    private readonly IMongoCollection<BsonDocument> _compactionSnapshots;

    public MongoNodeStoreProvider(MongoNodeStorageOptions options)
    {
        var client = new MongoClient(options.ConnectionString);
        var database = client.GetDatabase(options.DatabaseName);

        _acceptedNodes = database.GetCollection<BsonDocument>("accepted_nodes");
        _compactionSnapshots = database.GetCollection<BsonDocument>("compaction_snapshots");
    }

    public async ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
    {
        var filter = Builders<BsonDocument>.Filter.Eq("room_id", roomId);
        var docs = await _acceptedNodes
            .Find(filter)
            .Sort(Builders<BsonDocument>.Sort.Ascending("node_id_hex"))
            .ToListAsync(cancellationToken);

        if (docs.Count == 0)
        {
            return null;
        }

        var nodes = docs.Select(doc => new AcceptedNodeRecord(
            doc["node_id_hex"].AsString,
            doc["payload"].AsByteArray
        )).ToArray();

        return new NodeSnapshot(roomId, nodes);
    }

    public async ValueTask PersistAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<AcceptedNodeRecord> nodes,
        CancellationToken cancellationToken = default
    )
    {
        foreach (var node in nodes)
        {
            var filter = Builders<BsonDocument>.Filter.And(
                Builders<BsonDocument>.Filter.Eq("room_id", roomId),
                Builders<BsonDocument>.Filter.Eq("node_id_hex", node.NodeIdHex)
            );

            var doc = new BsonDocument
            {
                ["room_id"] = roomId,
                ["node_id_hex"] = node.NodeIdHex,
                ["payload"] = node.Payload,
                ["updated_at_utc"] = DateTime.UtcNow
            };

            await _acceptedNodes.ReplaceOneAsync(
                filter,
                doc,
                new ReplaceOptions { IsUpsert = true },
                cancellationToken
            );
        }
    }

    public async ValueTask PersistCompactionSnapshotAsync(
        string roomId,
        CompactionSnapshot snapshot,
        CancellationToken cancellationToken = default
    )
    {
        var filter = Builders<BsonDocument>.Filter.Eq("room_id", roomId);
        var doc = new BsonDocument
        {
            ["room_id"] = roomId,
            ["snapshot_payload"] = snapshot.SnapshotPayload,
            ["created_at_utc"] = snapshot.CreatedAtUtc.UtcDateTime
        };

        await _compactionSnapshots.ReplaceOneAsync(
            filter,
            doc,
            new ReplaceOptions { IsUpsert = true },
            cancellationToken
        );
    }
}
