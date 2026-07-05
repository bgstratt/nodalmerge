using NodalMerge.Host.Abstractions.Providers;
using MongoDB.Bson;
using MongoDB.Driver;

namespace NodalMerge.Host.Composition;

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

        EnsureIndexes();
    }

    public async ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
    {
        var filter = Builders<BsonDocument>.Filter.Eq("room_id", roomId);
        var docs = await _acceptedNodes
            .Find(filter)
            .Sort(
                Builders<BsonDocument>.Sort
                    .Ascending("accepted_at_utc")
                    .Ascending("node_id_hex")
            )
            .ToListAsync(cancellationToken);

        if (docs.Count == 0)
        {
            return null;
        }

        var nodes = new List<AcceptedNodeRecord>(docs.Count);
        foreach (var doc in docs)
        {
            // Be permissive when reading older/malformed records: skip invalid entries instead of crashing room hydration.
            var nodeIdHex = GetStringOrNull(doc, "node_id_hex");
            var payload = GetByteArrayOrNull(doc, "payload");
            if (string.IsNullOrWhiteSpace(nodeIdHex) || payload is null)
            {
                continue;
            }

            var acceptedAt = GetDateTimeOffsetOrNull(doc, "accepted_at_utc");
            nodes.Add(new AcceptedNodeRecord(
                nodeIdHex,
                payload,
                GetStringOrNull(doc, "payload_kind") ?? AcceptedNodeKinds.Pack,
                GetStringArrayOrNull(doc, "causal_parent_node_ids"),
                GetStringOrNull(doc, "frontier_hash_hex"),
                doc.TryGetValue("applied", out var applied) && applied.ToBoolean(),
                doc.TryGetValue("is_tombstone", out var isTombstone) && isTombstone.ToBoolean(),
                acceptedAt,
                GetDateTimeOffsetOrNull(doc, "eligible_for_compaction_at_utc")
            ));
        }

        if (nodes.Count == 0)
        {
            return null;
        }

        return new NodeSnapshot(roomId, nodes.ToArray());
    }

    public async ValueTask<CompactionSnapshot?> LoadCompactionSnapshotAsync(
        string roomId,
        CancellationToken cancellationToken = default)
    {
        var filter = Builders<BsonDocument>.Filter.Eq("room_id", roomId);
        var doc = await _compactionSnapshots
            .Find(filter)
            .FirstOrDefaultAsync(cancellationToken);

        if (doc is null)
        {
            return null;
        }

        var createdAtUtc = GetDateTimeOffsetOrNull(doc, "created_at_utc") ?? DateTimeOffset.UtcNow;
        return new CompactionSnapshot(
            roomId,
            doc["snapshot_payload"].AsByteArray,
            createdAtUtc,
            doc.TryGetValue("boundary_node_id_hex", out var boundaryNodeId) && !boundaryNodeId.IsBsonNull
                ? boundaryNodeId.AsString
                : null,
            doc.TryGetValue("frontier_hash_hex", out var frontierHash) && !frontierHash.IsBsonNull
                ? frontierHash.AsString
                : null,
            doc.TryGetValue("schema_version", out var schemaVersion) && schemaVersion.IsInt32
                ? schemaVersion.AsInt32
                : 1
        );
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

            // Canonical cross-runtime write (docs/PERSISTENCE_SCHEMA.md,
            // shared with the Rust MongoNodeStore): $set the record fields,
            // $setOnInsert a deterministic compound _id and the original
            // accepted_at_utc (the hydration sort key must not move on
            // idempotent re-writes). Legacy documents keep their ObjectId
            // _id — identity is the unique (room_id, node_id_hex) index
            // either way.
            var update = Builders<BsonDocument>.Update
                .Set("room_id", roomId)
                .Set("node_id_hex", node.NodeIdHex)
                .Set("payload", node.Payload)
                .Set("payload_kind", string.IsNullOrWhiteSpace(node.PayloadKind) ? AcceptedNodeKinds.Pack : node.PayloadKind)
                .Set("causal_parent_node_ids", node.CausalParentNodeIds is { Count: > 0 }
                    ? new BsonArray(node.CausalParentNodeIds)
                    : new BsonArray())
                .Set("frontier_hash_hex", string.IsNullOrWhiteSpace(node.FrontierHashHex)
                    ? BsonNull.Value
                    : BsonValue.Create(node.FrontierHashHex))
                .Set("applied", node.Applied)
                .Set("is_tombstone", node.IsTombstone)
                .Set("eligible_for_compaction_at_utc", node.EligibleForCompactionAtUtc is null
                    ? BsonNull.Value
                    : BsonValue.Create(node.EligibleForCompactionAtUtc.Value.UtcDateTime))
                .Set("updated_at_utc", DateTime.UtcNow)
                .SetOnInsert("_id", $"{roomId}:{node.NodeIdHex}")
                .SetOnInsert("accepted_at_utc", (node.AcceptedAtUtc ?? DateTimeOffset.UtcNow).UtcDateTime);

            await _acceptedNodes.UpdateOneAsync(
                filter,
                update,
                new UpdateOptions { IsUpsert = true },
                cancellationToken
            );
        }
    }

    public async ValueTask DeleteAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<string> nodeIdHexes,
        CancellationToken cancellationToken = default
    )
    {
        if (nodeIdHexes.Count == 0)
        {
            return;
        }

        var filter = Builders<BsonDocument>.Filter.And(
            Builders<BsonDocument>.Filter.Eq("room_id", roomId),
            Builders<BsonDocument>.Filter.In("node_id_hex", nodeIdHexes)
        );

        await _acceptedNodes.DeleteManyAsync(filter, cancellationToken);
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
            ["created_at_utc"] = snapshot.CreatedAtUtc.UtcDateTime,
            ["boundary_node_id_hex"] = string.IsNullOrWhiteSpace(snapshot.BoundaryNodeIdHex)
                ? BsonNull.Value
                : snapshot.BoundaryNodeIdHex,
            ["frontier_hash_hex"] = string.IsNullOrWhiteSpace(snapshot.FrontierHashHex)
                ? BsonNull.Value
                : snapshot.FrontierHashHex,
            ["schema_version"] = snapshot.SchemaVersion
        };

        await _compactionSnapshots.ReplaceOneAsync(
            filter,
            doc,
            new ReplaceOptions { IsUpsert = true },
            cancellationToken
        );
    }

    private void EnsureIndexes()
    {
        var uniqueNodeIndex = new CreateIndexModel<BsonDocument>(
            Builders<BsonDocument>.IndexKeys.Ascending("room_id").Ascending("node_id_hex"),
            new CreateIndexOptions { Unique = true, Name = "ux_room_node" }
        );

        var compactionEligibilityIndex = new CreateIndexModel<BsonDocument>(
            Builders<BsonDocument>.IndexKeys.Ascending("room_id").Ascending("eligible_for_compaction_at_utc"),
            new CreateIndexOptions { Name = "ix_room_compaction_eligibility" }
        );

        _acceptedNodes.Indexes.CreateMany([uniqueNodeIndex, compactionEligibilityIndex]);
    }

    private static DateTimeOffset? GetDateTimeOffsetOrNull(BsonDocument doc, string fieldName)
    {
        if (!doc.TryGetValue(fieldName, out var value) || value.IsBsonNull)
        {
            return null;
        }

        if (value.IsValidDateTime)
        {
            return DateTime.SpecifyKind(value.ToUniversalTime(), DateTimeKind.Utc);
        }

        if (DateTimeOffset.TryParse(value.ToString(), out var parsed))
        {
            return parsed.ToUniversalTime();
        }

        return null;
    }

    private static string? GetStringOrNull(BsonDocument doc, string fieldName)
    {
        if (!doc.TryGetValue(fieldName, out var value) || value.IsBsonNull)
        {
            return null;
        }

        return value.BsonType == BsonType.String
            ? value.AsString
            : value.ToString();
    }

    private static byte[]? GetByteArrayOrNull(BsonDocument doc, string fieldName)
    {
        if (!doc.TryGetValue(fieldName, out var value) || value.IsBsonNull)
        {
            return null;
        }

        if (value.BsonType == BsonType.Binary)
        {
            return value.AsByteArray;
        }

        return null;
    }

    private static string[]? GetStringArrayOrNull(BsonDocument doc, string fieldName)
    {
        if (!doc.TryGetValue(fieldName, out var value) || value.IsBsonNull)
        {
            return null;
        }

        if (value is not BsonArray arr)
        {
            return null;
        }

        var values = arr
            .Where(x => !x.IsBsonNull)
            .Select(x => x.BsonType == BsonType.String ? x.AsString : x.ToString())
            .Where(x => !string.IsNullOrWhiteSpace(x))
            .Select(x => x!)
            .ToArray();

        return values.Length == 0 ? null : values;
    }
}
