using ActiveSync.Host.Abstractions.Providers;
using Microsoft.Data.Sqlite;
using System.Globalization;
using System.Text.Json;

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
            SELECT node_id_hex, payload, payload_kind, causal_parent_node_ids_json, frontier_hash_hex,
                   applied, is_tombstone, accepted_at_utc, eligible_for_compaction_at_utc
            FROM accepted_nodes
            WHERE room_id = $room
            ORDER BY accepted_at_utc ASC, node_id_hex ASC;";
        command.Parameters.AddWithValue("$room", roomId);

        var nodes = new List<AcceptedNodeRecord>();
        await using var reader = await command.ExecuteReaderAsync(cancellationToken);
        while (await reader.ReadAsync(cancellationToken))
        {
            var nodeIdHex = reader.GetString(0);
            var payload = (byte[])reader[1];
            var payloadKind = reader.IsDBNull(2) ? AcceptedNodeKinds.Pack : reader.GetString(2);
            var causalJson = reader.IsDBNull(3) ? null : reader.GetString(3);
            var causalParents = string.IsNullOrWhiteSpace(causalJson)
                ? null
                : JsonSerializer.Deserialize<string[]>(causalJson);
            var frontierHashHex = reader.IsDBNull(4) ? null : reader.GetString(4);
            var applied = !reader.IsDBNull(5) && reader.GetInt64(5) == 1;
            var isTombstone = !reader.IsDBNull(6) && reader.GetInt64(6) == 1;
            var acceptedAtUtc = reader.IsDBNull(7)
                ? DateTimeOffset.UtcNow
                : DateTimeOffset.Parse(reader.GetString(7), CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind);
            DateTimeOffset? eligibleForCompactionAtUtc = reader.IsDBNull(8)
                ? null
                : DateTimeOffset.Parse(reader.GetString(8), CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind);

            nodes.Add(new AcceptedNodeRecord(
                nodeIdHex,
                payload,
                payloadKind,
                causalParents,
                frontierHashHex,
                applied,
                isTombstone,
                acceptedAtUtc,
                eligibleForCompactionAtUtc
            ));
        }

        if (nodes.Count == 0)
        {
            return null;
        }

        return new NodeSnapshot(roomId, nodes);
    }

    public async ValueTask<CompactionSnapshot?> LoadCompactionSnapshotAsync(
        string roomId,
        CancellationToken cancellationToken = default)
    {
        await using var connection = new SqliteConnection(_connectionString);
        await connection.OpenAsync(cancellationToken);

        await using var command = connection.CreateCommand();
        command.CommandText = @"
            SELECT snapshot_payload, created_at_utc, boundary_node_id_hex, frontier_hash_hex, schema_version
            FROM compaction_snapshots
            WHERE room_id = $room
            LIMIT 1;";
        command.Parameters.AddWithValue("$room", roomId);

        await using var reader = await command.ExecuteReaderAsync(cancellationToken);
        if (!await reader.ReadAsync(cancellationToken))
        {
            return null;
        }

        var snapshotPayload = (byte[])reader[0];
        var createdAtUtc = DateTimeOffset.Parse(reader.GetString(1), CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind);
        var boundaryNodeIdHex = reader.IsDBNull(2) ? null : reader.GetString(2);
        var frontierHashHex = reader.IsDBNull(3) ? null : reader.GetString(3);
        var schemaVersion = reader.IsDBNull(4) ? 1 : reader.GetInt32(4);

        return new CompactionSnapshot(
            roomId,
            snapshotPayload,
            createdAtUtc,
            boundaryNodeIdHex,
            frontierHashHex,
            schemaVersion
        );
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
                INSERT INTO accepted_nodes (
                    room_id,
                    node_id_hex,
                    payload,
                    payload_kind,
                    causal_parent_node_ids_json,
                    frontier_hash_hex,
                    applied,
                    is_tombstone,
                    accepted_at_utc,
                    eligible_for_compaction_at_utc,
                    updated_at_utc
                )
                VALUES (
                    $room,
                    $node,
                    $payload,
                    $payloadKind,
                    $causalParentsJson,
                    $frontierHash,
                    $applied,
                    $isTombstone,
                    $acceptedAt,
                    $eligibleForCompactionAt,
                    $updated
                )
                ON CONFLICT(room_id, node_id_hex) DO UPDATE SET
                    payload = excluded.payload,
                    payload_kind = excluded.payload_kind,
                    causal_parent_node_ids_json = excluded.causal_parent_node_ids_json,
                    frontier_hash_hex = excluded.frontier_hash_hex,
                    applied = excluded.applied,
                    is_tombstone = excluded.is_tombstone,
                    accepted_at_utc = excluded.accepted_at_utc,
                    eligible_for_compaction_at_utc = excluded.eligible_for_compaction_at_utc,
                    updated_at_utc = excluded.updated_at_utc;";
            command.Parameters.AddWithValue("$room", roomId);
            command.Parameters.AddWithValue("$node", node.NodeIdHex);
            command.Parameters.AddWithValue("$payload", node.Payload);
            command.Parameters.AddWithValue("$payloadKind", string.IsNullOrWhiteSpace(node.PayloadKind) ? AcceptedNodeKinds.Pack : node.PayloadKind);
            command.Parameters.AddWithValue("$causalParentsJson", node.CausalParentNodeIds is { Count: > 0 } ? JsonSerializer.Serialize(node.CausalParentNodeIds) : (object)DBNull.Value);
            command.Parameters.AddWithValue("$frontierHash", node.FrontierHashHex ?? (object)DBNull.Value);
            command.Parameters.AddWithValue("$applied", node.Applied ? 1 : 0);
            command.Parameters.AddWithValue("$isTombstone", node.IsTombstone ? 1 : 0);
            command.Parameters.AddWithValue("$acceptedAt", (node.AcceptedAtUtc ?? DateTimeOffset.UtcNow).ToString("O", CultureInfo.InvariantCulture));
            command.Parameters.AddWithValue("$eligibleForCompactionAt", node.EligibleForCompactionAtUtc?.ToString("O", CultureInfo.InvariantCulture) ?? (object)DBNull.Value);
            command.Parameters.AddWithValue("$updated", DateTimeOffset.UtcNow.ToString("O", CultureInfo.InvariantCulture));
            await command.ExecuteNonQueryAsync(cancellationToken);
        }

        await transaction.CommitAsync(cancellationToken);
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

        await using var connection = new SqliteConnection(_connectionString);
        await connection.OpenAsync(cancellationToken);
        await using var transaction = (SqliteTransaction)await connection.BeginTransactionAsync(cancellationToken);

        foreach (var nodeIdHex in nodeIdHexes)
        {
            await using var command = connection.CreateCommand();
            command.Transaction = transaction;
            command.CommandText = @"
                DELETE FROM accepted_nodes
                WHERE room_id = $room AND node_id_hex = $node;";
            command.Parameters.AddWithValue("$room", roomId);
            command.Parameters.AddWithValue("$node", nodeIdHex);
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
            INSERT INTO compaction_snapshots (
                room_id,
                snapshot_payload,
                created_at_utc,
                boundary_node_id_hex,
                frontier_hash_hex,
                schema_version
            )
            VALUES ($room, $payload, $created, $boundaryNodeId, $frontierHash, $schemaVersion)
            ON CONFLICT(room_id) DO UPDATE SET
                snapshot_payload = excluded.snapshot_payload,
                created_at_utc = excluded.created_at_utc,
                boundary_node_id_hex = excluded.boundary_node_id_hex,
                frontier_hash_hex = excluded.frontier_hash_hex,
                schema_version = excluded.schema_version;";
        command.Parameters.AddWithValue("$room", roomId);
        command.Parameters.AddWithValue("$payload", snapshot.SnapshotPayload);
        command.Parameters.AddWithValue("$created", snapshot.CreatedAtUtc.ToString("O", CultureInfo.InvariantCulture));
        command.Parameters.AddWithValue("$boundaryNodeId", snapshot.BoundaryNodeIdHex ?? (object)DBNull.Value);
        command.Parameters.AddWithValue("$frontierHash", snapshot.FrontierHashHex ?? (object)DBNull.Value);
        command.Parameters.AddWithValue("$schemaVersion", snapshot.SchemaVersion);
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
                payload_kind TEXT NOT NULL DEFAULT 'pack',
                causal_parent_node_ids_json TEXT NULL,
                frontier_hash_hex TEXT NULL,
                applied INTEGER NOT NULL DEFAULT 0,
                is_tombstone INTEGER NOT NULL DEFAULT 0,
                accepted_at_utc TEXT NOT NULL,
                eligible_for_compaction_at_utc TEXT NULL,
                updated_at_utc TEXT NOT NULL,
                PRIMARY KEY (room_id, node_id_hex)
            );

            CREATE TABLE IF NOT EXISTS compaction_snapshots (
                room_id TEXT PRIMARY KEY,
                snapshot_payload BLOB NOT NULL,
                created_at_utc TEXT NOT NULL,
                boundary_node_id_hex TEXT NULL,
                frontier_hash_hex TEXT NULL,
                schema_version INTEGER NOT NULL DEFAULT 1
            );";
        command.ExecuteNonQuery();

        EnsureAcceptedNodeColumns(connection);
        EnsureCompactionSnapshotColumns(connection);
    }

    private static void EnsureAcceptedNodeColumns(SqliteConnection connection)
    {
        var existingColumns = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        using (var pragma = connection.CreateCommand())
        {
            pragma.CommandText = "PRAGMA table_info(accepted_nodes);";
            using var reader = pragma.ExecuteReader();
            while (reader.Read())
            {
                existingColumns.Add(reader.GetString(1));
            }
        }

        EnsureColumn(connection, existingColumns, "payload_kind", "TEXT NOT NULL DEFAULT 'pack'");
        EnsureColumn(connection, existingColumns, "causal_parent_node_ids_json", "TEXT NULL");
        EnsureColumn(connection, existingColumns, "frontier_hash_hex", "TEXT NULL");
        EnsureColumn(connection, existingColumns, "applied", "INTEGER NOT NULL DEFAULT 0");
        EnsureColumn(connection, existingColumns, "is_tombstone", "INTEGER NOT NULL DEFAULT 0");
        EnsureColumn(connection, existingColumns, "accepted_at_utc", "TEXT NOT NULL DEFAULT ''");
        EnsureColumn(connection, existingColumns, "eligible_for_compaction_at_utc", "TEXT NULL");

        using var backfill = connection.CreateCommand();
        backfill.CommandText = @"
            UPDATE accepted_nodes
            SET accepted_at_utc = CASE
                WHEN accepted_at_utc IS NULL OR accepted_at_utc = '' THEN $now
                ELSE accepted_at_utc
            END;";
        backfill.Parameters.AddWithValue("$now", DateTimeOffset.UtcNow.ToString("O", CultureInfo.InvariantCulture));
        backfill.ExecuteNonQuery();
    }

    private static void EnsureColumn(
        SqliteConnection connection,
        ISet<string> existingColumns,
        string columnName,
        string definition)
    {
        if (existingColumns.Contains(columnName))
        {
            return;
        }

        using var add = connection.CreateCommand();
        add.CommandText = $"ALTER TABLE accepted_nodes ADD COLUMN {columnName} {definition};";
        add.ExecuteNonQuery();
        existingColumns.Add(columnName);
    }

    private static void EnsureCompactionSnapshotColumns(SqliteConnection connection)
    {
        var existingColumns = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        using (var pragma = connection.CreateCommand())
        {
            pragma.CommandText = "PRAGMA table_info(compaction_snapshots);";
            using var reader = pragma.ExecuteReader();
            while (reader.Read())
            {
                existingColumns.Add(reader.GetString(1));
            }
        }

        EnsureColumn(connection, existingColumns, "boundary_node_id_hex", "TEXT NULL");
        EnsureColumn(connection, existingColumns, "frontier_hash_hex", "TEXT NULL");
        EnsureColumn(connection, existingColumns, "schema_version", "INTEGER NOT NULL DEFAULT 1");
    }
}
