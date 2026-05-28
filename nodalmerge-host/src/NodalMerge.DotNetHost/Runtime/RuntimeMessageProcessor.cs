using NodalMerge.DotNetHost.Ffi;
using System.Diagnostics.Metrics;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Nodes;

namespace NodalMerge.DotNetHost.Runtime;

public sealed class RuntimeMessageProcessor
{
    private static readonly Meter RuntimeControlPlaneMeter = new("NodalMerge.DotNetHost.RuntimeControlPlane", "1.0.0");
    private static readonly Meter LegacyRuntimeControlPlaneMeter = new("NodalMerge.DotNetHost.RuntimeControlPlane", "1.0.0");
    private static readonly Counter<long> RuntimeControlPlaneDeniedCounter = RuntimeControlPlaneMeter.CreateCounter<long>(
        "runtime_control_plane_denied_total"
    );
    private static readonly Counter<long> LegacyRuntimeControlPlaneDeniedCounter = LegacyRuntimeControlPlaneMeter.CreateCounter<long>(
        "runtime_control_plane_denied_total"
    );

    private readonly IRuntimeCommandBridge _bridge;
    private readonly RuntimeProtocolMapper _mapper;

    public RuntimeMessageProcessor(IRuntimeCommandBridge bridge, RuntimeProtocolMapper mapper)
    {
        _bridge = bridge;
        _mapper = mapper;
    }

    public RuntimeMessageProcessResult ProcessIncomingText(
        string incomingJson,
        RuntimeConnectionState state
    )
    {
        if (TryReadHelperType(incomingJson, out var helperType))
        {
            if (string.Equals(helperType, "text-insert-at", StringComparison.OrdinalIgnoreCase))
            {
                return ProcessTextInsertAt(incomingJson, state);
            }

            if (string.Equals(helperType, "text-delete-at", StringComparison.OrdinalIgnoreCase))
            {
                return ProcessTextDeleteAt(incomingJson, state);
            }
        }

        var outbound = new List<string>();

        var mapResult = _mapper.MapIncomingMessageToCommandJsons(incomingJson, state);
        if (!mapResult.IsSuccess)
        {
            RecordControlPlaneDenyMetricIfApplicable(mapResult.Error);
            outbound.Add(
                RuntimeErrorEnvelopeBuilder.BuildMessageError(mapResult.Error ?? "runtime map failed")
            );
            return RuntimeMessageProcessResult.DispatchFailure(outbound, shouldCloseConnection: false);
        }

        var closeRequested = mapResult.ShouldCloseConnection;
        var dispatchSucceeded = true;

        foreach (var commandJson in mapResult.CommandJsons)
        {
            TrackCanonicalMapMutation(state, commandJson);
            var bridgeResult = TryExecuteQueryProjectionStubCommand(state, commandJson, out var stubResult)
                ? stubResult
                : _bridge.ProcessJsonCommand(commandJson);
            if (!bridgeResult.IsSuccess)
            {
                dispatchSucceeded = false;
                outbound.Add(
                    RuntimeErrorEnvelopeBuilder.BuildStatusError(
                        bridgeResult.Status.ToString(),
                        bridgeResult.DenyMetadata?.DenyMessage,
                        bridgeResult.DenyMetadata?.ReasonClass,
                        bridgeResult.DenyMetadata?.Command,
                        bridgeResult.DenyMetadata?.RequiredCapability
                    )
                );
                break;
            }

            var eventMapResult = _mapper.MapEventsJsonToOutboundMessages(bridgeResult.EventsJson);
            if (!eventMapResult.IsSuccess)
            {
                dispatchSucceeded = false;
                outbound.Add(
                    RuntimeErrorEnvelopeBuilder.BuildMessageError(
                        eventMapResult.Error ?? "runtime event map failed"
                    )
                );
                break;
            }

            outbound.AddRange(eventMapResult.OutboundMessages);
        }

        var shouldCloseAfterDispatch = RuntimeDispatchClosePolicy.ShouldCloseAfterDispatch(
            closeRequested,
            dispatchSucceeded
        );

        return dispatchSucceeded
            ? RuntimeMessageProcessResult.DispatchSuccess(outbound, shouldCloseAfterDispatch)
            : RuntimeMessageProcessResult.DispatchFailure(outbound, shouldCloseAfterDispatch);
    }

    private RuntimeMessageProcessResult ProcessTextInsertAt(string incomingJson, RuntimeConnectionState state)
    {
        var parse = ParseTextAtRequest(incomingJson, "text-insert-at");
        if (!parse.IsSuccess)
        {
            return RuntimeMessageProcessResult.DispatchFailure(
                [RuntimeErrorEnvelopeBuilder.BuildMessageError(parse.Error!)],
                shouldCloseConnection: false
            );
        }

        if (string.IsNullOrWhiteSpace(parse.Ch))
        {
            return RuntimeMessageProcessResult.DispatchFailure(
                [RuntimeErrorEnvelopeBuilder.BuildMessageError("text-insert-at.ch is required")],
                shouldCloseConnection: false
            );
        }

        var resolve = ResolveTextEntriesForPosition(state, parse.Namespace!, parse.Key!);
        if (!resolve.IsSuccess)
        {
            return resolve.ToDispatchFailure();
        }

        var index = parse.Index ?? -1;

        if (index > resolve.Entries.Count)
        {
            return RuntimeMessageProcessResult.DispatchFailure(
                [RuntimeErrorEnvelopeBuilder.BuildMessageError("text-insert-at.index is out of range")],
                shouldCloseConnection: false
            );
        }

        string? afterId = null;
        if (index > 0)
        {
            afterId = resolve.Entries[index - 1].Id;
        }

        var insertEnvelope = BuildEnvelope(
            state.RoomId!,
            new JsonObject
            {
                ["TextInsert"] = new JsonObject
                {
                    ["namespace"] = parse.Namespace,
                    ["key"] = parse.Key,
                    ["after_id"] = afterId,
                    ["ch"] = parse.Ch
                }
            }
        );

        return ExecuteSingleBridgeCommand(insertEnvelope);
    }

    private RuntimeMessageProcessResult ProcessTextDeleteAt(string incomingJson, RuntimeConnectionState state)
    {
        var parse = ParseTextAtRequest(incomingJson, "text-delete-at");
        if (!parse.IsSuccess)
        {
            return RuntimeMessageProcessResult.DispatchFailure(
                [RuntimeErrorEnvelopeBuilder.BuildMessageError(parse.Error!)],
                shouldCloseConnection: false
            );
        }

        var resolve = ResolveTextEntriesForPosition(state, parse.Namespace!, parse.Key!);
        if (!resolve.IsSuccess)
        {
            return resolve.ToDispatchFailure();
        }

        var index = parse.Index ?? -1;

        if (index >= resolve.Entries.Count)
        {
            return RuntimeMessageProcessResult.DispatchFailure(
                [RuntimeErrorEnvelopeBuilder.BuildMessageError("text-delete-at.index is out of range")],
                shouldCloseConnection: false
            );
        }

        var targetId = resolve.Entries[index].Id;
        var deleteEnvelope = BuildEnvelope(
            state.RoomId!,
            new JsonObject
            {
                ["TextDelete"] = new JsonObject
                {
                    ["namespace"] = parse.Namespace,
                    ["key"] = parse.Key,
                    ["target_id"] = targetId
                }
            }
        );

        return ExecuteSingleBridgeCommand(deleteEnvelope);
    }

    private RuntimeMessageProcessResult ExecuteSingleBridgeCommand(string commandJson)
    {
        var outbound = new List<string>();
        var bridgeResult = TryExecuteQueryProjectionStubCommand(state: null, commandJson, out var stubResult)
            ? stubResult
            : _bridge.ProcessJsonCommand(commandJson);
        if (!bridgeResult.IsSuccess)
        {
            outbound.Add(
                RuntimeErrorEnvelopeBuilder.BuildStatusError(
                    bridgeResult.Status.ToString(),
                    bridgeResult.DenyMetadata?.DenyMessage,
                    bridgeResult.DenyMetadata?.ReasonClass,
                    bridgeResult.DenyMetadata?.Command,
                    bridgeResult.DenyMetadata?.RequiredCapability
                )
            );
            return RuntimeMessageProcessResult.DispatchFailure(outbound, shouldCloseConnection: false);
        }

        var eventMapResult = _mapper.MapEventsJsonToOutboundMessages(bridgeResult.EventsJson);
        if (!eventMapResult.IsSuccess)
        {
            outbound.Add(
                RuntimeErrorEnvelopeBuilder.BuildMessageError(
                    eventMapResult.Error ?? "runtime event map failed"
                )
            );
            return RuntimeMessageProcessResult.DispatchFailure(outbound, shouldCloseConnection: false);
        }

        outbound.AddRange(eventMapResult.OutboundMessages);
        return RuntimeMessageProcessResult.DispatchSuccess(outbound, shouldCloseConnection: false);
    }

    private static bool TryExecuteQueryProjectionStubCommand(
        RuntimeConnectionState? state,
        string commandJson,
        out FfiJsonBridgeResult result)
    {
        result = FfiJsonBridgeResult.Success("[]");

        JsonNode? root;
        try
        {
            root = JsonNode.Parse(commandJson);
        }
        catch
        {
            return false;
        }

        if (root is not JsonObject envelope)
        {
            return false;
        }

        var roomId = envelope["room_id"]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(roomId))
        {
            return false;
        }

        if (envelope["command"] is not JsonObject command)
        {
            return false;
        }

        if (command.TryGetPropertyValue("RegisterQuerySpec", out var registerNode)
            && registerNode is JsonObject register)
        {
            var specId = register["query_spec_id"]?.GetValue<string>() ?? string.Empty;
            var version = register["version"]?.GetValue<string>() ?? string.Empty;
            if (state is not null && !string.IsNullOrWhiteSpace(specId))
            {
                state.QuerySpecStubs[specId] = new QuerySpecStubState(
                    specId,
                    version,
                    register["descriptor"]?.DeepClone()
                );
            }
            var canonicalHash = $"stub-canonical:{specId}:{version}";
            var events = new JsonArray
            {
                new JsonObject
                {
                    ["QuerySpecRegistered"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["query_spec_id"] = specId,
                        ["version"] = version,
                        ["canonical_hash"] = canonicalHash,
                        ["accepted"] = true
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        if (command.TryGetPropertyValue("BuildProjection", out var buildNode)
            && buildNode is JsonObject build)
        {
            var projectionId = build["projection_id"]?.GetValue<string>() ?? string.Empty;
            var querySpecId = build["query_spec_id"]?.GetValue<string>() ?? string.Empty;
            if (state is not null
                && !string.IsNullOrWhiteSpace(querySpecId)
                && !state.QuerySpecStubs.TryGetValue(querySpecId, out var querySpec))
            {
                var rejectEvents = new JsonArray
                {
                    new JsonObject
                    {
                        ["ProjectionBuildRejected"] = new JsonObject
                        {
                            ["room_id"] = roomId,
                            ["projection_id"] = projectionId,
                            ["reason_class"] = "reject.query_spec_not_found",
                            ["reason_message"] = "projection.build.query_spec_id is not registered"
                        }
                    }
                };
                result = FfiJsonBridgeResult.Success(rejectEvents.ToJsonString());
                return true;
            }

            var checkpoint = build["target_checkpoint"]?.DeepClone()
                ?? new JsonObject { ["selector"] = "latest" };
            var canonicalSnapshot = new CanonicalSnapshotState(
                0,
                new Dictionary<string, JsonNode?>(StringComparer.Ordinal),
                string.Empty,
                new JsonArray("seq:0")
            );
            var checkpointRejectClass = "reject.checkpoint_not_found";
            var checkpointRejectMessage = "projection.build.target_checkpoint does not resolve to known canonical snapshot";
            if (state is not null
                && !state.TryResolveCanonicalSnapshotAtCheckpoint(
                    checkpoint,
                    out canonicalSnapshot,
                    out checkpointRejectClass,
                    out checkpointRejectMessage
                ))
            {
                var rejectEvents = new JsonArray
                {
                    new JsonObject
                    {
                        ["ProjectionBuildRejected"] = new JsonObject
                        {
                            ["room_id"] = roomId,
                            ["projection_id"] = projectionId,
                            ["reason_class"] = checkpointRejectClass,
                            ["reason_message"] = checkpointRejectMessage
                        }
                    }
                };
                result = FfiJsonBridgeResult.Success(rejectEvents.ToJsonString());
                return true;
            }

            JsonNode? prefixNode = null;
            string? prefix = null;
            if (state?.QuerySpecStubs.TryGetValue(querySpecId, out var specWithDescriptor) == true
                && specWithDescriptor.Descriptor is JsonObject descriptorObj
                && descriptorObj.TryGetPropertyValue("prefix", out var prefixValue)
                && prefixValue is not null)
            {
                prefixNode = prefixValue.DeepClone();
                prefix = prefixValue.GetValue<string>();
            }
            var specVersion = state?.QuerySpecStubs.TryGetValue(querySpecId, out var querySpecState) == true
                ? querySpecState.Version
                : string.Empty;
            var rows = BuildProjectionRowsFromCanonicalSnapshot(
                canonicalSnapshot.Rows,
                projectionId,
                querySpecId,
                specVersion,
                prefixNode,
                prefix
            );
            var digest = ComputeProjectionDigest(rows, projectionId, querySpecId, specVersion);
            var resolvedCheckpoint = ResolveProjectionCheckpoint(checkpoint, canonicalSnapshot);

            if (state is not null && !string.IsNullOrWhiteSpace(projectionId))
            {
                state.ProjectionStubs[projectionId] = new ProjectionStubState(
                    projectionId,
                    querySpecId,
                    resolvedCheckpoint.DeepClone(),
                    rows.DeepClone(),
                    digest,
                    false,
                    null
                );
            }

            var events = new JsonArray
            {
                new JsonObject
                {
                    ["ProjectionBuildCompleted"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["projection_id"] = projectionId,
                        ["checkpoint"] = resolvedCheckpoint,
                        ["digest"] = digest
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        if (command.TryGetPropertyValue("ReadProjection", out var readNode)
            && readNode is JsonObject read)
        {
            var projectionId = read["projection_id"]?.GetValue<string>() ?? string.Empty;
            var limit = (int)(read["limit"]?.GetValue<ulong>() ?? 100UL);
            if (limit <= 0)
            {
                limit = 1;
            }
            var pageToken = read["page_token"]?.GetValue<string>();
            var start = ParseOffsetPageToken(pageToken);
            ProjectionStubState? projection = null;
            var foundProjection = state is not null
                && !string.IsNullOrWhiteSpace(projectionId)
                && state.ProjectionStubs.TryGetValue(projectionId, out projection);
            var checkpointNode = foundProjection
                ? projection!.Checkpoint.DeepClone()
                : new JsonObject { ["selector"] = "latest" };
            var rowsNode = new JsonArray();
            JsonNode? nextPageTokenNode = JsonValue.Create((string?)null);
            if (foundProjection && projection!.Rows is JsonArray storedRows)
            {
                if (start < 0)
                {
                    start = 0;
                }

                if (start < storedRows.Count)
                {
                    var endExclusive = Math.Min(storedRows.Count, start + limit);
                    for (var i = start; i < endExclusive; i++)
                    {
                        var row = storedRows[i];
                        if (row is not null)
                        {
                            rowsNode.Add(row.DeepClone());
                        }
                    }

                    if (endExclusive < storedRows.Count)
                    {
                        nextPageTokenNode = JsonValue.Create($"offset:{endExclusive}");
                    }
                }
            }
            var digestNode = foundProjection
                ? projection!.Digest
                : null;
            var events = new JsonArray
            {
                new JsonObject
                {
                    ["ProjectionReadResult"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["projection_id"] = projectionId,
                        ["checkpoint"] = checkpointNode,
                        ["rows"] = rowsNode,
                        ["digest"] = digestNode,
                        ["found"] = foundProjection,
                        ["next_page_token"] = nextPageTokenNode
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        if (command.TryGetPropertyValue("InvalidateProjection", out var invalidateNode)
            && invalidateNode is JsonObject invalidate)
        {
            var projectionId = invalidate["projection_id"]?.GetValue<string>() ?? string.Empty;
            var reason = invalidate["reason"]?.GetValue<string>() ?? "manual";
            if (state is not null
                && !string.IsNullOrWhiteSpace(projectionId)
                && state.ProjectionStubs.TryGetValue(projectionId, out var existingProjection))
            {
                state.ProjectionStubs[projectionId] = existingProjection with
                {
                    IsInvalidated = true,
                    InvalidationReason = reason
                };
            }
            var events = new JsonArray
            {
                new JsonObject
                {
                    ["ProjectionInvalidated"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["projection_id"] = projectionId,
                        ["reason"] = reason,
                        ["invalidated_at_hlc"] = 0
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        if (command.TryGetPropertyValue("ListProjections", out var listNode)
            && listNode is JsonObject list)
        {
            var querySpecId = list["query_spec_id"]?.GetValue<string>();
            var stateFilter = list["state_filter"]?.GetValue<string>();
            var items = new JsonArray();
            if (state is not null)
            {
                foreach (var projection in state.ProjectionStubs.Values.OrderBy(p => p.ProjectionId, StringComparer.Ordinal))
                {
                    if (!string.IsNullOrWhiteSpace(querySpecId)
                        && !string.Equals(projection.QuerySpecId, querySpecId, StringComparison.Ordinal))
                    {
                        continue;
                    }

                    if (string.Equals(stateFilter, "active", StringComparison.OrdinalIgnoreCase)
                        && projection.IsInvalidated)
                    {
                        continue;
                    }

                    if (string.Equals(stateFilter, "invalidated", StringComparison.OrdinalIgnoreCase)
                        && !projection.IsInvalidated)
                    {
                        continue;
                    }

                    items.Add(new JsonObject
                    {
                        ["projection_id"] = projection.ProjectionId,
                        ["query_spec_id"] = projection.QuerySpecId,
                        ["state"] = projection.IsInvalidated ? "invalidated" : "active",
                        ["digest"] = projection.Digest,
                        ["checkpoint"] = projection.Checkpoint.DeepClone(),
                        ["invalidation_reason"] = projection.InvalidationReason
                    });
                }
            }
            var events = new JsonArray
            {
                new JsonObject
                {
                    ["ProjectionListResult"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["query_spec_id"] = querySpecId,
                        ["items"] = items,
                        ["cursor"] = JsonValue.Create((string?)null)
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        if (command.TryGetPropertyValue("DescribeArchive", out var describeNode)
            && describeNode is JsonObject describe)
        {
            var archiveRef = describe["archive_ref"]?.GetValue<string>() ?? string.Empty;
            var events = new JsonArray
            {
                new JsonObject
                {
                    ["ArchiveDescribed"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["archive_ref"] = archiveRef,
                        ["manifest_id"] = "m.stub.0001",
                        ["format_version"] = "1",
                        ["archive_kind"] = "full_clone",
                        ["checkpoint"] = new JsonObject
                        {
                            ["frontier"] = new JsonArray("seq:0"),
                            ["canonical_hash"] = "0000000000000000000000000000000000000000000000000000000000000000"
                        },
                        ["payload_digest_set"] = new JsonObject
                        {
                            ["nodes"] = "sha256:stub-nodes",
                            ["blobs"] = "sha256:stub-blobs"
                        },
                        ["compatibility_window"] = new JsonObject
                        {
                            ["min_supported"] = "1",
                            ["max_supported"] = "1"
                        },
                        ["provenance"] = new JsonObject
                        {
                            ["source_room"] = roomId,
                            ["tool"] = "dotnet-host-stub"
                        }
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        if (command.TryGetPropertyValue("ValidateArchive", out var validateNode)
            && validateNode is JsonObject validate)
        {
            var archiveRef = validate["archive_ref"]?.GetValue<string>() ?? string.Empty;
            if (archiveRef.Contains("invalid-manifest", StringComparison.OrdinalIgnoreCase))
            {
                var rejectEvents = new JsonArray
                {
                    new JsonObject
                    {
                        ["ArchiveValidationRejected"] = new JsonObject
                        {
                            ["room_id"] = roomId,
                            ["archive_ref"] = archiveRef,
                            ["reason_class"] = "reject.archive_manifest_invalid",
                            ["reason_message"] = "manifest missing required field: checkpoint.canonical_hash"
                        }
                    }
                };
                result = FfiJsonBridgeResult.Success(rejectEvents.ToJsonString());
                return true;
            }

            var mode = validate["mode"]?.GetValue<string>() ?? "metadata_only";
            var events = new JsonArray
            {
                new JsonObject
                {
                    ["ArchiveValidated"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["archive_ref"] = archiveRef,
                        ["accepted"] = true,
                        ["mode"] = mode,
                        ["checks"] = new JsonArray("manifest", "compatibility"),
                        ["compatibility_window"] = new JsonObject
                        {
                            ["min_supported"] = "1",
                            ["max_supported"] = "1"
                        }
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        if (command.TryGetPropertyValue("ImportArchive", out var importNode)
            && importNode is JsonObject import)
        {
            var archiveRef = import["archive_ref"]?.GetValue<string>() ?? string.Empty;
            if (archiveRef.Contains("digest-mismatch", StringComparison.OrdinalIgnoreCase))
            {
                var rejectEvents = new JsonArray
                {
                    new JsonObject
                    {
                        ["ArchiveImportRejected"] = new JsonObject
                        {
                            ["room_id"] = roomId,
                            ["archive_ref"] = archiveRef,
                            ["reason_class"] = "reject.archive_digest_mismatch",
                            ["reason_message"] = "payload digest mismatch for nodes payload"
                        }
                    }
                };
                result = FfiJsonBridgeResult.Success(rejectEvents.ToJsonString());
                return true;
            }

            var canonicalHash = "1111111111111111111111111111111111111111111111111111111111111111";
            var events = new JsonArray
            {
                new JsonObject
                {
                    ["ArchiveImported"] = new JsonObject
                    {
                        ["room_id"] = roomId,
                        ["archive_ref"] = archiveRef,
                        ["canonical_hash"] = canonicalHash,
                        ["checkpoint"] = new JsonObject
                        {
                            ["frontier"] = new JsonArray("seq:0"),
                            ["canonical_hash"] = canonicalHash
                        },
                        ["imported_nodes"] = 0,
                        ["imported_blobs"] = 0
                    }
                }
            };
            result = FfiJsonBridgeResult.Success(events.ToJsonString());
            return true;
        }

        return false;
    }

    private static void TrackCanonicalMapMutation(RuntimeConnectionState state, string commandJson)
    {
        JsonNode? root;
        try
        {
            root = JsonNode.Parse(commandJson);
        }
        catch
        {
            return;
        }

        if (root is not JsonObject envelope || envelope["command"] is not JsonObject command)
        {
            return;
        }

        if (command.TryGetPropertyValue("MapSet", out var setNode) && setNode is JsonObject setObj)
        {
            var key = setObj["key"]?.GetValue<string>();
            if (!string.IsNullOrWhiteSpace(key))
            {
                state.CanonicalMapRows[key] = setObj["value"]?.DeepClone();
                state.AdvanceCanonicalSequenceAndSnapshot();
            }

            return;
        }

        if (command.TryGetPropertyValue("MapDelete", out var deleteNode) && deleteNode is JsonObject deleteObj)
        {
            var key = deleteObj["key"]?.GetValue<string>();
            if (!string.IsNullOrWhiteSpace(key))
            {
                state.CanonicalMapRows.Remove(key);
                state.AdvanceCanonicalSequenceAndSnapshot();
            }
        }
    }

    private static JsonArray BuildProjectionRowsFromCanonicalSnapshot(
        Dictionary<string, JsonNode?> canonicalRows,
        string projectionId,
        string querySpecId,
        string specVersion,
        JsonNode? prefixNode,
        string? prefix)
    {
        var rows = new JsonArray();
        if (canonicalRows.Count == 0)
        {
            return rows;
        }

        foreach (var kvp in canonicalRows.OrderBy(x => x.Key, StringComparer.Ordinal))
        {
            if (!string.IsNullOrWhiteSpace(prefix)
                && !kvp.Key.StartsWith(prefix, StringComparison.Ordinal))
            {
                continue;
            }

            rows.Add(new JsonObject
            {
                ["k"] = kvp.Key,
                ["v"] = CanonicalValueToRowString(kvp.Value),
                ["projection_id"] = projectionId,
                ["query_spec_id"] = querySpecId,
                ["version"] = specVersion,
                ["prefix"] = prefixNode?.DeepClone() ?? JsonValue.Create((string?)null)
            });
        }

        return rows;
    }

    private static JsonNode ResolveProjectionCheckpoint(JsonNode checkpoint, CanonicalSnapshotState snapshot)
    {
        var checkpointObj = checkpoint.DeepClone() as JsonObject ?? new JsonObject { ["selector"] = "latest" };
        checkpointObj["canonical_seq"] = snapshot.Sequence;
        checkpointObj["canonical_hash"] = snapshot.CanonicalHash;
        checkpointObj["frontier"] = snapshot.Frontier.DeepClone();
        return checkpointObj;
    }

    private static string CanonicalValueToRowString(JsonNode? value)
    {
        if (value is null)
        {
            return string.Empty;
        }

        if (value is JsonValue jsonValue)
        {
            try
            {
                return jsonValue.GetValue<string>();
            }
            catch
            {
                return jsonValue.ToJsonString();
            }
        }

        return value.ToJsonString();
    }

    private static string ComputeProjectionDigest(
        JsonArray rows,
        string projectionId,
        string querySpecId,
        string version)
    {
        // Deterministic digest over projection identity + row payload
        // sorted by key so pagination windows share one stable digest.
        var keyVals = rows
            .OfType<JsonObject>()
            .Select(row =>
            {
                var k = row["k"]?.GetValue<string>() ?? string.Empty;
                var v = row["v"]?.GetValue<string>() ?? string.Empty;
                return $"{k}={v}";
            })
            .OrderBy(x => x, StringComparer.Ordinal);

        var payload = $"projection={projectionId}|spec={querySpecId}|version={version}|rows={string.Join(";", keyVals)}";
        var bytes = SHA256.HashData(Encoding.UTF8.GetBytes(payload));
        return Convert.ToHexString(bytes).ToLowerInvariant();
    }

    private static int ParseOffsetPageToken(string? pageToken)
    {
        if (string.IsNullOrWhiteSpace(pageToken))
        {
            return 0;
        }

        const string prefix = "offset:";
        if (!pageToken.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
        {
            return 0;
        }

        var raw = pageToken[prefix.Length..];
        return int.TryParse(raw, out var offset) && offset > 0 ? offset : 0;
    }

    private ResolveTextEntriesResult ResolveTextEntriesForPosition(
        RuntimeConnectionState state,
        string @namespace,
        string key
    )
    {
        if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
        {
            return ResolveTextEntriesResult.Failure("hello must be sent first");
        }

        var getEnvelope = BuildEnvelope(
            state.RoomId!,
            new JsonObject
            {
                ["TextGet"] = new JsonObject
                {
                    ["namespace"] = @namespace,
                    ["key"] = key
                }
            }
        );

        var bridgeResult = _bridge.ProcessJsonCommand(getEnvelope);
        if (!bridgeResult.IsSuccess)
        {
            return ResolveTextEntriesResult.FailureStatus(bridgeResult.Status.ToString());
        }

        if (!TryExtractTextEntries(bridgeResult.EventsJson, out var entries))
        {
            return ResolveTextEntriesResult.Failure("text-get event decode failed");
        }

        return ResolveTextEntriesResult.Success(entries);
    }

    private static bool TryExtractTextEntries(string eventsJson, out List<TextEntryRef> entries)
    {
        entries = [];
        JsonNode? root;
        try
        {
            root = JsonNode.Parse(eventsJson);
        }
        catch
        {
            return false;
        }

        if (root is not JsonArray arr)
        {
            return false;
        }

        foreach (var item in arr)
        {
            if (item is not JsonObject obj)
            {
                continue;
            }

            if (obj["TextValueRead"] is not JsonObject readObj)
            {
                continue;
            }

            if (readObj["entries"] is not JsonArray entryArr)
            {
                return false;
            }

            foreach (var entryNode in entryArr)
            {
                if (entryNode is not JsonObject entryObj)
                {
                    continue;
                }

                var id = entryObj["id"]?.GetValue<string>();
                if (!string.IsNullOrWhiteSpace(id))
                {
                    entries.Add(new TextEntryRef(id!));
                }
            }

            return true;
        }

        return false;
    }

    private static bool TryReadHelperType(string incomingJson, out string? type)
    {
        type = null;
        try
        {
            var root = JsonNode.Parse(incomingJson) as JsonObject;
            type = root?["type"]?.GetValue<string>();
            return !string.IsNullOrWhiteSpace(type);
        }
        catch
        {
            return false;
        }
    }

    private static void RecordControlPlaneDenyMetricIfApplicable(string? mapError)
    {
        if (!TryParseControlPlaneForbidden(mapError, out var command, out var requiredCapability))
        {
            return;
        }

        RuntimeControlPlaneDeniedCounter.Add(
            1,
            KeyValuePair.Create<string, object?>("host", "dotnet-host"),
            KeyValuePair.Create<string, object?>("command", command),
            KeyValuePair.Create<string, object?>("required_capability", requiredCapability),
            KeyValuePair.Create<string, object?>("reason_class", "reject.control_plane_forbidden")
        );

        LegacyRuntimeControlPlaneDeniedCounter.Add(
            1,
            KeyValuePair.Create<string, object?>("host", "dotnet-host"),
            KeyValuePair.Create<string, object?>("command", command),
            KeyValuePair.Create<string, object?>("required_capability", requiredCapability),
            KeyValuePair.Create<string, object?>("reason_class", "reject.control_plane_forbidden")
        );
    }

    private static bool TryParseControlPlaneForbidden(
        string? mapError,
        out string command,
        out string requiredCapability
    )
    {
        command = "unknown";
        requiredCapability = "unknown";

        if (string.IsNullOrWhiteSpace(mapError))
        {
            return false;
        }

        const string prefix = "reject.control_plane_forbidden:";
        if (!mapError.StartsWith(prefix, StringComparison.Ordinal))
        {
            return false;
        }

        var body = mapError[prefix.Length..].Trim();
        var segments = body.Split(' ', StringSplitOptions.RemoveEmptyEntries);
        foreach (var segment in segments)
        {
            if (segment.StartsWith("command=", StringComparison.Ordinal))
            {
                command = segment["command=".Length..];
            }
            else if (segment.StartsWith("requires=", StringComparison.Ordinal))
            {
                requiredCapability = segment["requires=".Length..];
            }
        }

        command = NormalizeCommandLabel(command);
        requiredCapability = NormalizeCapabilityLabel(requiredCapability);
        return true;
    }

    private static string NormalizeCommandLabel(string value)
    {
        return value switch
        {
            "set-policy" => "set-policy",
            "set-room-key" => "set-room-key",
            "start-tick" => "start-tick",
            "stop-tick" => "stop-tick",
            "query.register" => "query.register",
            "projection.build" => "projection.build",
            "projection.read" => "projection.read",
            "projection.invalidate" => "projection.invalidate",
            "projection.list" => "projection.list",
            "archive.describe" => "archive.describe",
            "archive.validate" => "archive.validate",
            "archive.import" => "archive.import",
            "topology.create-child" => "topology.create-child",
            "topology.describe-lineage" => "topology.describe-lineage",
            "topology.list-children" => "topology.list-children",
            "topology.propose-promotion" => "topology.propose-promotion",
            "topology.validate-promotion" => "topology.validate-promotion",
            "topology.apply-promotion" => "topology.apply-promotion",
            _ => "unknown"
        };
    }

    private static string NormalizeCapabilityLabel(string value)
    {
        return value switch
        {
            "policy.admin" => "policy.admin",
            "room.admin" => "room.admin",
            "tick.admin" => "tick.admin",
            "query.admin" => "query.admin",
            "query.read" => "query.read",
            "archive.admin" => "archive.admin",
            "archive.read" => "archive.read",
            "topology.admin" => "topology.admin",
            _ => "unknown"
        };
    }

    private static TextAtRequestParseResult ParseTextAtRequest(string incomingJson, string expectedType)
    {
        JsonNode? root;
        try
        {
            root = JsonNode.Parse(incomingJson);
        }
        catch
        {
            return TextAtRequestParseResult.Failure("invalid JSON");
        }

        if (root is not JsonObject obj)
        {
            return TextAtRequestParseResult.Failure("invalid JSON");
        }

        var type = obj["type"]?.GetValue<string>();
        if (!string.Equals(type, expectedType, StringComparison.OrdinalIgnoreCase))
        {
            return TextAtRequestParseResult.Failure("unsupported helper type");
        }

        var key = obj["key"]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(key))
        {
            return TextAtRequestParseResult.Failure($"{expectedType}.key is required");
        }

        int? index;
        try
        {
            index = obj["index"]?.GetValue<int>();
        }
        catch
        {
            return TextAtRequestParseResult.Failure($"{expectedType}.index is required");
        }

        if (index is null || index < 0)
        {
            return TextAtRequestParseResult.Failure($"{expectedType}.index is required");
        }

        return TextAtRequestParseResult.Success(
            obj["namespace"]?.GetValue<string>() ?? string.Empty,
            key,
            index.Value,
            obj["ch"]?.GetValue<string>()
        );
    }

    private static string BuildEnvelope(string roomId, JsonObject commandNode)
    {
        var envelope = new JsonObject
        {
            ["room_id"] = roomId,
            ["command"] = commandNode
        };

        return envelope.ToJsonString();
    }
}

internal sealed record TextEntryRef(string Id);

internal sealed record TextAtRequestParseResult(
    bool IsSuccess,
    string? Error,
    string? Namespace,
    string? Key,
    int? Index,
    string? Ch
)
{
    public static TextAtRequestParseResult Success(string @namespace, string key, int index, string? ch) =>
        new(true, null, @namespace, key, index, ch);

    public static TextAtRequestParseResult Failure(string error) =>
        new(false, error, null, null, null, null);
}

internal sealed record ResolveTextEntriesResult(
    bool IsSuccess,
    bool IsStatusFailure,
    string? Error,
    List<TextEntryRef> Entries
)
{
    public static ResolveTextEntriesResult Success(List<TextEntryRef> entries) =>
        new(true, false, null, entries);

    public static ResolveTextEntriesResult Failure(string error) =>
        new(false, false, error, []);

    public static ResolveTextEntriesResult FailureStatus(string status) =>
        new(false, true, status, []);

    public RuntimeMessageProcessResult ToDispatchFailure()
    {
        if (IsStatusFailure)
        {
            return RuntimeMessageProcessResult.DispatchFailure(
                [RuntimeErrorEnvelopeBuilder.BuildStatusError(Error ?? "Internal")],
                shouldCloseConnection: false
            );
        }

        return RuntimeMessageProcessResult.DispatchFailure(
            [RuntimeErrorEnvelopeBuilder.BuildMessageError(Error ?? "text helper failed")],
            shouldCloseConnection: false
        );
    }
}

public sealed record RuntimeMessageProcessResult(
    bool DispatchSucceeded,
    bool ShouldCloseConnection,
    IReadOnlyList<string> OutboundMessages
)
{
    public static RuntimeMessageProcessResult DispatchSuccess(
        IReadOnlyList<string> outboundMessages,
        bool shouldCloseConnection
    ) =>
        new(true, shouldCloseConnection, outboundMessages);

    public static RuntimeMessageProcessResult DispatchFailure(
        IReadOnlyList<string> outboundMessages,
        bool shouldCloseConnection
    ) =>
        new(false, shouldCloseConnection, outboundMessages);
}