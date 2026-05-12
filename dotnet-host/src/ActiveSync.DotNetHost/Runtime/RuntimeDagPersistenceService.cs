using ActiveSync.DotNetHost.Ffi;
using ActiveSync.Host.Abstractions.Providers;
using System.Collections.Concurrent;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace ActiveSync.DotNetHost.Runtime;

public sealed class RuntimeDagPersistenceService
{
    private readonly INodeStoreProvider _nodeStore;
    private readonly IRuntimeCommandBridge _bridge;
    private readonly ILogger<RuntimeDagPersistenceService> _logger;
    private readonly ConcurrentDictionary<string, Task> _hydrateByRoom =
        new(StringComparer.Ordinal);

    public RuntimeDagPersistenceService(
        INodeStoreProvider nodeStore,
        IRuntimeCommandBridge bridge,
        ILogger<RuntimeDagPersistenceService> logger
    )
    {
        _nodeStore = nodeStore;
        _bridge = bridge;
        _logger = logger;
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

            var hashHex = Convert.ToHexStringLower(SHA256.HashData(payload));
            var record = new AcceptedNodeRecord($"pack:{hashHex}", payload);

            await _nodeStore.PersistAcceptedNodesAsync(roomId, [record], cancellationToken);
            _logger.LogInformation(
                "runtime dag persisted room={Room} bytes={Bytes} key={Key}",
                roomId,
                payload.Length,
                record.NodeIdHex
            );
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

    private async Task HydrateRoomCoreAsync(string roomId, CancellationToken cancellationToken)
    {
        try
        {
            var ensureStatus = SubmitCommand(BuildEnsureRoomEnvelope(roomId));
            if (ensureStatus != AsStatus.Ok)
            {
                _logger.LogWarning(
                    "runtime dag hydrate ensure-room failed room={Room} status={Status}",
                    roomId,
                    ensureStatus
                );
                return;
            }

            var snapshot = await _nodeStore.LoadRoomSnapshotAsync(roomId, cancellationToken);
            if (snapshot is null || snapshot.Nodes.Count == 0)
            {
                _logger.LogInformation("runtime dag hydrate room={Room} source=empty", roomId);
                return;
            }

            var imported = 0;
            foreach (var node in snapshot.Nodes)
            {
                if (!node.NodeIdHex.StartsWith("pack:", StringComparison.Ordinal))
                {
                    // Runtime persistence currently stores full pack payloads.
                    continue;
                }

                var nodesB64 = Convert.ToBase64String(node.Payload);
                var status = SubmitCommand(BuildImportPackEnvelope(roomId, nodesB64));
                if (status == AsStatus.Ok)
                {
                    imported += 1;
                }
            }

            _logger.LogInformation(
                "runtime dag hydrate room={Room} records={Records} imported={Imported}",
                roomId,
                snapshot.Nodes.Count,
                imported
            );
        }
        catch (Exception ex)
        {
            _logger.LogWarning(ex, "runtime dag hydrate failed room={Room}", roomId);
        }
    }

    private AsStatus SubmitCommand(string json)
    {
        var result = _bridge.ProcessJsonCommand(json);
        return result.Status;
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
}