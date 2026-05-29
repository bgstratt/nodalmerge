using NodalMerge.DotNetHost.Ffi;
using System.Diagnostics.Metrics;
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
            var bridgeResult = _bridge.ProcessJsonCommand(commandJson);
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
        var bridgeResult = _bridge.ProcessJsonCommand(commandJson);
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