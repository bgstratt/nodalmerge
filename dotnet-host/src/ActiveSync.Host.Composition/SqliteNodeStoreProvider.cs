using ActiveSync.Host.Abstractions.Providers;
using Microsoft.Data.Sqlite;
using System.Globalization;

namespace ActiveSync.Host.Composition;

internal sealed class SqliteNodeStoreProvider : INodeStoreProvider
{
    private readonly string _connectionString;

    public SqliteNodeStoreProvider(SqliteNodeStorageOptions options)
    {
        var fullPath = Path.GetFullPath(options.DbPath);
        var parent = Path.GetDirectoryName(fullPath);
        if (!string.IsNullOrWhiteSpace(parent))
        {
            Directory.CreateDirectory(parent);
        }

        _connectionString = new SqliteConnectionStringBuilder
        {
            DataSource = fullPath,
            Mode = SqliteOpenMode.ReadWriteCreate
        }.ToString();

        EnsureSchema();
    }

    public async ValueTask<NodeSnapshot?> LoadRoomSnapshotAsync(string roomId, CancellationToken cancellationToken = default)
    {
        await using var connection = new SqliteConnection(_connectionString);
        await connection.OpenAsync(cancellationToken);

        await using var command = connection.CreateCommand();
        command.CommandText = @"
            SELECT node_id_hex, payload
            FROM accepted_nodes
            WHERE room_id = $room
            ORDER BY node_id_hex;";
        command.Parameters.AddWithValue("$room", roomId);

        var nodes = new List<AcceptedNodeRecord>();
        await using var reader = await command.ExecuteReaderAsync(cancellationToken);
        while (await reader.ReadAsync(cancellationToken))
        {
            var nodeIdHex = reader.GetString(0);
            var payload = (byte[])reader[1];
            nodes.Add(new AcceptedNodeRecord(nodeIdHex, payload));
        }

        if (nodes.Count == 0)
        {
            return null;
        }

        return new NodeSnapshot(roomId, nodes);
    }

    public async ValueTask PersistAcceptedNodesAsync(
        string roomId,
        IReadOnlyList<AcceptedNodeRecord> nodes,
        CancellationToken cancellationToken = default
    )
    {
        if (nodes.Count == 0)
        {
            return;
        }

        await using var connection = new SqliteConnection(_connectionString);
        await connection.OpenAsync(cancellationToken);
        await using var transaction = (SqliteTransaction)await connection.BeginTransactionAsync(cancellationToken);

        foreach (var node in nodes)
        {
            await using var command = connection.CreateCommand();
            command.Transaction = transaction;
            command.CommandText = @"
                INSERT INTO accepted_nodes (room_id, node_id_hex, payload, updated_at_utc)
                VALUES ($room, $node, $payload, $updated)
                ON CONFLICT(room_id, node_id_hex) DO UPDATE SET
                    payload = excluded.payload,
                    updated_at_utc = excluded.updated_at_utc;";
            command.Parameters.AddWithValue("$room", roomId);
            command.Parameters.AddWithValue("$node", node.NodeIdHex);
            command.Parameters.AddWithValue("$payload", node.Payload);
            command.Parameters.AddWithValue("$updated", DateTimeOffset.UtcNow.ToString("O", CultureInfo.InvariantCulture));
            await command.ExecuteNonQueryAsync(cancellationToken);
        }

        await transaction.CommitAsync(cancellationToken);
    }

    public async ValueTask PersistCompactionSnapshotAsync(
        string roomId,
        CompactionSnapshot snapshot,
        CancellationToken cancellationToken = default
    )
    {
        await using var connection = new SqliteConnection(_connectionString);
        await connection.OpenAsync(cancellationToken);

        await using var command = connection.CreateCommand();
        command.CommandText = @"
            INSERT INTO compaction_snapshots (room_id, snapshot_payload, created_at_utc)
            VALUES ($room, $payload, $created)
            ON CONFLICT(room_id) DO UPDATE SET
                snapshot_payload = excluded.snapshot_payload,
                created_at_utc = excluded.created_at_utc;";
        command.Parameters.AddWithValue("$room", roomId);
        command.Parameters.AddWithValue("$payload", snapshot.SnapshotPayload);
        command.Parameters.AddWithValue("$created", snapshot.CreatedAtUtc.ToString("O", CultureInfo.InvariantCulture));
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    private void EnsureSchema()
    {
        using var connection = new SqliteConnection(_connectionString);
        connection.Open();

        using var command = connection.CreateCommand();
        command.CommandText = @"
            CREATE TABLE IF NOT EXISTS accepted_nodes (
                room_id TEXT NOT NULL,
                node_id_hex TEXT NOT NULL,
                payload BLOB NOT NULL,
                updated_at_utc TEXT NOT NULL,
                PRIMARY KEY (room_id, node_id_hex)
            );

            CREATE TABLE IF NOT EXISTS compaction_snapshots (
                room_id TEXT PRIMARY KEY,
                snapshot_payload BLOB NOT NULL,
                created_at_utc TEXT NOT NULL
            );";
        command.ExecuteNonQuery();
    }
}
