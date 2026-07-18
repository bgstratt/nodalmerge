using NodalMerge.DotNetHost.Ffi;
using NodalMerge.Host.Abstractions.Providers;
using System.Collections.Concurrent;
using System.Diagnostics.Metrics;
using System.Security.Cryptography;
using System.Text.Json;

namespace NodalMerge.DotNetHost.Runtime;

public sealed class RuntimeDagPersistenceService
{
    private static readonly TimeSpan DefaultCompactionRetentionWindow = TimeSpan.FromDays(7);
    private static readonly Meter RuntimeDagMeter = new("NodalMerge.DotNetHost.RuntimeDag", "1.0.0");
    private static readonly Counter<long> CompactionActionsCounter = RuntimeDagMeter.CreateCounter<long>(
        "room_compaction_actions_total"
    );
    private static readonly Counter<long> CompactionRecordsRemovedCounter = RuntimeDagMeter.CreateCounter<long>(
        "room_compaction_records_removed_total"
    );
    private static readonly Histogram<double> CompactionDurationMsHistogram = RuntimeDagMeter.CreateHistogram<double>(
        "room_compaction_duration_ms"
    );
    private static readonly Counter<long> RecoverySemanticMutationsCounter = RuntimeDagMeter.CreateCounter<long>(
        "room_recovery_semantic_mutations_total"
    );
    private static readonly Counter<long> RecoveryIdempotentReplayCounter = RuntimeDagMeter.CreateCounter<long>(
        "room_recovery_idempotent_replay_total"
    );
    private static readonly Counter<long> RecoverySignalClassCounter = RuntimeDagMeter.CreateCounter<long>(
        "room_recovery_signal_class_total"
    );
    private static readonly Counter<long> DuplicatePackReplaySuppressedCounter = RuntimeDagMeter.CreateCounter<long>(
        "room_duplicate_pack_replay_suppressed_total"
    );

    private readonly INodeStoreProvider _nodeStore;
    private readonly IRuntimeCommandBridge _bridge;
    private readonly ILogger<RuntimeDagPersistenceService> _logger;
    private readonly RuntimeDagCompactionOptions _compactionOptions;
    private readonly RuntimeSnapshotDebounceOptions _snapshotOptions;
    private readonly TimeProvider _timeProvider;
    private readonly ConcurrentDictionary<string, Task> _hydrateByRoom =
        new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, Task> _compactionByRoom =
        new(StringComparer.Ordinal);
    private readonly ConcurrentDictionary<string, RoomSnapshotDebounceState> _snapshotDebounceByRoom =
        new(StringComparer.Ordinal);

    public RuntimeDagPersistenceService(
        INodeStoreProvider nodeStore,
        IRuntimeCommandBridge bridge,
        ILogger<RuntimeDagPersistenceService> logger
    ) : this(
        nodeStore,
        bridge,
        logger,
        RuntimeDagCompactionOptions.Default
    )
    {
    }

    public RuntimeDagPersistenceService(
        INodeStoreProvider nodeStore,
        IRuntimeCommandBridge bridge,
        ILogger<RuntimeDagPersistenceService> logger,
        RuntimeDagCompactionOptions compactionOptions
    ) : this(
        nodeStore,
        bridge,
        logger,
        compactionOptions,
        RuntimeSnapshotDebounceOptions.Default,
        TimeProvider.System
    )
    {
    }

    public RuntimeDagPersistenceService(
        INodeStoreProvider nodeStore,
        IRuntimeCommandBridge bridge,
        ILogger<RuntimeDagPersistenceService> logger,
        RuntimeDagCompactionOptions compactionOptions,
        RuntimeSnapshotDebounceOptions snapshotOptions,
        TimeProvider timeProvider
    )
    {
        _nodeStore = nodeStore;
        _bridge = bridge;
        _logger = logger;
        _compactionOptions = compactionOptions;
        _snapshotOptions = snapshotOptions;
        _timeProvider = timeProvider;
    }

    public Task HydrateRoomIfNeededAsync(string roomId, CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(roomId))
        {
            return Task.CompletedTask;
        }

        var task = _hydrateByRoom.GetOrAdd(roomId, key => HydrateRoomCoreAsync(key, cancellationToken));
        return task;
    }

    public async Task<string?> TryExportRoomPackB64Async(string roomId, CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(roomId))
            return null;
        var snapshot = await TryGetServerPackSnapshotAsync(roomId, cancellationToken);
        return snapshot is null || snapshot.Payload.Length == 0
            ? null
            : Convert.ToBase64String(snapshot.Payload);
    }

    public void InvalidateHydration(string roomId)
    {
        if (string.IsNullOrWhiteSpace(roomId))
        {
            return;
        }

        _hydrateByRoom.TryRemove(roomId, out _);
    }

    public async ValueTask PersistInboundPackAsync(
        string roomId,
        string nodesB64,
        CancellationToken cancellationToken = default
    )
    {
        if (string.IsNullOrWhiteSpace(roomId) || string.IsNullOrWhiteSpace(nodesB64))
        {
            return;
        }

        try
        {
            var payload = Convert.FromBase64String(nodesB64);
            if (payload.Length == 0)
            {
                return;
            }
            await PersistPackPayloadAsync(roomId, payload, "inbound-pack", cancellationToken);
        }
        catch (FormatException)
        {
            _logger.LogWarning("runtime dag persist skipped room={Room} reason=invalid-base64", roomId);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime dag persist failed room={Room}", roomId);
        }
    }

    public async ValueTask PersistRoomSnapshotAsync(
        string roomId,
        CancellationToken cancellationToken = default
    )
    {
        if (string.IsNullOrWhiteSpace(roomId))
        {
            return;
        }

        try
        {
            var snapshot = await TryGetServerPackSnapshotAsync(roomId, cancellationToken);
            if (snapshot is null || snapshot.Payload.Length == 0)
            {
                _logger.LogInformation(
                    "runtime dag persist skipped room={Room} reason=empty-server-pack",
                    roomId
                );
                return;
            }

            await PersistPackPayloadAsync(roomId, snapshot.Payload, "snapshot-on-mutation", cancellationToken);
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime dag persist snapshot failed room={Room}", roomId);
        }
    }

    /// <summary>
    /// Records a mutation against <paramref name="roomId"/> and persists a full server-pack snapshot
    /// only when the debounce window trips (at-most-once per <see cref="RuntimeSnapshotDebounceOptions.MaxPendingMutations"/>
    /// mutations or per <see cref="RuntimeSnapshotDebounceOptions.MinInterval"/> of wall-clock).
    /// The full-room snapshot is a hydration <em>checkpoint</em>, not the durability log — every inbound
    /// pack is already persisted incrementally via <see cref="PersistInboundPackAsync"/>, so coalescing
    /// the checkpoint only lengthens the delta chain replayed on the next hydrate; it never loses data.
    /// This is what keeps a growing room from being re-serialized on every single mutation (the O(n^2)
    /// snapshot-on-mutation storm). When debounce is disabled the call snapshots every time (legacy behaviour).
    /// </summary>
    public async ValueTask PersistRoomSnapshotDebouncedAsync(
        string roomId,
        CancellationToken cancellationToken = default
    )
    {
        if (string.IsNullOrWhiteSpace(roomId))
        {
            return;
        }

        if (!_snapshotOptions.Enabled)
        {
            await PersistRoomSnapshotAsync(roomId, cancellationToken);
            return;
        }

        var state = _snapshotDebounceByRoom.GetOrAdd(
            roomId,
            _ => new RoomSnapshotDebounceState { LastSnapshotUtc = _timeProvider.GetUtcNow() }
        );

        bool due;
        lock (state)
        {
            state.PendingMutations += 1;
            var elapsed = _timeProvider.GetUtcNow() - state.LastSnapshotUtc;
            due = state.PendingMutations >= _snapshotOptions.MaxPendingMutations
                || elapsed >= _snapshotOptions.MinInterval;
            if (due)
            {
                state.PendingMutations = 0;
                state.LastSnapshotUtc = _timeProvider.GetUtcNow();
            }
        }

        if (due)
        {
            await PersistRoomSnapshotAsync(roomId, cancellationToken);
        }
    }

    /// <summary>
    /// Forces a checkpoint of any mutations accumulated since the last debounced snapshot and resets the
    /// window. Called on peer disconnect / host shutdown so a room that trailed off mid-window still leaves
    /// a fresh checkpoint behind for the next hydrate. No-op when nothing is pending. Duplicate server-pack
    /// payloads are suppressed downstream, so an over-eager flush is cheap.
    /// </summary>
    public async ValueTask FlushRoomSnapshotAsync(
        string roomId,
        CancellationToken cancellationToken = default
    )
    {
        if (string.IsNullOrWhiteSpace(roomId))
        {
            return;
        }

        if (_snapshotOptions.Enabled && _snapshotDebounceByRoom.TryGetValue(roomId, out var state))
        {
            bool hasPending;
            lock (state)
            {
                hasPending = state.PendingMutations > 0;
                state.PendingMutations = 0;
                state.LastSnapshotUtc = _timeProvider.GetUtcNow();
            }

            if (!hasPending)
            {
                return;
            }
        }

        await PersistRoomSnapshotAsync(roomId, cancellationToken);
    }

    private static string BuildInspectPackEnvelope(string nodesB64)
    {
        return JsonSerializer.Serialize(new
        {
            room_id = "",
            command = new
            {
                InspectPack = new
                {
                    nodes_b64 = nodesB64
                }
            }
        });
    }

    private (IReadOnlyList<string>? causalParentNodeIds, string? frontierHashHex) TryInspectPack(byte[] payload)
    {
        try
        {
            var nodesB64 = Convert.ToBase64String(payload);
            var response = _bridge.ProcessJsonCommand(BuildInspectPackEnvelope(nodesB64));
            if (response.Status != AsStatus.Ok)
            {
                return (null, null);
            }
            using var eventsDoc = JsonDocument.Parse(response.EventsJson);
            if (eventsDoc.RootElement.ValueKind != JsonValueKind.Array)
            {
                return (null, null);
            }
            foreach (var eventElement in eventsDoc.RootElement.EnumerateArray())
            {
                if (eventElement.ValueKind != JsonValueKind.Object
                    || !eventElement.TryGetProperty("PackInspected", out var inspected)
                    || inspected.ValueKind != JsonValueKind.Object)
                {
                    continue;
                }

                IReadOnlyList<string>? causalParents = null;
                if (inspected.TryGetProperty("external_parent_ids_hex", out var extParents)
                    && extParents.ValueKind == JsonValueKind.Array)
                {
                    var ids = extParents.EnumerateArray()
                        .Where(e => e.ValueKind == JsonValueKind.String)
                        .Select(e => e.GetString()!)
                        .ToList();
                    if (ids.Count > 0)
                    {
                        causalParents = ids;
                    }
                }

                string? frontierHashHex = null;
                if (inspected.TryGetProperty("tip_node_ids_hex", out var tips)
                    && tips.ValueKind == JsonValueKind.Array)
                {
                    var tipList = tips.EnumerateArray()
                        .Where(e => e.ValueKind == JsonValueKind.String)
                        .Select(e => e.GetString()!)
                        .OrderBy(s => s, StringComparer.Ordinal)
                        .ToList();
                    if (tipList.Count > 0)
                    {
                        var joined = string.Join(string.Empty, tipList);
                        frontierHashHex = Convert.ToHexStringLower(SHA256.HashData(System.Text.Encoding.UTF8.GetBytes(joined)));
                    }
                }

                return (causalParents, frontierHashHex);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime dag inspect-pack failed");
        }
        return (null, null);
    }

    private async ValueTask PersistPackPayloadAsync(
        string roomId,
        byte[] payload,
        string source,
        CancellationToken cancellationToken
    )
    {
        var hashHex = Convert.ToHexStringLower(SHA256.HashData(payload));
        var acceptedAtUtc = DateTimeOffset.UtcNow;
        var retentionWindow = _compactionOptions.RetentionWindow ?? DefaultCompactionRetentionWindow;
        var (causalParentNodeIds, frontierHashHex) = TryInspectPack(payload);
        var record = new AcceptedNodeRecord(
            NodeIdHex: $"pack:{hashHex}",
            Payload: payload,
            PayloadKind: AcceptedNodeKinds.Pack,
            CausalParentNodeIds: causalParentNodeIds,
            FrontierHashHex: frontierHashHex,
            Applied: false,
            IsTombstone: false,
            AcceptedAtUtc: acceptedAtUtc,
            EligibleForCompactionAtUtc: acceptedAtUtc.Add(retentionWindow)
        );

        if (await IsKnownPackReplayAsync(roomId, record.NodeIdHex, cancellationToken))
        {
            DuplicatePackReplaySuppressedCounter.Add(
                1,
                KeyValuePair.Create<string, object?>("room", roomId)
            );
            _logger.LogInformation(
                "runtime dag persist skipped room={Room} reason=duplicate-pack-replay key={Key} source={Source}",
                roomId,
                record.NodeIdHex,
                source
            );
            return;
        }

        await _nodeStore.PersistAcceptedNodesAsync(roomId, [record], cancellationToken);
        _logger.LogInformation(
            "runtime dag persisted room={Room} bytes={Bytes} key={Key} source={Source}",
            roomId,
            payload.Length,
            record.NodeIdHex,
            source
        );

        await TryRunCompactionAsync(roomId, cancellationToken);
    }

    private async Task HydrateRoomCoreAsync(string roomId, CancellationToken cancellationToken)
    {
        var hydrateStartedAt = DateTimeOffset.UtcNow;
        var audit = new RecoveryAuditState(roomId, hydrateStartedAt);

        try
        {
            _logger.LogInformation(
                "runtime dag recovery marker=hydrate-start room={Room} at={StartedAtUtc}",
                roomId,
                hydrateStartedAt
            );

            var preReplay = await TryCaptureEntityStateAsync(roomId, cancellationToken);
            _logger.LogInformation(
                "runtime dag recovery marker=replay-start room={Room} pre_state_entries={PreStateEntries}",
                roomId,
                preReplay.Count
            );

            var ensureStatus = SubmitCommand(BuildEnsureRoomEnvelope(roomId));
            if (ensureStatus != AsStatus.Ok)
            {
                _logger.LogWarning(
                    "runtime dag hydrate ensure-room failed room={Room} status={Status}",
                    roomId,
                    ensureStatus
                );
                audit.Source = "ensure-room-failed";
                return;
            }

            var snapshotApplied = false;
            string? boundaryNodeIdHex = null;
            DateTimeOffset? boundaryAcceptedAtUtc = null;
            try
            {
                var compactionSnapshot = await _nodeStore.LoadCompactionSnapshotAsync(roomId, cancellationToken);
                if (compactionSnapshot is not null && compactionSnapshot.SnapshotPayload.Length > 0)
                {
                    var snapshotB64 = Convert.ToBase64String(compactionSnapshot.SnapshotPayload);
                    var snapshotStatus = SubmitCommand(BuildImportPackEnvelope(roomId, snapshotB64));
                    if (snapshotStatus == AsStatus.Ok)
                    {
                        snapshotApplied = true;
                        boundaryNodeIdHex = compactionSnapshot.BoundaryNodeIdHex;
                        boundaryAcceptedAtUtc = compactionSnapshot.CreatedAtUtc;
                        audit.SnapshotLoaded = true;
                    }
                    else
                    {
                        _logger.LogWarning(
                            "runtime dag hydrate snapshot import failed room={Room} status={Status} action=fallback-accepted-nodes",
                            roomId,
                            snapshotStatus
                        );
                    }
                }
            }
            catch (Exception ex)
            {
                _logger.LogWarning(
                    ex,
                    "runtime dag hydrate snapshot load failed room={Room} action=fallback-accepted-nodes",
                    roomId
                );
            }

            var snapshot = await _nodeStore.LoadRoomSnapshotAsync(roomId, cancellationToken);
            if (snapshot is null || snapshot.Nodes.Count == 0)
            {
                _logger.LogInformation(
                    "runtime dag hydrate room={Room} source={Source} records={Records} imported={Imported}",
                    roomId,
                    snapshotApplied ? "snapshot_only" : "empty",
                    0,
                    snapshotApplied ? 1 : 0
                );
                audit.Source = snapshotApplied ? "snapshot_only" : "empty";
                audit.RecordsLoaded = 0;
                audit.RecordsImported = snapshotApplied ? 1 : 0;
                return;
            }

            var imported = 0;
            var loaded = 0;
            var duplicateSkipped = 0;
            var seenNodeIds = new HashSet<string>(StringComparer.Ordinal);
            var packNodes = snapshot.Nodes
                .Where(node =>
                    string.Equals(node.PayloadKind, AcceptedNodeKinds.Pack, StringComparison.OrdinalIgnoreCase)
                    && node.NodeIdHex.StartsWith("pack:", StringComparison.Ordinal))
                .ToArray();

            var deltaNodes = packNodes.AsEnumerable();
            if (snapshotApplied)
            {
                if (!string.IsNullOrWhiteSpace(boundaryNodeIdHex))
                {
                    var boundaryIndex = Array.FindIndex(
                        packNodes,
                        node => string.Equals(node.NodeIdHex, boundaryNodeIdHex, StringComparison.Ordinal)
                    );

                    if (boundaryIndex >= 0)
                    {
                        deltaNodes = packNodes.Skip(boundaryIndex + 1);
                    }
                    else
                    {
                        _logger.LogWarning(
                            "runtime dag hydrate room={Room} missing-snapshot-boundary={Boundary} action=fallback-boundary-time",
                            roomId,
                            boundaryNodeIdHex
                        );
                        deltaNodes = packNodes.Where(node =>
                            !boundaryAcceptedAtUtc.HasValue
                            || !node.AcceptedAtUtc.HasValue
                            || node.AcceptedAtUtc.Value > boundaryAcceptedAtUtc.Value);
                    }
                }
                else
                {
                    deltaNodes = packNodes.Where(node =>
                        !boundaryAcceptedAtUtc.HasValue
                        || !node.AcceptedAtUtc.HasValue
                        || node.AcceptedAtUtc.Value > boundaryAcceptedAtUtc.Value);
                }
            }

            foreach (var node in deltaNodes)
            {
                if (!seenNodeIds.Add(node.NodeIdHex))
                {
                    duplicateSkipped += 1;
                    continue;
                }

                loaded += 1;

                var nodesB64 = Convert.ToBase64String(node.Payload);
                var status = SubmitCommand(BuildImportPackEnvelope(roomId, nodesB64));
                if (status == AsStatus.Ok)
                {
                    imported += 1;
                }
            }

            _logger.LogInformation(
                "runtime dag hydrate room={Room} source={Source} records={Records} imported={Imported} duplicate_skipped={DuplicateSkipped}",
                roomId,
                snapshotApplied ? "snapshot_plus_delta" : "accepted_nodes_only",
                loaded,
                imported,
                duplicateSkipped
            );

            audit.Source = snapshotApplied ? "snapshot_plus_delta" : "accepted_nodes_only";
            audit.RecordsLoaded = loaded;
            audit.RecordsImported = imported;
            audit.DuplicateSkipped = duplicateSkipped;

            var postReplay = await TryCaptureEntityStateAsync(roomId, cancellationToken);
            var diff = ComputeEntityDiff(preReplay, postReplay);
            audit.Created = diff.Created;
            audit.Updated = diff.Updated;
            audit.Deleted = diff.Deleted;
            audit.Noop = diff.Noop;
            audit.Boards = CountNamespace(postReplay, "boards");
            audit.Buttons = CountNamespace(postReplay, "buttons");
            audit.Positions = CountNamespace(postReplay, "positions");
            audit.Text = CountNamespace(postReplay, "text");
            audit.List = CountNamespace(postReplay, "list");

            _logger.LogInformation(
                "runtime dag recovery marker=replay-complete room={Room} created={Created} updated={Updated} deleted={Deleted} noop={Noop} boards={Boards} buttons={Buttons} positions={Positions} text={Text} list={List}",
                roomId,
                audit.Created,
                audit.Updated,
                audit.Deleted,
                audit.Noop,
                audit.Boards,
                audit.Buttons,
                audit.Positions,
                audit.Text,
                audit.List
            );
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime dag hydrate failed room={Room}", roomId);
            audit.Source = "hydrate-failed";
        }
        finally
        {
            var finishedAt = DateTimeOffset.UtcNow;
            var durationMs = (finishedAt - hydrateStartedAt).TotalMilliseconds;
            _logger.LogInformation(
                "runtime dag recovery marker=hydrate-complete room={Room} source={Source} snapshot_loaded={SnapshotLoaded} records_loaded={RecordsLoaded} records_imported={RecordsImported} duplicate_skipped={DuplicateSkipped} duration_ms={DurationMs}",
                roomId,
                audit.Source,
                audit.SnapshotLoaded,
                audit.RecordsLoaded,
                audit.RecordsImported,
                audit.DuplicateSkipped,
                Math.Round(durationMs, 3)
            );
            _logger.LogInformation(
                "runtime dag recovery marker=reconcile-counts room={Room} source={Source} records_loaded={RecordsLoaded} records_imported={RecordsImported} duplicate_skipped={DuplicateSkipped} created={Created} updated={Updated} deleted={Deleted} noop={Noop} boards={Boards} buttons={Buttons} positions={Positions} text={Text} list={List}",
                roomId,
                audit.Source,
                audit.RecordsLoaded,
                audit.RecordsImported,
                audit.DuplicateSkipped,
                audit.Created,
                audit.Updated,
                audit.Deleted,
                audit.Noop,
                audit.Boards,
                audit.Buttons,
                audit.Positions,
                audit.Text,
                audit.List
            );

            var semanticMutations = audit.Created + audit.Updated + audit.Deleted;
            var idempotentReplay = audit.Noop + audit.DuplicateSkipped;
            var signalClass = semanticMutations > 0 ? "semantic-change" : "idempotent-replay-only";
            RecoverySemanticMutationsCounter.Add(
                semanticMutations,
                KeyValuePair.Create<string, object?>("room", roomId)
            );
            RecoveryIdempotentReplayCounter.Add(
                idempotentReplay,
                KeyValuePair.Create<string, object?>("room", roomId)
            );
            RecoverySignalClassCounter.Add(
                1,
                KeyValuePair.Create<string, object?>("room", roomId),
                KeyValuePair.Create<string, object?>("signal_class", signalClass)
            );
            _logger.LogInformation(
                "runtime dag recovery marker=churn-summary room={Room} source={Source} semantic_mutations={SemanticMutations} idempotent_replay={IdempotentReplay} signal_class={SignalClass}",
                roomId,
                audit.Source,
                semanticMutations,
                idempotentReplay,
                signalClass
            );
        }
    }

    private async ValueTask<bool> IsKnownPackReplayAsync(
        string roomId,
        string nodeIdHex,
        CancellationToken cancellationToken)
    {
        var snapshot = await _nodeStore.LoadRoomSnapshotAsync(roomId, cancellationToken);
        if (snapshot is null || snapshot.Nodes.Count == 0)
        {
            return false;
        }

        return snapshot.Nodes.Any(node => string.Equals(node.NodeIdHex, nodeIdHex, StringComparison.Ordinal));
    }

    private AsStatus SubmitCommand(string json)
    {
        var result = _bridge.ProcessJsonCommand(json);
        return result.Status;
    }

    private Task TryRunCompactionAsync(string roomId, CancellationToken cancellationToken)
    {
        if (!_compactionOptions.Enabled)
        {
            return Task.CompletedTask;
        }

        var task = _compactionByRoom.GetOrAdd(
            roomId,
            key => RunCompactionCoreAsync(key, cancellationToken)
        );

        return task.ContinueWith(
            _ =>
            {
                _compactionByRoom.TryRemove(roomId, out Task? _);
            },
            CancellationToken.None,
            TaskContinuationOptions.ExecuteSynchronously,
            TaskScheduler.Default
        );
    }

    private async Task RunCompactionCoreAsync(string roomId, CancellationToken cancellationToken)
    {
        var startedAt = DateTimeOffset.UtcNow;
        var removed = 0;

        try
        {
            var snapshot = await _nodeStore.LoadRoomSnapshotAsync(roomId, cancellationToken);
            if (snapshot is null || snapshot.Nodes.Count == 0)
            {
                return;
            }

            var now = DateTimeOffset.UtcNow;
            var eligible = snapshot.Nodes
                .Where(node =>
                    string.Equals(node.PayloadKind, AcceptedNodeKinds.Pack, StringComparison.OrdinalIgnoreCase)
                    && node.EligibleForCompactionAtUtc.HasValue
                    && node.EligibleForCompactionAtUtc.Value <= now
                    && !node.IsTombstone)
                .OrderBy(node => node.AcceptedAtUtc ?? DateTimeOffset.MinValue)
                .ThenBy(node => node.NodeIdHex, StringComparer.Ordinal)
                .ToList();

            if (eligible.Count < _compactionOptions.MinEligibleNodes)
            {
                return;
            }

            var boundary = eligible[^1];
            var snapshotPayload = boundary.Payload;
            var frontierHashHex = Convert.ToHexStringLower(SHA256.HashData(boundary.Payload));
            var serverPack = await TryGetServerPackSnapshotAsync(roomId, cancellationToken);
            if (serverPack is not null)
            {
                snapshotPayload = serverPack.Payload;
                frontierHashHex = serverPack.RootHex ?? frontierHashHex;
            }
            var compactionSnapshot = new CompactionSnapshot(
                roomId,
                snapshotPayload,
                now,
                boundary.NodeIdHex,
                frontierHashHex,
                SchemaVersion: 2
            );

            await _nodeStore.PersistCompactionSnapshotAsync(roomId, compactionSnapshot, cancellationToken);
            CompactionActionsCounter.Add(1, KeyValuePair.Create<string, object?>("room", roomId));

            if (_compactionOptions.EnablePruning)
            {
                var removableNodeIds = eligible
                    .Take(Math.Max(0, eligible.Count - 1))
                    .Select(node => node.NodeIdHex)
                    .ToArray();

                if (removableNodeIds.Length > 0)
                {
                    await _nodeStore.DeleteAcceptedNodesAsync(roomId, removableNodeIds, cancellationToken);
                    removed = removableNodeIds.Length;
                    CompactionRecordsRemovedCounter.Add(
                        removed,
                        KeyValuePair.Create<string, object?>("room", roomId)
                    );
                }
            }

            _logger.LogInformation(
                "runtime dag compaction room={Room} eligible={Eligible} boundary={Boundary} pruned={Pruned} pruning={PruningMode}",
                roomId,
                eligible.Count,
                boundary.NodeIdHex,
                removed,
                _compactionOptions.EnablePruning
            );
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime dag compaction failed room={Room}", roomId);
        }
        finally
        {
            var durationMs = (DateTimeOffset.UtcNow - startedAt).TotalMilliseconds;
            CompactionDurationMsHistogram.Record(
                durationMs,
                KeyValuePair.Create<string, object?>("room", roomId)
            );
        }
    }

    private static string BuildEnsureRoomEnvelope(string roomId)
    {
        return JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = "EnsureRoom"
        });
    }

    private static string BuildImportPackEnvelope(string roomId, string nodesB64)
    {
        return JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = new
            {
                ImportPack = new
                {
                    nodes_b64 = nodesB64
                }
            }
        });
    }

    private async Task<ServerPackSnapshotPayload?> TryGetServerPackSnapshotAsync(
        string roomId,
        CancellationToken cancellationToken)
    {
        cancellationToken.ThrowIfCancellationRequested();

        var ensure = SubmitCommand(BuildEnsureRoomEnvelope(roomId));
        if (ensure != AsStatus.Ok)
        {
            _logger.LogWarning(
                "runtime dag compaction request-pack ensure-room failed room={Room} status={Status}",
                roomId,
                ensure
            );
            return null;
        }

        var response = _bridge.ProcessJsonCommand(BuildRequestServerPackEnvelope(roomId));
        if (response.Status != AsStatus.Ok)
        {
            _logger.LogWarning(
                "runtime dag compaction request-pack failed room={Room} status={Status}",
                roomId,
                response.Status
            );
            return null;
        }

        try
        {
            using var eventsDoc = JsonDocument.Parse(response.EventsJson);
            if (eventsDoc.RootElement.ValueKind != JsonValueKind.Array)
            {
                return null;
            }

            foreach (var eventElement in eventsDoc.RootElement.EnumerateArray())
            {
                if (eventElement.ValueKind != JsonValueKind.Object
                    || !eventElement.TryGetProperty("ServerPackPrepared", out var serverPack)
                    || serverPack.ValueKind != JsonValueKind.Object)
                {
                    continue;
                }

                if (!serverPack.TryGetProperty("nodes_b64", out var nodesB64Node)
                    || nodesB64Node.ValueKind != JsonValueKind.String)
                {
                    continue;
                }

                var nodesB64 = nodesB64Node.GetString();
                if (string.IsNullOrWhiteSpace(nodesB64))
                {
                    continue;
                }

                var payload = Convert.FromBase64String(nodesB64);
                if (payload.Length == 0)
                {
                    continue;
                }

                string? rootHex = serverPack.TryGetProperty("root_hex", out var rootHexNode)
                    && rootHexNode.ValueKind == JsonValueKind.String
                    ? rootHexNode.GetString()
                    : null;

                return new ServerPackSnapshotPayload(payload, rootHex);
            }
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime dag compaction request-pack parse failed room={Room}", roomId);
        }

        return null;
    }

    private async Task<Dictionary<string, string>> TryCaptureEntityStateAsync(
        string roomId,
        CancellationToken cancellationToken)
    {
        var snapshot = await TryGetServerPackSnapshotAsync(roomId, cancellationToken);
        if (snapshot is null)
        {
            return new Dictionary<string, string>(StringComparer.Ordinal);
        }

        return TryExtractEntityState(snapshot.Payload);
    }

    private static Dictionary<string, string> TryExtractEntityState(byte[] payload)
    {
        try
        {
            var state = JsonSerializer.Deserialize<Dictionary<string, JsonElement>>(payload)
                ?? new Dictionary<string, JsonElement>(StringComparer.Ordinal);

            return state.ToDictionary(
                kvp => kvp.Key,
                kvp => kvp.Value.GetRawText(),
                StringComparer.Ordinal
            );
        }
        catch
        {
            return new Dictionary<string, string>(StringComparer.Ordinal);
        }
    }

    private static ReconcileDiff ComputeEntityDiff(
        IReadOnlyDictionary<string, string> before,
        IReadOnlyDictionary<string, string> after)
    {
        var created = 0;
        var updated = 0;
        var deleted = 0;
        var noop = 0;

        foreach (var pair in after)
        {
            if (!before.TryGetValue(pair.Key, out var beforeValue))
            {
                created += 1;
                continue;
            }

            if (string.Equals(beforeValue, pair.Value, StringComparison.Ordinal))
            {
                noop += 1;
            }
            else
            {
                updated += 1;
            }
        }

        foreach (var pair in before)
        {
            if (!after.ContainsKey(pair.Key))
            {
                deleted += 1;
            }
        }

        return new ReconcileDiff(created, updated, deleted, noop);
    }

    private static int CountNamespace(IReadOnlyDictionary<string, string> state, string prefix)
    {
        var match = prefix + "/";
        return state.Keys.Count(key => key.StartsWith(match, StringComparison.Ordinal));
    }

    private static string BuildRequestServerPackEnvelope(string roomId)
    {
        return JsonSerializer.Serialize(new
        {
            room_id = roomId,
            command = new
            {
                RequestServerPack = new
                {
                    known_ids = Array.Empty<string>()
                }
            }
        });
    }
}

public sealed record RuntimeDagCompactionOptions(
    bool Enabled,
    int MinEligibleNodes,
    bool EnablePruning,
    TimeSpan? RetentionWindow = null
)
{
    public static RuntimeDagCompactionOptions Default { get; } = new(
        Enabled: true,
        MinEligibleNodes: 32,
        EnablePruning: false,
        RetentionWindow: null
    );
}

/// <summary>
/// Debounce policy for the per-room full server-pack checkpoint (<see cref="RuntimeDagPersistenceService.PersistRoomSnapshotDebouncedAsync"/>).
/// The checkpoint re-serializes the entire room, so taking it on every mutation is O(n^2) as the room grows.
/// Because every inbound pack is already persisted incrementally, the checkpoint is only a hydrate accelerator
/// and is safe to coalesce. Snapshot is taken when EITHER threshold trips, then the window resets.
/// </summary>
public sealed record RuntimeSnapshotDebounceOptions(
    bool Enabled,
    int MaxPendingMutations,
    TimeSpan MinInterval
)
{
    public static RuntimeSnapshotDebounceOptions Default { get; } = new(
        Enabled: true,
        MaxPendingMutations: 200,
        MinInterval: TimeSpan.FromSeconds(30)
    );
}

internal sealed class RoomSnapshotDebounceState
{
    public int PendingMutations;
    public DateTimeOffset LastSnapshotUtc;
}

internal sealed record ServerPackSnapshotPayload(byte[] Payload, string? RootHex);

internal sealed record ReconcileDiff(int Created, int Updated, int Deleted, int Noop);

internal sealed class RecoveryAuditState
{
    public RecoveryAuditState(string roomId, DateTimeOffset startedAtUtc)
    {
        RoomId = roomId;
        StartedAtUtc = startedAtUtc;
    }

    public string RoomId { get; }

    public DateTimeOffset StartedAtUtc { get; }

    public string Source { get; set; } = "unknown";

    public bool SnapshotLoaded { get; set; }

    public int RecordsLoaded { get; set; }

    public int RecordsImported { get; set; }

    public int DuplicateSkipped { get; set; }

    public int Created { get; set; }

    public int Updated { get; set; }

    public int Deleted { get; set; }

    public int Noop { get; set; }

    public int Boards { get; set; }

    public int Buttons { get; set; }

    public int Positions { get; set; }

    public int Text { get; set; }

    public int List { get; set; }
}