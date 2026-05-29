using System.Text.Json;
using System.Text.Json.Nodes;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json.Serialization;
using NodalMerge.Host.Composition;

namespace NodalMerge.DotNetHost.Runtime;

public sealed class RuntimeProtocolMapper
{
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNameCaseInsensitive = true
    };

    private readonly CapabilityProfileExpander _capabilityProfileExpander;
    private readonly string? _trustedServerPeerPubkeyHex;

    public RuntimeProtocolMapper()
        : this(new CapabilityProfileExpander(new CapabilityCompositionOptions(Enabled: false, ProfilePath: null)), null)
    {
    }

    public RuntimeProtocolMapper(string? trustedServerPeerPubkeyHex)
        : this(new CapabilityProfileExpander(new CapabilityCompositionOptions(Enabled: false, ProfilePath: null)), trustedServerPeerPubkeyHex)
    {
    }

    public RuntimeProtocolMapper(CapabilityProfileExpander capabilityProfileExpander)
        : this(capabilityProfileExpander, null)
    {
    }

    public RuntimeProtocolMapper(
        CapabilityProfileExpander capabilityProfileExpander,
        string? trustedServerPeerPubkeyHex)
    {
        _capabilityProfileExpander = capabilityProfileExpander;
        _trustedServerPeerPubkeyHex = string.IsNullOrWhiteSpace(trustedServerPeerPubkeyHex)
            ? null
            : trustedServerPeerPubkeyHex.Trim();
    }

    public RuntimeMapResult MapIncomingMessageToCommandJsons(
        string incomingJson,
        RuntimeConnectionState state
    )
    {
        RuntimeInboundMessage? message;
        try
        {
            message = JsonSerializer.Deserialize<RuntimeInboundMessage>(incomingJson, JsonOptions);
        }
        catch
        {
            return RuntimeMapResult.Failure("invalid JSON");
        }

        if (message is null || string.IsNullOrWhiteSpace(message.Type))
        {
            return RuntimeMapResult.Failure("missing message type");
        }

        var type = message.Type.Trim();

        if (string.Equals(type, "hello", StringComparison.OrdinalIgnoreCase))
        {
            if (state.IsInitialized)
            {
                return RuntimeMapResult.Failure("hello already processed for this connection");
            }

            if (string.IsNullOrWhiteSpace(message.Room))
            {
                return RuntimeMapResult.Failure("hello.room is required");
            }

            if (string.IsNullOrWhiteSpace(message.Pubkey))
            {
                return RuntimeMapResult.Failure("hello.pubkey is required");
            }

            state.RoomId = message.Room;
            state.PeerPubkeyHex = message.Pubkey;
            UpdateServerPeerStatus(state, state.PeerPubkeyHex);
            state.IsInitialized = true;
            state.SessionId = ResolveSessionId(message, state);

            var capsNode = new JsonObject
            {
                ["supports_ibf"] = message.Caps?.SupportsIbf ?? true,
                ["supports_mst"] = message.Caps?.SupportsMst ?? true
            };

            if (message.Token is not null)
            {
                var tokenError = ValidateToken(message.Token);
                if (tokenError is not null)
                {
                    return RuntimeMapResult.Failure(tokenError);
                }
            }

            var capabilityError = TrySetSessionCapabilities(state, message.Token?.GetCapabilities(), message.Token?.GetCapabilityProfileVersion());
            if (capabilityError is not null)
            {
                return RuntimeMapResult.Failure(capabilityError);
            }

            var clientHelloPayload = new JsonObject
            {
                ["peer_pubkey_hex"] = state.PeerPubkeyHex,
                ["client_frontier"] = new JsonArray((message.Frontier ?? []).Select(x => (JsonNode?)x).ToArray()),
                ["capabilities"] = capsNode
            };
            if (message.Token is not null)
            {
                clientHelloPayload["token"] = BuildTokenJson(message.Token);
            }

            var commands = new List<string>
            {
                SerializeEnvelope(state.RoomId!, JsonValue.Create("EnsureRoom")!),
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["OpenSession"] = new JsonObject
                        {
                            ["session_id"] = state.SessionId,
                            ["peer_pubkey_hex"] = state.PeerPubkeyHex
                        }
                    }
                ),
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["ClientHello"] = new JsonObject
                        {
                            ["session_id"] = state.SessionId,
                            ["hello"] = clientHelloPayload
                        }
                    }
                ),
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["RequestServerPack"] = new JsonObject
                        {
                            ["known_ids"] = new JsonArray()
                        }
                    }
                )
            };

            return RuntimeMapResult.Success(commands);
        }

        if (string.Equals(type, "ensure-room", StringComparison.OrdinalIgnoreCase))
        {
            if (
                state.IsInitialized
                && HasExplicitRoom(message)
                && !string.Equals(message.Room, state.RoomId, StringComparison.Ordinal)
            )
            {
                return RuntimeMapResult.Failure("ensure-room.room must match initialized room");
            }

            var roomId = ResolveRoomId(message, state);
            if (roomId is null)
            {
                return RuntimeMapResult.Failure("room is required");
            }

            state.RoomId = roomId;
            return RuntimeMapResult.Success([
                SerializeEnvelope(roomId, JsonValue.Create("EnsureRoom")!)
            ]);
        }

        if (string.Equals(type, "open-session", StringComparison.OrdinalIgnoreCase))
        {
            if (
                state.IsInitialized
                && HasExplicitRoom(message)
                && !string.Equals(message.Room, state.RoomId, StringComparison.Ordinal)
            )
            {
                return RuntimeMapResult.Failure("open-session.room must match initialized room");
            }

            if (
                state.IsInitialized
                && HasExplicitPubkey(message)
                && !string.Equals(message.Pubkey, state.PeerPubkeyHex, StringComparison.Ordinal)
            )
            {
                return RuntimeMapResult.Failure("open-session.pubkey must match initialized pubkey");
            }

            var roomId = ResolveRoomId(message, state);
            if (roomId is null)
            {
                return RuntimeMapResult.Failure("room is required");
            }

            var peerPubkeyHex = ResolvePeerPubkey(message, state);
            if (peerPubkeyHex is null)
            {
                return RuntimeMapResult.Failure("pubkey is required");
            }

            state.RoomId = roomId;
            state.PeerPubkeyHex = peerPubkeyHex;
            UpdateServerPeerStatus(state, state.PeerPubkeyHex);
            state.IsInitialized = true;
            SetSessionCapabilities(state, null);

            var sessionId = ResolveSessionId(message, state);
            state.SessionId = sessionId;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    roomId,
                    new JsonObject
                    {
                        ["OpenSession"] = new JsonObject
                        {
                            ["session_id"] = sessionId,
                            ["peer_pubkey_hex"] = peerPubkeyHex
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "client-hello", StringComparison.OrdinalIgnoreCase))
        {
            if (
                state.IsInitialized
                && HasExplicitRoom(message)
                && !string.Equals(message.Room, state.RoomId, StringComparison.Ordinal)
            )
            {
                return RuntimeMapResult.Failure("client-hello.room must match initialized room");
            }

            if (
                state.IsInitialized
                && HasExplicitPubkey(message)
                && !string.Equals(message.Pubkey, state.PeerPubkeyHex, StringComparison.Ordinal)
            )
            {
                return RuntimeMapResult.Failure("client-hello.pubkey must match initialized pubkey");
            }

            var roomId = ResolveRoomId(message, state);
            if (roomId is null)
            {
                return RuntimeMapResult.Failure("room is required");
            }

            var peerPubkeyHex = ResolvePeerPubkey(message, state);
            if (peerPubkeyHex is null)
            {
                return RuntimeMapResult.Failure("pubkey is required");
            }

            state.RoomId = roomId;
            state.PeerPubkeyHex = peerPubkeyHex;
            UpdateServerPeerStatus(state, state.PeerPubkeyHex);
            state.IsInitialized = true;

            var capsNode = new JsonObject
            {
                ["supports_ibf"] = message.Caps?.SupportsIbf ?? true,
                ["supports_mst"] = message.Caps?.SupportsMst ?? true
            };

            if (message.Token is not null)
            {
                var tokenError = ValidateToken(message.Token);
                if (tokenError is not null)
                {
                    return RuntimeMapResult.Failure(tokenError);
                }
            }

            var capabilityError = TrySetSessionCapabilities(state, message.Token?.GetCapabilities(), message.Token?.GetCapabilityProfileVersion());
            if (capabilityError is not null)
            {
                return RuntimeMapResult.Failure(capabilityError);
            }

            var clientHelloPayload = new JsonObject
            {
                ["peer_pubkey_hex"] = peerPubkeyHex,
                ["client_frontier"] = new JsonArray((message.Frontier ?? []).Select(x => (JsonNode?)x).ToArray()),
                ["capabilities"] = capsNode
            };
            if (message.Token is not null)
            {
                clientHelloPayload["token"] = BuildTokenJson(message.Token);
            }

            var sessionId = ResolveSessionId(message, state);
            state.SessionId = sessionId;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    roomId,
                    new JsonObject
                    {
                        ["ClientHello"] = new JsonObject
                        {
                            ["session_id"] = sessionId,
                            ["hello"] = clientHelloPayload
                        }
                    }
                ),
                SerializeEnvelope(
                    roomId,
                    new JsonObject
                    {
                        ["RequestServerPack"] = new JsonObject
                        {
                            ["known_ids"] = new JsonArray()
                        }
                    }
                )
            ]);
        }

        if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
        {
            return RuntimeMapResult.Failure("hello must be sent first");
        }

        if (string.Equals(type, "noop", StringComparison.OrdinalIgnoreCase))
        {
            return RuntimeMapResult.Success([
                SerializeEnvelope(state.RoomId, JsonValue.Create("Noop")!)
            ]);
        }

        if (string.Equals(type, "close-session", StringComparison.OrdinalIgnoreCase))
        {
            var closeSessionId = ResolveSessionId(message, state);
            state.SessionId = closeSessionId;
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["CloseSession"] = new JsonObject
                        {
                            ["session_id"] = closeSessionId
                        }
                    }
                )
            ], shouldCloseConnection: true);
        }

        if (string.Equals(type, "set-room-key", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (!IsControlPlaneAllowed(state, "room.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=set-room-key requires=room.admin");
            }

            if (string.IsNullOrWhiteSpace(message.Pubkey))
            {
                return RuntimeMapResult.Failure("set-room-key.pubkey is required");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["SetRoomKey"] = new JsonObject
                        {
                            ["pubkey_hex"] = message.Pubkey
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "set-policy", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (!IsControlPlaneAllowed(state, "policy.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=set-policy requires=policy.admin");
            }

            var defaultValue = string.IsNullOrWhiteSpace(message.PolicyDefault)
                ? "allow"
                : message.PolicyDefault!.Trim();
            if (!string.Equals(defaultValue, "allow", StringComparison.Ordinal)
                && !string.Equals(defaultValue, "deny", StringComparison.Ordinal))
            {
                return RuntimeMapResult.Failure($"set-policy: unknown default '{defaultValue}', use 'allow' or 'deny'");
            }

            var ruleNodes = new JsonArray();
            foreach (var rule in message.PolicyRules ?? [])
            {
                if (string.IsNullOrWhiteSpace(rule.PathGlob))
                {
                    return RuntimeMapResult.Failure("set-policy: each rule must have a 'path_glob' string");
                }

                ruleNodes.Add(new JsonObject
                {
                    ["path_glob"] = rule.PathGlob,
                    ["can_write"] = new JsonArray((rule.CanWrite ?? []).Select(x => (JsonNode?)x).ToArray())
                });
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["SetPolicy"] = new JsonObject
                        {
                            ["default"] = defaultValue,
                            ["rules"] = ruleNodes
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "start-tick", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (!IsControlPlaneAllowed(state, "tick.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=start-tick requires=tick.admin");
            }

            var intervalMs = message.IntervalMs.GetValueOrDefault(16);
            if (intervalMs == 0)
            {
                intervalMs = 1;
            }

            var intentPrefix = string.IsNullOrWhiteSpace(message.IntentPrefix)
                ? "intent/"
                : message.IntentPrefix!;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["StartTick"] = new JsonObject
                        {
                            ["interval_ms"] = intervalMs,
                            ["intent_prefix"] = intentPrefix
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "stop-tick", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (!IsControlPlaneAllowed(state, "tick.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=stop-tick requires=tick.admin");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(state.RoomId, JsonValue.Create("StopTick")!)
            ]);
        }

        if (string.Equals(type, "query.register", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "query.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=query.register requires=query.admin");
            }

            if (string.IsNullOrWhiteSpace(message.QuerySpecId))
            {
                return RuntimeMapResult.Failure("query.register.query_spec_id is required");
            }

            if (string.IsNullOrWhiteSpace(message.QuerySpecVersion))
            {
                return RuntimeMapResult.Failure("query.register.version is required");
            }

            if (message.Descriptor is null)
            {
                return RuntimeMapResult.Failure("query.register.descriptor is required");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["RegisterQuerySpec"] = new JsonObject
                        {
                            ["query_spec_id"] = message.QuerySpecId,
                            ["version"] = message.QuerySpecVersion,
                            ["descriptor"] = message.Descriptor.DeepClone()
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "projection.build", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "query.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=projection.build requires=query.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ProjectionId))
            {
                return RuntimeMapResult.Failure("projection.build.projection_id is required");
            }

            if (string.IsNullOrWhiteSpace(message.QuerySpecId))
            {
                return RuntimeMapResult.Failure("projection.build.query_spec_id is required");
            }

            var buildPayload = new JsonObject
            {
                ["projection_id"] = message.ProjectionId,
                ["query_spec_id"] = message.QuerySpecId
            };
            if (message.TargetCheckpoint is not null)
            {
                buildPayload["target_checkpoint"] = message.TargetCheckpoint.DeepClone();
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["BuildProjection"] = buildPayload
                    }
                )
            ]);
        }

        if (string.Equals(type, "projection.read", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "query.read"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=projection.read requires=query.read");
            }

            if (string.IsNullOrWhiteSpace(message.ProjectionId))
            {
                return RuntimeMapResult.Failure("projection.read.projection_id is required");
            }

            var readPayload = new JsonObject
            {
                ["projection_id"] = message.ProjectionId,
                ["limit"] = message.Limit.GetValueOrDefault(100)
            };
            if (!string.IsNullOrWhiteSpace(message.PageToken))
            {
                readPayload["page_token"] = message.PageToken;
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ReadProjection"] = readPayload
                    }
                )
            ]);
        }

        if (string.Equals(type, "projection.invalidate", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "query.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=projection.invalidate requires=query.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ProjectionId))
            {
                return RuntimeMapResult.Failure("projection.invalidate.projection_id is required");
            }

            var reason = string.IsNullOrWhiteSpace(message.InvalidationReason)
                ? "manual"
                : message.InvalidationReason;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["InvalidateProjection"] = new JsonObject
                        {
                            ["projection_id"] = message.ProjectionId,
                            ["reason"] = reason
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "projection.list", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "query.read"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=projection.list requires=query.read");
            }

            var listPayload = new JsonObject();
            if (!string.IsNullOrWhiteSpace(message.QuerySpecId))
            {
                listPayload["query_spec_id"] = message.QuerySpecId;
            }
            if (!string.IsNullOrWhiteSpace(message.StateFilter))
            {
                listPayload["state_filter"] = message.StateFilter;
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ListProjections"] = listPayload
                    }
                )
            ]);
        }

        if (string.Equals(type, "archive.describe", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "archive.read"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=archive.describe requires=archive.read");
            }

            if (string.IsNullOrWhiteSpace(message.ArchiveRef))
            {
                return RuntimeMapResult.Failure("archive.describe.archive_ref is required");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["DescribeArchive"] = new JsonObject
                        {
                            ["archive_ref"] = message.ArchiveRef
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "archive.validate", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "archive.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=archive.validate requires=archive.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ArchiveRef))
            {
                return RuntimeMapResult.Failure("archive.validate.archive_ref is required");
            }

            var mode = string.IsNullOrWhiteSpace(message.ArchiveMode)
                ? "metadata_only"
                : message.ArchiveMode;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ValidateArchive"] = new JsonObject
                        {
                            ["archive_ref"] = message.ArchiveRef,
                            ["mode"] = mode
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "archive.import", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "archive.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=archive.import requires=archive.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ArchiveRef))
            {
                return RuntimeMapResult.Failure("archive.import.archive_ref is required");
            }

            var importMode = string.IsNullOrWhiteSpace(message.ImportMode)
                ? "full_clone"
                : message.ImportMode;
            var importPayload = new JsonObject
            {
                ["archive_ref"] = message.ArchiveRef,
                ["import_mode"] = importMode
            };
            if (message.ExpectedCheckpoint is not null)
            {
                importPayload["expected_checkpoint"] = message.ExpectedCheckpoint.DeepClone();
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ImportArchive"] = importPayload
                    }
                )
            ]);
        }

        if (string.Equals(type, "topology.create-child", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "topology.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=topology.create-child requires=topology.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ChildRoomId))
            {
                return RuntimeMapResult.Failure("topology.create-child.child_room_id is required");
            }

            if (string.IsNullOrWhiteSpace(message.ChildPurpose))
            {
                return RuntimeMapResult.Failure("topology.create-child.child_purpose is required");
            }

            if (string.IsNullOrWhiteSpace(message.PromotionPolicyId))
            {
                return RuntimeMapResult.Failure("topology.create-child.promotion_policy_id is required");
            }

            if (message.ParentCheckpoint is null)
            {
                return RuntimeMapResult.Failure("topology.create-child.parent_checkpoint is required");
            }

            var parentRoomId = string.IsNullOrWhiteSpace(message.ParentRoomId)
                ? state.RoomId
                : message.ParentRoomId;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["CreateTopologyChild"] = new JsonObject
                        {
                            ["parent_room_id"] = parentRoomId,
                            ["child_room_id"] = message.ChildRoomId,
                            ["child_purpose"] = message.ChildPurpose,
                            ["created_by"] = string.IsNullOrWhiteSpace(message.CreatedBy)
                                ? "dotnet-host"
                                : message.CreatedBy,
                            ["promotion_policy_id"] = message.PromotionPolicyId,
                            ["parent_checkpoint"] = message.ParentCheckpoint.DeepClone()
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "topology.describe-lineage", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "topology.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=topology.describe-lineage requires=topology.admin");
            }

            var targetRoom = string.IsNullOrWhiteSpace(message.TargetRoomId)
                ? state.RoomId
                : message.TargetRoomId;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["DescribeRoomLineage"] = new JsonObject
                        {
                            ["room_id"] = targetRoom
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "topology.list-children", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "topology.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=topology.list-children requires=topology.admin");
            }

            var parentRoomId = string.IsNullOrWhiteSpace(message.ParentRoomId)
                ? state.RoomId
                : message.ParentRoomId;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["ListTopologyChildren"] = new JsonObject
                        {
                            ["parent_room_id"] = parentRoomId
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "topology.propose-promotion", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "topology.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=topology.propose-promotion requires=topology.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ParentRoomId)
                || string.IsNullOrWhiteSpace(message.ChildRoomId)
                || string.IsNullOrWhiteSpace(message.ChildCheckpointHash)
                || string.IsNullOrWhiteSpace(message.PayloadRef))
            {
                return RuntimeMapResult.Failure("topology.propose-promotion requires parent_room_id, child_room_id, child_checkpoint_hash, payload_ref");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["ProposeTopologyPromotion"] = new JsonObject
                        {
                            ["parent_room_id"] = message.ParentRoomId,
                            ["child_room_id"] = message.ChildRoomId,
                            ["child_checkpoint_hash"] = message.ChildCheckpointHash,
                            ["payload_ref"] = message.PayloadRef,
                            ["idempotency_key"] = message.IdempotencyKey
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "topology.validate-promotion", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "topology.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=topology.validate-promotion requires=topology.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ProposalId))
            {
                return RuntimeMapResult.Failure("topology.validate-promotion.proposal_id is required");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["ValidateTopologyPromotion"] = new JsonObject
                        {
                            ["proposal_id"] = message.ProposalId
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "topology.apply-promotion", StringComparison.OrdinalIgnoreCase))
        {
            if (!IsControlPlaneAllowed(state, "topology.admin"))
            {
                return RuntimeMapResult.Failure("reject.control_plane_forbidden: command=topology.apply-promotion requires=topology.admin");
            }

            if (string.IsNullOrWhiteSpace(message.ProposalId))
            {
                return RuntimeMapResult.Failure("topology.apply-promotion.proposal_id is required");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId!,
                    new JsonObject
                    {
                        ["ApplyTopologyPromotion"] = new JsonObject
                        {
                            ["proposal_id"] = message.ProposalId
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "pack", StringComparison.OrdinalIgnoreCase))
        {
            if (string.IsNullOrWhiteSpace(message.NodesB64))
            {
                return RuntimeMapResult.Failure("pack.nodes is required");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ImportPack"] = new JsonObject
                        {
                            ["nodes_b64"] = message.NodesB64
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "request", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "request-server-pack", StringComparison.OrdinalIgnoreCase))
        {
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["RequestServerPack"] = new JsonObject
                        {
                            ["known_ids"] = new JsonArray((message.KnownIds ?? []).Select(x => (JsonNode?)x).ToArray())
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "mst-request", StringComparison.OrdinalIgnoreCase))
        {
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["MstRequest"] = new JsonObject
                        {
                            ["paths"] = new JsonArray((message.MstPaths ?? []).Select(x => (JsonNode?)x).ToArray())
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "mst-done", StringComparison.OrdinalIgnoreCase))
        {
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["MstDone"] = new JsonObject
                        {
                            ["ids"] = new JsonArray((message.MstIds ?? []).Select(x => (JsonNode?)x).ToArray())
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "recent-conflicts", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            JsonNode sinceNode = message.SinceUnixMs is ulong since
                ? JsonValue.Create(since)!
                : JsonValue.Create((ulong?)null)!;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["GetRecentConflicts"] = new JsonObject
                        {
                            ["since_unix_ms"] = sinceNode
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "webrtc-offer", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "webrtc-answer", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "webrtc-ice", StringComparison.OrdinalIgnoreCase))
        {
            if (string.IsNullOrWhiteSpace(message.ToPeerPubkey))
            {
                return RuntimeMapResult.Failure(type + ".to is required");
            }

            var payload = new JsonObject();
            if (message.Sdp is not null)
            {
                payload["sdp"] = message.Sdp.DeepClone();
            }
            if (message.Candidate is not null)
            {
                payload["candidate"] = message.Candidate.DeepClone();
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["RelayPeerSignal"] = new JsonObject
                        {
                            ["session_id"] = state.SessionId,
                            ["msg_type"] = type,
                            ["to_peer_pubkey"] = message.ToPeerPubkey,
                            ["payload"] = payload
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "map-set", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("map-set.key is required");
            }

            var namespaceValue = ResolveNamespace(message);
            var valueNode = message.Value?.DeepClone() ?? JsonValue.Create((string?)null)!;

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["MapSet"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["value"] = valueNode
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "map-get", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("map-get.key is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["MapGet"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "map-delete", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("map-delete.key is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["MapDelete"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "map-all", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["MapAll"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "text-insert", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("text-insert.key is required");
            }

            if (string.IsNullOrWhiteSpace(message.Ch))
            {
                return RuntimeMapResult.Failure("text-insert.ch is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["TextInsert"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["after_id"] = message.AfterId,
                            ["ch"] = message.Ch
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "text-delete", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("text-delete.key is required");
            }

            if (string.IsNullOrWhiteSpace(message.TargetId))
            {
                return RuntimeMapResult.Failure("text-delete.target_id is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["TextDelete"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["target_id"] = message.TargetId
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "text-get", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("text-get.key is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["TextGet"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "text-get-canonical", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("text-get-canonical.key is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["TextGetCanonical"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "list-push", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("list-push.key is required");
            }

            var namespaceValue = ResolveNamespace(message);
            var valueNode = message.Value?.DeepClone() ?? JsonValue.Create((string?)null)!;
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ListPush"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["value"] = valueNode
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "list-insert", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("list-insert.key is required");
            }

            if (message.Index is null)
            {
                return RuntimeMapResult.Failure("list-insert.index is required");
            }

            var namespaceValue = ResolveNamespace(message);
            var valueNode = message.Value?.DeepClone() ?? JsonValue.Create((string?)null)!;
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ListInsert"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["index"] = message.Index,
                            ["value"] = valueNode
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "list-delete", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("list-delete.key is required");
            }

            if (message.Index is null)
            {
                return RuntimeMapResult.Failure("list-delete.index is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ListDelete"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["index"] = message.Index
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "list-get", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("list-get.key is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ListGet"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "list-move", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("list-move.key is required");
            }

            if (message.FromIndex is null)
            {
                return RuntimeMapResult.Failure("list-move.from_index is required");
            }

            if (message.ToIndex is null)
            {
                return RuntimeMapResult.Failure("list-move.to_index is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ListMove"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["from_index"] = message.FromIndex,
                            ["to_index"] = message.ToIndex
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "list-update", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Key))
            {
                return RuntimeMapResult.Failure("list-update.key is required");
            }

            if (message.Index is null)
            {
                return RuntimeMapResult.Failure("list-update.index is required");
            }

            var namespaceValue = ResolveNamespace(message);
            var valueNode = message.Value?.DeepClone() ?? JsonValue.Create((string?)null)!;
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["ListUpdate"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["key"] = message.Key,
                            ["index"] = message.Index,
                            ["value"] = valueNode
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "blob-set", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Hash))
            {
                return RuntimeMapResult.Failure("blob-set.hash is required");
            }

            if (string.IsNullOrWhiteSpace(message.DataB64))
            {
                return RuntimeMapResult.Failure("blob-set.data_b64 is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["BlobSet"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["hash"] = message.Hash,
                            ["data_b64"] = message.DataB64
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "blob-upload", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (message.Blobs is null || message.Blobs.Length == 0)
            {
                return RuntimeMapResult.Failure("blob-upload.blobs is required");
            }

            var namespaceValue = ResolveNamespace(message);
            var commands = new List<string>(message.Blobs.Length);
            foreach (var blob in message.Blobs)
            {
                if (blob is null || string.IsNullOrWhiteSpace(blob.Hash))
                {
                    return RuntimeMapResult.Failure("blob-upload.blobs[].hash is required");
                }

                var dataB64 = blob.DataB64;
                if (string.IsNullOrWhiteSpace(dataB64))
                {
                    dataB64 = blob.Data;
                }

                if (string.IsNullOrWhiteSpace(dataB64))
                {
                    return RuntimeMapResult.Failure("blob-upload.blobs[].data is required");
                }

                commands.Add(
                    SerializeEnvelope(
                        state.RoomId,
                        new JsonObject
                        {
                            ["BlobSet"] = new JsonObject
                            {
                                ["namespace"] = namespaceValue,
                                ["hash"] = blob.Hash,
                                ["data_b64"] = dataB64
                            }
                        }
                    )
                );
            }

            return RuntimeMapResult.Success(commands);
        }

        if (string.Equals(type, "blob-uploaded", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Hash))
            {
                return RuntimeMapResult.Failure("blob-uploaded.hash is required");
            }

            return RuntimeMapResult.Success([]);
        }

        if (string.Equals(type, "blob-get", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Hash))
            {
                return RuntimeMapResult.Failure("blob-get.hash is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["BlobGet"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["hash"] = message.Hash
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "blob-get-many", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (message.Hashes is null || message.Hashes.Length == 0)
            {
                return RuntimeMapResult.Failure("blob-get-many.hashes is required");
            }

            var namespaceValue = ResolveNamespace(message);
            var hashes = new JsonArray(message.Hashes.Select(x => (JsonNode?)x).ToArray());
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["BlobGetMany"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["hashes"] = hashes
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "request-upload", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (string.IsNullOrWhiteSpace(message.Hash))
            {
                return RuntimeMapResult.Failure("request-upload.hash is required");
            }

            if (message.SizeBytes is null || message.SizeBytes == 0)
            {
                return RuntimeMapResult.Failure("request-upload.size is required");
            }

            var namespaceValue = ResolveNamespace(message);
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["RequestUpload"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["hash"] = message.Hash,
                            ["size_bytes"] = message.SizeBytes,
                            ["content_type"] = message.ContentType
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "blob-request", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (message.Hashes is null || message.Hashes.Length == 0)
            {
                return RuntimeMapResult.Failure("blob-request.hashes is required");
            }

            var namespaceValue = ResolveNamespace(message);
            var hashes = new JsonArray(message.Hashes.Select(x => (JsonNode?)x).ToArray());
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["BlobRequest"] = new JsonObject
                        {
                            ["namespace"] = namespaceValue,
                            ["hashes"] = hashes
                        }
                    }
                )
            ]);
        }

        if (
            string.Equals(type, "presence", StringComparison.OrdinalIgnoreCase)
            || string.Equals(type, "presence-set", StringComparison.OrdinalIgnoreCase)
        )
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["PresenceSet"] = new JsonObject
                        {
                            ["session_id"] = state.SessionId,
                            ["data"] = message.Data?.DeepClone() ?? JsonValue.Create((string?)null),
                            ["ttl_ms"] = message.TtlMs,
                            ["now_unix_ms"] = message.NowUnixMs
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "presence-get", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(state.RoomId, JsonValue.Create("PresenceGetAll")!)
            ]);
        }

        if (string.Equals(type, "presence-sweep", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            if (message.NowUnixMs is null)
            {
                return RuntimeMapResult.Failure("presence-sweep.now_unix_ms is required");
            }

            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["PresenceSweep"] = new JsonObject
                        {
                            ["now_unix_ms"] = message.NowUnixMs
                        }
                    }
                )
            ]);
        }

        if (string.Equals(type, "subscribe", StringComparison.OrdinalIgnoreCase))
        {
            if (!state.IsInitialized || string.IsNullOrWhiteSpace(state.RoomId))
            {
                return RuntimeMapResult.Failure("hello must be sent first");
            }

            var patterns = new JsonArray((message.Patterns ?? []).Select(x => (JsonNode?)x).ToArray());
            return RuntimeMapResult.Success([
                SerializeEnvelope(
                    state.RoomId,
                    new JsonObject
                    {
                        ["Subscribe"] = new JsonObject
                        {
                            ["session_id"] = state.SessionId,
                            ["patterns"] = patterns
                        }
                    }
                )
            ]);
        }

        return RuntimeMapResult.Failure("unsupported message type for PR7 step 9");
    }

    public RuntimeEventMapResult MapEventsJsonToOutboundMessages(string eventsJson)
    {
        JsonNode? root;
        try
        {
            root = JsonNode.Parse(eventsJson);
        }
        catch
        {
            return RuntimeEventMapResult.Failure("host event decode failed");
        }

        if (root is not JsonArray arr)
        {
            return RuntimeEventMapResult.Failure("host event payload must be an array");
        }

        var outbound = new List<string>();
        foreach (var item in arr)
        {
            if (item is null)
            {
                continue;
            }

            if (item is JsonValue value && value.TryGetValue<string>(out var unitVariant))
            {
                if (string.Equals(unitVariant, "NoopAck", StringComparison.Ordinal))
                {
                    outbound.Add(JsonSerializer.Serialize(new
                    {
                        type = "noop-ack"
                    }));
                }

                continue;
            }

            if (item is not JsonObject obj)
            {
                continue;
            }

            if (obj.TryGetPropertyValue("RoomEnsured", out var roomEnsuredNode) && roomEnsuredNode is JsonObject roomEnsured)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "room-ensured",
                    room = roomEnsured["room_id"]?.GetValue<string>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("SessionOpened", out var sessionOpenedNode) && sessionOpenedNode is JsonObject sessionOpened)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "session-opened",
                    room = sessionOpened["room_id"]?.GetValue<string>(),
                    session_id = sessionOpened["session_id"]?.GetValue<ulong>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("SessionClosed", out var sessionClosedNode) && sessionClosedNode is JsonObject sessionClosed)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "session-closed",
                    room = sessionClosed["room_id"]?.GetValue<string>(),
                    session_id = sessionClosed["session_id"]?.GetValue<ulong>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("RoomLocked", out var roomLockedNode) && roomLockedNode is JsonObject roomLocked)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "room-locked",
                    room = roomLocked["room_id"]?.GetValue<string>(),
                    pubkey = roomLocked["pubkey_hex"]?.GetValue<string>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("SetRoomKeyRejected", out var roomKeyRejectedNode) && roomKeyRejectedNode is JsonObject roomKeyRejected)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "set-room-key-rejected",
                    room = roomKeyRejected["room_id"]?.GetValue<string>(),
                    msg = roomKeyRejected["msg"]?.GetValue<string>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("PolicySet", out var policySetNode) && policySetNode is JsonObject policySet)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "policy-set",
                    room = policySet["room_id"]?.GetValue<string>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("SetPolicyRejected", out var policyRejectedNode) && policyRejectedNode is JsonObject policyRejected)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "set-policy-rejected",
                    room = policyRejected["room_id"]?.GetValue<string>(),
                    msg = policyRejected["msg"]?.GetValue<string>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("QuerySpecRegistered", out var queryRegisteredNode) && queryRegisteredNode is JsonObject queryRegistered)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "query.registered",
                    ["room"] = queryRegistered["room_id"]?.GetValue<string>(),
                    ["query_spec_id"] = queryRegistered["query_spec_id"]?.GetValue<string>(),
                    ["version"] = queryRegistered["version"]?.GetValue<string>(),
                    ["canonical_hash"] = queryRegistered["canonical_hash"]?.DeepClone(),
                    ["accepted"] = queryRegistered["accepted"]?.GetValue<bool>() ?? true
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("QuerySpecRejected", out var queryRejectedNode) && queryRejectedNode is JsonObject queryRejected)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "query.register.rejected",
                    ["room"] = queryRejected["room_id"]?.GetValue<string>(),
                    ["query_spec_id"] = queryRejected["query_spec_id"]?.GetValue<string>(),
                    ["version"] = queryRejected["version"]?.GetValue<string>(),
                    ["reason_class"] = queryRejected["reason_class"]?.GetValue<string>(),
                    ["reason_message"] = queryRejected["reason_message"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ProjectionBuildCompleted", out var projectionBuildCompletedNode) && projectionBuildCompletedNode is JsonObject projectionBuildCompleted)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "projection.build.completed",
                    ["room"] = projectionBuildCompleted["room_id"]?.GetValue<string>(),
                    ["projection_id"] = projectionBuildCompleted["projection_id"]?.GetValue<string>(),
                    ["checkpoint"] = projectionBuildCompleted["checkpoint"]?.DeepClone(),
                    ["digest"] = projectionBuildCompleted["digest"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ProjectionBuildRejected", out var projectionBuildRejectedNode) && projectionBuildRejectedNode is JsonObject projectionBuildRejected)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "projection.build.rejected",
                    ["room"] = projectionBuildRejected["room_id"]?.GetValue<string>(),
                    ["projection_id"] = projectionBuildRejected["projection_id"]?.GetValue<string>(),
                    ["reason_class"] = projectionBuildRejected["reason_class"]?.GetValue<string>(),
                    ["reason_message"] = projectionBuildRejected["reason_message"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ProjectionReadResult", out var projectionReadResultNode) && projectionReadResultNode is JsonObject projectionReadResult)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "projection.read.result",
                    ["room"] = projectionReadResult["room_id"]?.GetValue<string>(),
                    ["projection_id"] = projectionReadResult["projection_id"]?.GetValue<string>(),
                    ["checkpoint"] = projectionReadResult["checkpoint"]?.DeepClone(),
                    ["rows"] = projectionReadResult["rows"]?.DeepClone() ?? new JsonArray(),
                    ["digest"] = projectionReadResult["digest"]?.DeepClone(),
                    ["next_page_token"] = projectionReadResult["next_page_token"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ProjectionInvalidated", out var projectionInvalidatedNode) && projectionInvalidatedNode is JsonObject projectionInvalidated)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "projection.invalidated",
                    ["room"] = projectionInvalidated["room_id"]?.GetValue<string>(),
                    ["projection_id"] = projectionInvalidated["projection_id"]?.GetValue<string>(),
                    ["reason"] = projectionInvalidated["reason"]?.GetValue<string>(),
                    ["invalidated_at_hlc"] = projectionInvalidated["invalidated_at_hlc"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ProjectionListResult", out var projectionListResultNode) && projectionListResultNode is JsonObject projectionListResult)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "projection.list.result",
                    ["room"] = projectionListResult["room_id"]?.GetValue<string>(),
                    ["query_spec_id"] = projectionListResult["query_spec_id"]?.GetValue<string>(),
                    ["items"] = projectionListResult["items"]?.DeepClone() ?? new JsonArray(),
                    ["cursor"] = projectionListResult["cursor"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ArchiveDescribed", out var archiveDescribedNode) && archiveDescribedNode is JsonObject archiveDescribed)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "archive.describe.result",
                    ["room"] = archiveDescribed["room_id"]?.GetValue<string>(),
                    ["archive_ref"] = archiveDescribed["archive_ref"]?.GetValue<string>(),
                    ["manifest_id"] = archiveDescribed["manifest_id"]?.GetValue<string>(),
                    ["format_version"] = archiveDescribed["format_version"]?.GetValue<string>(),
                    ["archive_kind"] = archiveDescribed["archive_kind"]?.GetValue<string>(),
                    ["checkpoint"] = archiveDescribed["checkpoint"]?.DeepClone(),
                    ["payload_digest_set"] = archiveDescribed["payload_digest_set"]?.DeepClone(),
                    ["compatibility_window"] = archiveDescribed["compatibility_window"]?.DeepClone(),
                    ["provenance"] = archiveDescribed["provenance"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ArchiveValidated", out var archiveValidatedNode) && archiveValidatedNode is JsonObject archiveValidated)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "archive.validate.result",
                    ["room"] = archiveValidated["room_id"]?.GetValue<string>(),
                    ["archive_ref"] = archiveValidated["archive_ref"]?.GetValue<string>(),
                    ["accepted"] = archiveValidated["accepted"]?.GetValue<bool>() ?? false,
                    ["mode"] = archiveValidated["mode"]?.GetValue<string>(),
                    ["checks"] = archiveValidated["checks"]?.DeepClone() ?? new JsonArray(),
                    ["compatibility_window"] = archiveValidated["compatibility_window"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ArchiveValidationRejected", out var archiveValidationRejectedNode) && archiveValidationRejectedNode is JsonObject archiveValidationRejected)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "archive.validate.rejected",
                    ["room"] = archiveValidationRejected["room_id"]?.GetValue<string>(),
                    ["archive_ref"] = archiveValidationRejected["archive_ref"]?.GetValue<string>(),
                    ["reason_class"] = archiveValidationRejected["reason_class"]?.GetValue<string>(),
                    ["reason_message"] = archiveValidationRejected["reason_message"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ArchiveImported", out var archiveImportedNode) && archiveImportedNode is JsonObject archiveImported)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "archive.import.completed",
                    ["room"] = archiveImported["room_id"]?.GetValue<string>(),
                    ["archive_ref"] = archiveImported["archive_ref"]?.GetValue<string>(),
                    ["canonical_hash"] = archiveImported["canonical_hash"]?.GetValue<string>(),
                    ["checkpoint"] = archiveImported["checkpoint"]?.DeepClone(),
                    ["imported_nodes"] = archiveImported["imported_nodes"]?.GetValue<int>() ?? 0,
                    ["imported_blobs"] = archiveImported["imported_blobs"]?.GetValue<int>() ?? 0
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ArchiveImportRejected", out var archiveImportRejectedNode) && archiveImportRejectedNode is JsonObject archiveImportRejected)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "archive.import.rejected",
                    ["room"] = archiveImportRejected["room_id"]?.GetValue<string>(),
                    ["archive_ref"] = archiveImportRejected["archive_ref"]?.GetValue<string>(),
                    ["reason_class"] = archiveImportRejected["reason_class"]?.GetValue<string>(),
                    ["reason_message"] = archiveImportRejected["reason_message"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ChildRoomCreated", out var childRoomCreatedNode) && childRoomCreatedNode is JsonObject childRoomCreated)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "topology.create-child.completed",
                    ["child_room_id"] = childRoomCreated["child_room_id"]?.GetValue<string>(),
                    ["lineage"] = childRoomCreated["lineage"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("RoomLineageDescribed", out var roomLineageDescribedNode) && roomLineageDescribedNode is JsonObject roomLineageDescribed)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "topology.describe-lineage.result",
                    ["room_id"] = roomLineageDescribed["room_id"]?.GetValue<string>(),
                    ["lineage"] = roomLineageDescribed["lineage"]?.DeepClone(),
                    ["ancestors"] = roomLineageDescribed["ancestors"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ChildrenListed", out var childrenListedNode) && childrenListedNode is JsonObject childrenListed)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "topology.list-children.result",
                    ["parent_room_id"] = childrenListed["parent_room_id"]?.GetValue<string>(),
                    ["children"] = childrenListed["children"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("PromotionProposed", out var promotionProposedNode) && promotionProposedNode is JsonObject promotionProposed)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "topology.propose-promotion.completed",
                    ["proposal_id"] = promotionProposed["proposal_id"]?.GetValue<string>(),
                    ["parent_room_id"] = promotionProposed["parent_room_id"]?.GetValue<string>(),
                    ["child_room_id"] = promotionProposed["child_room_id"]?.GetValue<string>(),
                    ["child_checkpoint_hash"] = promotionProposed["child_checkpoint_hash"]?.GetValue<string>(),
                    ["payload_ref"] = promotionProposed["payload_ref"]?.GetValue<string>(),
                    ["proposal_digest"] = promotionProposed["proposal_digest"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("PromotionValidated", out var promotionValidatedNode) && promotionValidatedNode is JsonObject promotionValidated)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "topology.validate-promotion.completed",
                    ["proposal_id"] = promotionValidated["proposal_id"]?.GetValue<string>(),
                    ["validation_digest"] = promotionValidated["validation_digest"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("PromotionApplied", out var promotionAppliedNode) && promotionAppliedNode is JsonObject promotionApplied)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "topology.apply-promotion.completed",
                    ["proposal_id"] = promotionApplied["proposal_id"]?.GetValue<string>(),
                    ["parent_room_id"] = promotionApplied["parent_room_id"]?.GetValue<string>(),
                    ["parent_new_canonical_hash"] = promotionApplied["parent_new_canonical_hash"]?.GetValue<string>(),
                    ["audit_key"] = promotionApplied["audit_key"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("WelcomePrepared", out var welcomeNode) && welcomeNode is JsonObject welcome)
            {
                var negotiated = welcome["negotiated"] as JsonObject;
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "welcome",
                    room = welcome["room_id"]?.GetValue<string>(),
                    session_id = welcome["session_id"]?.GetValue<ulong>(),
                    caps = new
                    {
                        supports_ibf = negotiated?["supports_ibf"]?.GetValue<bool>() ?? true,
                        supports_mst = negotiated?["supports_mst"]?.GetValue<bool>() ?? true
                    },
                    missing_from_server_count = welcome["missing_from_server_count"]?.GetValue<int>() ?? 0
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("PackImported", out var packImportedNode) && packImportedNode is JsonObject packImported)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "pack-ack",
                    room = packImported["room_id"]?.GetValue<string>(),
                    incoming_count = packImported["incoming_count"]?.GetValue<int>() ?? 0,
                    accepted_count = packImported["accepted_count"]?.GetValue<int>() ?? 0,
                    rejected_count = packImported["rejected_count"]?.GetValue<int>() ?? 0
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("ServerPackPrepared", out var serverPackNode) && serverPackNode is JsonObject serverPack)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "pack",
                    room = serverPack["room_id"]?.GetValue<string>(),
                    from = "server",
                    nodes = serverPack["nodes_b64"]?.GetValue<string>(),
                    root = serverPack["root_hex"]?.GetValue<string>()
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("MstResponsePrepared", out var mstResponseNode) && mstResponseNode is JsonObject mstResponse)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "mst-response",
                    ["room"] = mstResponse["room_id"]?.GetValue<string>(),
                    ["nodes"] = mstResponse["nodes"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ConflictsObserved", out var conflictsObservedNode) && conflictsObservedNode is JsonObject conflictsObserved)
            {
                var room = conflictsObserved["room_id"]?.GetValue<string>();
                var entries = conflictsObserved["entries"] as JsonArray;
                if (entries is not null)
                {
                    foreach (var entryNode in entries)
                    {
                        if (entryNode is not JsonObject entry)
                        {
                            continue;
                        }

                        outbound.Add(new JsonObject
                        {
                            ["type"] = "conflict",
                            ["room"] = room,
                            ["at_unix_ms"] = entry["at_unix_ms"]?.DeepClone(),
                            ["event"] = entry["event"]?.DeepClone()
                        }.ToJsonString());
                    }
                }
                continue;
            }

            if (obj.TryGetPropertyValue("RecentConflictsListed", out var recentConflictsNode) && recentConflictsNode is JsonObject recentConflicts)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "recent-conflicts",
                    ["room"] = recentConflicts["room_id"]?.GetValue<string>(),
                    ["entries"] = recentConflicts["entries"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("PeerSignalRelayed", out var relayedNode) && relayedNode is JsonObject relayed)
            {
                var msgType = relayed["msg_type"]?.GetValue<string>();
                if (!string.IsNullOrWhiteSpace(msgType))
                {
                    var wire = new JsonObject
                    {
                        ["type"] = msgType,
                        ["room"] = relayed["room_id"]?.GetValue<string>(),
                        ["from"] = relayed["from_peer_pubkey"]?.GetValue<string>(),
                        ["to"] = relayed["to_peer_pubkey"]?.GetValue<string>()
                    };

                    if (relayed["payload"] is JsonObject relayPayload)
                    {
                        if (relayPayload.TryGetPropertyValue("sdp", out var sdpNode) && sdpNode is not null)
                        {
                            wire["sdp"] = sdpNode.DeepClone();
                        }
                        if (relayPayload.TryGetPropertyValue("candidate", out var candidateNode) && candidateNode is not null)
                        {
                            wire["candidate"] = candidateNode.DeepClone();
                        }
                    }

                    outbound.Add(wire.ToJsonString());
                }
                continue;
            }

            if (obj.TryGetPropertyValue("MapValueUpserted", out var mapSetNode) && mapSetNode is JsonObject mapSet)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "map-set-ack",
                    ["room"] = mapSet["room_id"]?.GetValue<string>(),
                    ["namespace"] = mapSet["namespace"]?.GetValue<string>(),
                    ["key"] = mapSet["key"]?.GetValue<string>(),
                    ["value"] = mapSet["value"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("MapValueDeleted", out var mapDeleteNode) && mapDeleteNode is JsonObject mapDelete)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "map-delete-ack",
                    room = mapDelete["room_id"]?.GetValue<string>(),
                    @namespace = mapDelete["namespace"]?.GetValue<string>(),
                    key = mapDelete["key"]?.GetValue<string>(),
                    found = mapDelete["found"]?.GetValue<bool>() ?? false
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("MapValueRead", out var mapReadNode) && mapReadNode is JsonObject mapRead)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "map-value",
                    ["room"] = mapRead["room_id"]?.GetValue<string>(),
                    ["namespace"] = mapRead["namespace"]?.GetValue<string>(),
                    ["key"] = mapRead["key"]?.GetValue<string>(),
                    ["found"] = mapRead["value"] is not null,
                    ["value"] = mapRead["value"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("MapEntriesListed", out var mapAllNode) && mapAllNode is JsonObject mapAll)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "map-all",
                    ["room"] = mapAll["room_id"]?.GetValue<string>(),
                    ["namespace"] = mapAll["namespace"]?.GetValue<string>(),
                    ["entries"] = mapAll["entries"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("TextValueInserted", out var textInsertNode) && textInsertNode is JsonObject textInsert)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "text-insert-ack",
                    ["room"] = textInsert["room_id"]?.GetValue<string>(),
                    ["namespace"] = textInsert["namespace"]?.GetValue<string>(),
                    ["key"] = textInsert["key"]?.GetValue<string>(),
                    ["id"] = textInsert["id"]?.GetValue<string>(),
                    ["ch"] = textInsert["ch"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("TextValueDeleted", out var textDeleteNode) && textDeleteNode is JsonObject textDelete)
            {
                outbound.Add(JsonSerializer.Serialize(new
                {
                    type = "text-delete-ack",
                    room = textDelete["room_id"]?.GetValue<string>(),
                    @namespace = textDelete["namespace"]?.GetValue<string>(),
                    key = textDelete["key"]?.GetValue<string>(),
                    target_id = textDelete["target_id"]?.GetValue<string>(),
                    found = textDelete["found"]?.GetValue<bool>() ?? false
                }));
                continue;
            }

            if (obj.TryGetPropertyValue("TextValueRead", out var textReadNode) && textReadNode is JsonObject textRead)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "text-value",
                    ["room"] = textRead["room_id"]?.GetValue<string>(),
                    ["namespace"] = textRead["namespace"]?.GetValue<string>(),
                    ["key"] = textRead["key"]?.GetValue<string>(),
                    ["value"] = textRead["value"]?.GetValue<string>() ?? string.Empty,
                    ["entries"] = textRead["entries"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ListValuePushed", out var listPushNode) && listPushNode is JsonObject listPush)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "list-push-ack",
                    ["room"] = listPush["room_id"]?.GetValue<string>(),
                    ["namespace"] = listPush["namespace"]?.GetValue<string>(),
                    ["key"] = listPush["key"]?.GetValue<string>(),
                    ["id"] = listPush["id"]?.GetValue<string>(),
                    ["index"] = listPush["index"]?.GetValue<ulong>(),
                    ["value"] = listPush["value"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ListValueInserted", out var listInsertNode) && listInsertNode is JsonObject listInsert)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "list-insert-ack",
                    ["room"] = listInsert["room_id"]?.GetValue<string>(),
                    ["namespace"] = listInsert["namespace"]?.GetValue<string>(),
                    ["key"] = listInsert["key"]?.GetValue<string>(),
                    ["id"] = listInsert["id"]?.GetValue<string>(),
                    ["index"] = listInsert["index"]?.GetValue<ulong>(),
                    ["value"] = listInsert["value"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ListValueDeleted", out var listDeleteNode) && listDeleteNode is JsonObject listDelete)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "list-delete-ack",
                    ["room"] = listDelete["room_id"]?.GetValue<string>(),
                    ["namespace"] = listDelete["namespace"]?.GetValue<string>(),
                    ["key"] = listDelete["key"]?.GetValue<string>(),
                    ["index"] = listDelete["index"]?.GetValue<ulong>(),
                    ["found"] = listDelete["found"]?.GetValue<bool>() ?? false,
                    ["removed"] = listDelete["removed"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ListValueRead", out var listReadNode) && listReadNode is JsonObject listRead)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "list-value",
                    ["room"] = listRead["room_id"]?.GetValue<string>(),
                    ["namespace"] = listRead["namespace"]?.GetValue<string>(),
                    ["key"] = listRead["key"]?.GetValue<string>(),
                    ["entries"] = listRead["entries"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ListValueMoved", out var listMoveNode) && listMoveNode is JsonObject listMove)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "list-move-ack",
                    ["room"] = listMove["room_id"]?.GetValue<string>(),
                    ["namespace"] = listMove["namespace"]?.GetValue<string>(),
                    ["key"] = listMove["key"]?.GetValue<string>(),
                    ["from_index"] = listMove["from_index"]?.GetValue<ulong>(),
                    ["to_index"] = listMove["to_index"]?.GetValue<ulong>(),
                    ["found"] = listMove["found"]?.GetValue<bool>() ?? false,
                    ["id"] = listMove["id"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("ListValueUpdated", out var listUpdateNode) && listUpdateNode is JsonObject listUpdate)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "list-update-ack",
                    ["room"] = listUpdate["room_id"]?.GetValue<string>(),
                    ["namespace"] = listUpdate["namespace"]?.GetValue<string>(),
                    ["key"] = listUpdate["key"]?.GetValue<string>(),
                    ["index"] = listUpdate["index"]?.GetValue<ulong>(),
                    ["found"] = listUpdate["found"]?.GetValue<bool>() ?? false,
                    ["id"] = listUpdate["id"]?.DeepClone(),
                    ["value"] = listUpdate["value"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("BlobValueStored", out var blobSetNode) && blobSetNode is JsonObject blobSet)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "blob-set-ack",
                    ["room"] = blobSet["room_id"]?.GetValue<string>(),
                    ["namespace"] = blobSet["namespace"]?.GetValue<string>(),
                    ["hash"] = blobSet["hash"]?.GetValue<string>(),
                    ["stored"] = blobSet["stored"]?.GetValue<bool>() ?? false
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("BlobValueRead", out var blobReadNode) && blobReadNode is JsonObject blobRead)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "blob-value",
                    ["room"] = blobRead["room_id"]?.GetValue<string>(),
                    ["namespace"] = blobRead["namespace"]?.GetValue<string>(),
                    ["hash"] = blobRead["hash"]?.GetValue<string>(),
                    ["found"] = blobRead["found"]?.GetValue<bool>() ?? false,
                    ["data_b64"] = blobRead["data_b64"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("BlobValuesRead", out var blobsReadNode) && blobsReadNode is JsonObject blobsRead)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "blob-values",
                    ["room"] = blobsRead["room_id"]?.GetValue<string>(),
                    ["namespace"] = blobsRead["namespace"]?.GetValue<string>(),
                    ["entries"] = blobsRead["entries"]?.DeepClone() ?? new JsonArray(),
                    ["missing"] = blobsRead["missing"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("UploadGranted", out var uploadGrantedNode) && uploadGrantedNode is JsonObject uploadGranted)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "upload-granted",
                    ["room"] = uploadGranted["room_id"]?.GetValue<string>(),
                    ["namespace"] = uploadGranted["namespace"]?.GetValue<string>(),
                    ["hash"] = uploadGranted["hash"]?.GetValue<string>(),
                    ["url"] = uploadGranted["url"]?.GetValue<string>(),
                    ["expires_at_unix"] = uploadGranted["expires_at_unix"]?.GetValue<ulong>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("UploadDenied", out var uploadDeniedNode) && uploadDeniedNode is JsonObject uploadDenied)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "upload-denied",
                    ["room"] = uploadDenied["room_id"]?.GetValue<string>(),
                    ["namespace"] = uploadDenied["namespace"]?.GetValue<string>(),
                    ["hash"] = uploadDenied["hash"]?.GetValue<string>(),
                    ["reason"] = uploadDenied["reason"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("BlobRedirectPrepared", out var blobRedirectNode) && blobRedirectNode is JsonObject blobRedirect)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "blob-redirect",
                    ["room"] = blobRedirect["room_id"]?.GetValue<string>(),
                    ["namespace"] = blobRedirect["namespace"]?.GetValue<string>(),
                    ["redirects"] = blobRedirect["redirects"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("BlobPackPrepared", out var blobPackNode) && blobPackNode is JsonObject blobPack)
            {
                var blobs = new JsonArray();
                if (blobPack["blobs"] is JsonArray sourceBlobs)
                {
                    foreach (var entryNode in sourceBlobs)
                    {
                        if (entryNode is not JsonObject entry)
                        {
                            continue;
                        }

                        var hash = entry["hash"]?.GetValue<string>();
                        var data = entry["data"]?.GetValue<string>() ?? entry["data_b64"]?.GetValue<string>();
                        if (string.IsNullOrWhiteSpace(hash) || string.IsNullOrWhiteSpace(data))
                        {
                            continue;
                        }

                        blobs.Add(new JsonObject
                        {
                            ["hash"] = hash,
                            ["data"] = data
                        });
                    }
                }

                outbound.Add(new JsonObject
                {
                    ["type"] = "blob-pack",
                    ["room"] = blobPack["room_id"]?.GetValue<string>(),
                    ["namespace"] = blobPack["namespace"]?.GetValue<string>(),
                    ["blobs"] = blobs,
                    ["requested"] = blobPack["requested"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("PresenceValueSet", out var presenceSetNode) && presenceSetNode is JsonObject presenceSet)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "presence",
                    ["room"] = presenceSet["room_id"]?.GetValue<string>(),
                    ["session_id"] = presenceSet["session_id"]?.GetValue<ulong>(),
                    ["from"] = presenceSet["from_peer_pubkey"]?.GetValue<string>(),
                    ["joined"] = presenceSet["joined"]?.GetValue<bool>() ?? false,
                    ["data"] = presenceSet["data"]?.DeepClone()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("PresenceValueRemoved", out var presenceRemovedNode) && presenceRemovedNode is JsonObject presenceRemoved)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "presence-leave",
                    ["room"] = presenceRemoved["room_id"]?.GetValue<string>(),
                    ["session_id"] = presenceRemoved["session_id"]?.GetValue<ulong>(),
                    ["from"] = presenceRemoved["from_peer_pubkey"]?.GetValue<string>(),
                    ["reason"] = presenceRemoved["reason"]?.GetValue<string>()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("PresenceValuesListed", out var presenceListNode) && presenceListNode is JsonObject presenceList)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "presence-snapshot",
                    ["room"] = presenceList["room_id"]?.GetValue<string>(),
                    ["entries"] = presenceList["entries"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
                continue;
            }

            if (obj.TryGetPropertyValue("SubscriptionUpdated", out var subUpdatedNode) && subUpdatedNode is JsonObject subUpdated)
            {
                outbound.Add(new JsonObject
                {
                    ["type"] = "subscribe-ack",
                    ["room"] = subUpdated["room_id"]?.GetValue<string>(),
                    ["session_id"] = subUpdated["session_id"]?.GetValue<ulong>(),
                    ["patterns"] = subUpdated["patterns"]?.DeepClone() ?? new JsonArray()
                }.ToJsonString());
            }
        }

        return RuntimeEventMapResult.Success(outbound);
    }

    private static string SerializeEnvelope(string roomId, JsonNode commandNode)
    {
        var envelope = new JsonObject
        {
            ["room_id"] = roomId,
            ["command"] = commandNode
        };

        return envelope.ToJsonString();
    }

    private static string? ResolveRoomId(RuntimeInboundMessage message, RuntimeConnectionState state)
    {
        if (!string.IsNullOrWhiteSpace(message.Room))
        {
            return message.Room;
        }

        return state.RoomId;
    }

    private static string? ResolvePeerPubkey(RuntimeInboundMessage message, RuntimeConnectionState state)
    {
        if (!string.IsNullOrWhiteSpace(message.Pubkey))
        {
            return message.Pubkey;
        }

        return state.PeerPubkeyHex;
    }

    private static ulong ResolveSessionId(RuntimeInboundMessage message, RuntimeConnectionState state)
    {
        return message.SessionId ?? state.SessionId;
    }

    private static string ResolveNamespace(RuntimeInboundMessage message)
    {
        return message.Namespace ?? string.Empty;
    }

    private static bool HasExplicitRoom(RuntimeInboundMessage message)
    {
        return !string.IsNullOrWhiteSpace(message.Room);
    }

    private static bool HasExplicitPubkey(RuntimeInboundMessage message)
    {
        return !string.IsNullOrWhiteSpace(message.Pubkey);
    }

    private static bool IsControlPlaneAllowed(RuntimeConnectionState state, string requiredCapability)
    {
        return state.IsServerPeer || state.SessionCapabilities.Contains(requiredCapability);
    }

    private void UpdateServerPeerStatus(RuntimeConnectionState state, string? peerPubkeyHex)
    {
        if (string.IsNullOrWhiteSpace(_trustedServerPeerPubkeyHex))
        {
            return;
        }

        state.IsServerPeer = string.Equals(
            peerPubkeyHex?.Trim(),
            _trustedServerPeerPubkeyHex,
            StringComparison.OrdinalIgnoreCase);
    }

    private string? TrySetSessionCapabilities(
        RuntimeConnectionState state,
        string[]? capabilities,
        string? capabilityProfileVersion)
    {
        if (!_capabilityProfileExpander.IsEnabled)
        {
            SetSessionCapabilities(state, capabilities);
            return null;
        }

        var assigned = (IReadOnlyList<string>)(capabilities ?? []);
        if (!_capabilityProfileExpander.TryExpand(
                assigned,
                capabilityProfileVersion,
                out var expanded,
                out var error))
        {
            return error ?? "capability expansion failed";
        }

        SetSessionCapabilities(state, expanded.ToArray());
        return null;
    }

    private static void SetSessionCapabilities(RuntimeConnectionState state, string[]? capabilities)
    {
        state.SessionCapabilities.Clear();
        if (capabilities is null)
        {
            return;
        }

        foreach (var capability in capabilities)
        {
            if (!string.IsNullOrWhiteSpace(capability))
            {
                state.SessionCapabilities.Add(capability.Trim());
            }
        }
    }

    private static string? ValidateToken(RuntimeInboundToken token)
    {
        if (string.IsNullOrWhiteSpace(token.GetPeerPubkey()))
        {
            return "hello.token.peer_pubkey is required";
        }

        if (token.GetExpiry() is null)
        {
            return "hello.token.expiry is required";
        }

        if (string.IsNullOrWhiteSpace(token.GetSignature()))
        {
            return "hello.token.sig is required";
        }

        var continuity = token.Continuity;
        if (continuity is null)
        {
            return null;
        }

        var predecessor = continuity.GetPredecessorPeerPubkey();
        if (string.IsNullOrWhiteSpace(predecessor))
        {
            return "hello.token.continuity.predecessor_peer_pubkey is required";
        }

        var overlapNotAfter = continuity.GetOverlapNotAfter();
        if (overlapNotAfter is null)
        {
            return "hello.token.continuity.overlap_not_after is required";
        }

        var currentPeer = token.GetPeerPubkey();
        if (string.Equals(predecessor.Trim(), currentPeer?.Trim(), StringComparison.OrdinalIgnoreCase))
        {
            return "hello.token.continuity.predecessor_peer_pubkey must differ from hello.token.peer_pubkey";
        }

        var nowSecs = (ulong)Math.Max(0, DateTimeOffset.UtcNow.ToUnixTimeSeconds());
        if (nowSecs > overlapNotAfter.Value)
        {
            return "hello.token.continuity overlap window expired";
        }

        var revoked = continuity.GetRevokedPredecessors();
        if (revoked is not null)
        {
            foreach (var revokedPredecessor in revoked)
            {
                if (string.Equals(revokedPredecessor?.Trim(), predecessor.Trim(), StringComparison.OrdinalIgnoreCase))
                {
                    return "hello.token.continuity predecessor key is revoked";
                }
            }
        }

        return null;
    }

    private static JsonObject BuildTokenJson(RuntimeInboundToken token)
    {
        var json = new JsonObject
        {
            ["peer_pubkey"] = token.GetPeerPubkey(),
            ["expiry"] = token.GetExpiry(),
            ["caps"] = new JsonArray((token.GetCapabilities() ?? []).Select(x => (JsonNode?)x).ToArray()),
            ["sig"] = token.GetSignature()
        };

        var profileVersion = token.GetCapabilityProfileVersion();
        if (!string.IsNullOrWhiteSpace(profileVersion))
        {
            json["capability_profile_version"] = profileVersion;
        }

        var continuity = token.Continuity;
        if (continuity is not null)
        {
            var continuityJson = new JsonObject
            {
                ["predecessor_peer_pubkey"] = continuity.GetPredecessorPeerPubkey(),
                ["overlap_not_after"] = continuity.GetOverlapNotAfter()
            };

            var revoked = continuity.GetRevokedPredecessors();
            if (revoked is not null)
            {
                continuityJson["revoked_predecessors"] = new JsonArray(revoked.Select(x => (JsonNode?)x).ToArray());
            }

            json["continuity"] = continuityJson;
        }

        return json;
    }
}

public sealed class RuntimeConnectionState
{
    public RuntimeConnectionState(ulong sessionId)
    {
        SessionId = sessionId;
        TraceId = $"sess-{sessionId}-{Guid.NewGuid():N}";
        CanonicalMapRows["world/a"] = JsonValue.Create("1");
        CanonicalMapRows["world/b"] = JsonValue.Create("2");
        CanonicalMapRows["world/c"] = JsonValue.Create("3");
        CaptureCanonicalSnapshot(0);
    }

    public ulong SessionId { get; set; }
    public bool IsInitialized { get; set; }
    public string? RoomId { get; set; }
    public string? PeerPubkeyHex { get; set; }
    public string? TraceId { get; set; }
    public bool IsServerPeer { get; set; }
    public HashSet<string> SessionCapabilities { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, QuerySpecStubState> QuerySpecStubs { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, ProjectionStubState> ProjectionStubs { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, List<TopologyChildStubState>> TopologyChildrenByParent { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, TopologyPromotionStubState> TopologyPromotions { get; } = new(StringComparer.Ordinal);
    public Dictionary<string, JsonNode?> CanonicalMapRows { get; } = new(StringComparer.Ordinal);
    public ulong CanonicalSequence { get; private set; }
    public Dictionary<ulong, CanonicalSnapshotState> CanonicalSnapshots { get; } = new();
    private Dictionary<string, ulong> CanonicalSnapshotSequenceByHash { get; } = new(StringComparer.Ordinal);

    public void AdvanceCanonicalSequenceAndSnapshot()
    {
        CanonicalSequence++;
        CaptureCanonicalSnapshot(CanonicalSequence);
    }

    public bool TryResolveCanonicalSnapshotAtCheckpoint(
        JsonNode? checkpoint,
        out CanonicalSnapshotState snapshot,
        out string rejectReasonClass,
        out string rejectReasonMessage)
    {
        rejectReasonClass = string.Empty;
        rejectReasonMessage = string.Empty;
        snapshot = CloneSnapshot(CanonicalSnapshots[CanonicalSequence]);

        if (checkpoint is null)
        {
            return true;
        }

        if (checkpoint is not JsonObject checkpointObj)
        {
            rejectReasonClass = "reject.checkpoint_selector_invalid";
            rejectReasonMessage = "target_checkpoint must be an object";
            return false;
        }

        var selector = checkpointObj["selector"]?.GetValue<string>() ?? "latest";
        if (!ValidateSelectorPayloadShape(checkpointObj, selector, out rejectReasonMessage))
        {
            rejectReasonClass = "reject.checkpoint_selector_invalid";
            return false;
        }

        if (string.Equals(selector, "latest", StringComparison.OrdinalIgnoreCase))
        {
            return true;
        }

        if (string.Equals(selector, "seq", StringComparison.OrdinalIgnoreCase)
            && TryReadCheckpointSequence(checkpointObj, out var requestedSequence)
            && CanonicalSnapshots.TryGetValue(requestedSequence, out var seqSnapshot))
        {
            snapshot = CloneSnapshot(seqSnapshot);
            return true;
        }

        if (string.Equals(selector, "hash", StringComparison.OrdinalIgnoreCase)
            && TryReadCheckpointCanonicalHash(checkpointObj, out var requestedHash)
            && CanonicalSnapshotSequenceByHash.TryGetValue(requestedHash, out var seqByHash)
            && CanonicalSnapshots.TryGetValue(seqByHash, out var hashSnapshot))
        {
            snapshot = CloneSnapshot(hashSnapshot);
            return true;
        }

        if (string.Equals(selector, "frontier", StringComparison.OrdinalIgnoreCase)
            && TryReadCheckpointFrontierSequence(checkpointObj, out var frontierSequence)
            && CanonicalSnapshots.TryGetValue(frontierSequence, out var frontierSnapshot))
        {
            snapshot = CloneSnapshot(frontierSnapshot);
            return true;
        }

        rejectReasonClass = "reject.checkpoint_not_found";
        rejectReasonMessage = "target_checkpoint does not resolve to known canonical snapshot";
        return false;
    }

    private static bool TryReadCheckpointSequence(JsonObject checkpointObj, out ulong sequence)
    {
        sequence = 0;

        if (checkpointObj["canonical_seq"] is JsonValue seqNode)
        {
            try
            {
                sequence = seqNode.GetValue<ulong>();
                return true;
            }
            catch
            {
                return false;
            }
        }

        return false;
    }

    private static bool TryReadCheckpointCanonicalHash(JsonObject checkpointObj, out string canonicalHash)
    {
        canonicalHash = string.Empty;
        var raw = checkpointObj["canonical_hash"]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(raw))
        {
            return false;
        }

        if (raw.Length != 64 || raw.Any(ch => !Uri.IsHexDigit(ch)))
        {
            return false;
        }

        canonicalHash = raw.ToLowerInvariant();
        return true;
    }

    private static bool TryReadCheckpointFrontierSequence(JsonObject checkpointObj, out ulong sequence)
    {
        sequence = 0;
        if (checkpointObj["frontier"] is not JsonArray frontier || frontier.Count == 0)
        {
            return false;
        }

        if (frontier.Count != 1)
        {
            return false;
        }

        var token = frontier[0]?.GetValue<string>();
        if (string.IsNullOrWhiteSpace(token))
        {
            return false;
        }

        const string prefix = "seq:";
        if (!token.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
        {
            return false;
        }

        return ulong.TryParse(token[prefix.Length..], out sequence);
    }

    private Dictionary<string, JsonNode?> CloneCanonicalMapRows()
    {
        return CanonicalMapRows.ToDictionary(
            kvp => kvp.Key,
            kvp => kvp.Value?.DeepClone(),
            StringComparer.Ordinal
        );
    }

    private static bool ValidateSelectorPayloadShape(
        JsonObject checkpointObj,
        string selector,
        out string rejectReasonMessage)
    {
        rejectReasonMessage = string.Empty;
        var hasSeq = checkpointObj["canonical_seq"] is not null;
        var hasHash = checkpointObj["canonical_hash"] is not null;
        var hasFrontier = checkpointObj["frontier"] is not null;

        if (string.Equals(selector, "latest", StringComparison.OrdinalIgnoreCase))
        {
            if (hasSeq || hasHash || hasFrontier)
            {
                rejectReasonMessage = "selector latest cannot include canonical_seq, canonical_hash, or frontier";
                return false;
            }

            return true;
        }

        if (string.Equals(selector, "seq", StringComparison.OrdinalIgnoreCase))
        {
            if (!hasSeq || hasHash || hasFrontier)
            {
                rejectReasonMessage = "selector seq requires canonical_seq and forbids canonical_hash/frontier";
                return false;
            }

            return true;
        }

        if (string.Equals(selector, "hash", StringComparison.OrdinalIgnoreCase))
        {
            if (!hasHash || hasSeq || hasFrontier)
            {
                rejectReasonMessage = "selector hash requires canonical_hash and forbids canonical_seq/frontier";
                return false;
            }

            if (!TryReadCheckpointCanonicalHash(checkpointObj, out _))
            {
                rejectReasonMessage = "selector hash requires canonical_hash in 64-char hex format";
                return false;
            }

            return true;
        }

        if (string.Equals(selector, "frontier", StringComparison.OrdinalIgnoreCase))
        {
            if (!hasFrontier || hasSeq || hasHash)
            {
                rejectReasonMessage = "selector frontier requires frontier and forbids canonical_seq/canonical_hash";
                return false;
            }

            if (!TryReadCheckpointFrontierSequence(checkpointObj, out _))
            {
                rejectReasonMessage = "selector frontier requires frontier token format seq:<u64>";
                return false;
            }

            return true;
        }

        rejectReasonMessage = "selector must be one of latest|seq|hash|frontier";
        return false;
    }

    private void CaptureCanonicalSnapshot(ulong sequence)
    {
        var rows = CloneCanonicalMapRows();
        var canonicalHash = ComputeCanonicalHash(rows);
        var snapshot = new CanonicalSnapshotState(
            sequence,
            rows,
            canonicalHash,
            new JsonArray($"seq:{sequence}")
        );

        CanonicalSnapshots[sequence] = snapshot;
        CanonicalSnapshotSequenceByHash[canonicalHash] = sequence;
    }

    private static CanonicalSnapshotState CloneSnapshot(CanonicalSnapshotState snapshot)
    {
        return snapshot with
        {
            Rows = snapshot.Rows.ToDictionary(
                kvp => kvp.Key,
                kvp => kvp.Value?.DeepClone(),
                StringComparer.Ordinal
            ),
            Frontier = snapshot.Frontier.DeepClone() as JsonArray ?? new JsonArray()
        };
    }

    private static string ComputeCanonicalHash(Dictionary<string, JsonNode?> rows)
    {
        var keyVals = rows
            .OrderBy(x => x.Key, StringComparer.Ordinal)
            .Select(kvp => $"{kvp.Key}={CanonicalValueToHashString(kvp.Value)}");

        var payload = string.Join(";", keyVals);
        var bytes = SHA256.HashData(Encoding.UTF8.GetBytes(payload));
        return Convert.ToHexString(bytes).ToLowerInvariant();
    }

    private static string CanonicalValueToHashString(JsonNode? value)
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
}

public sealed record CanonicalSnapshotState(
    ulong Sequence,
    Dictionary<string, JsonNode?> Rows,
    string CanonicalHash,
    JsonArray Frontier
);

public sealed record QuerySpecStubState(
    string QuerySpecId,
    string Version,
    JsonNode? Descriptor
);

public sealed record ProjectionStubState(
    string ProjectionId,
    string QuerySpecId,
    JsonNode Checkpoint,
    JsonNode Rows,
    string Digest,
    bool IsInvalidated,
    string? InvalidationReason
);

public sealed record RuntimeMapResult(
    bool IsSuccess,
    string? Error,
    IReadOnlyList<string> CommandJsons,
    bool ShouldCloseConnection
)
{
    public static RuntimeMapResult Success(
        IReadOnlyList<string> commandJsons,
        bool shouldCloseConnection = false
    ) =>
        new(true, null, commandJsons, shouldCloseConnection);

    public static RuntimeMapResult Failure(string error) =>
        new(false, error, [], false);
}

public sealed record RuntimeEventMapResult(bool IsSuccess, string? Error, IReadOnlyList<string> OutboundMessages)
{
    public static RuntimeEventMapResult Success(IReadOnlyList<string> outboundMessages) =>
        new(true, null, outboundMessages);

    public static RuntimeEventMapResult Failure(string error) =>
        new(false, error, []);
}

public sealed class RuntimeInboundMessage
{
    public string? Type { get; set; }
    public string? Room { get; set; }
    public string? Pubkey { get; set; }
    [JsonPropertyName("trace_id")]
    public string? TraceId { get; set; }
    public string? Namespace { get; set; }
    public string? Key { get; set; }
    public JsonNode? Value { get; set; }
    public string? Ch { get; set; }
    [JsonPropertyName("after_id")]
    public string? AfterId { get; set; }
    [JsonPropertyName("target_id")]
    public string? TargetId { get; set; }
    [JsonPropertyName("index")]
    public ulong? Index { get; set; }
    [JsonPropertyName("from_index")]
    public ulong? FromIndex { get; set; }
    [JsonPropertyName("to_index")]
    public ulong? ToIndex { get; set; }
    [JsonPropertyName("hash")]
    public string? Hash { get; set; }
    [JsonPropertyName("data_b64")]
    public string? DataB64 { get; set; }
    [JsonPropertyName("hashes")]
    public string[]? Hashes { get; set; }
    [JsonPropertyName("nodes")]
    public string? NodesB64 { get; set; }
    [JsonPropertyName("known")]
    public string[]? KnownIds { get; set; }
    [JsonPropertyName("paths")]
    public string[]? MstPaths { get; set; }
    [JsonPropertyName("ids")]
    public string[]? MstIds { get; set; }
    [JsonPropertyName("to")]
    public string? ToPeerPubkey { get; set; }
    [JsonPropertyName("sdp")]
    public JsonNode? Sdp { get; set; }
    [JsonPropertyName("candidate")]
    public JsonNode? Candidate { get; set; }
    [JsonPropertyName("size")]
    public ulong? SizeBytes { get; set; }
    [JsonPropertyName("content_type")]
    public string? ContentType { get; set; }
    public JsonNode? Data { get; set; }
    [JsonPropertyName("ttl_ms")]
    public ulong? TtlMs { get; set; }
    [JsonPropertyName("now_unix_ms")]
    public ulong? NowUnixMs { get; set; }
    [JsonPropertyName("patterns")]
    public string[]? Patterns { get; set; }
    public string[]? Frontier { get; set; }
    public RuntimeInboundCaps? Caps { get; set; }

    [JsonPropertyName("session_id")]
    public ulong? SessionId { get; set; }
    [JsonPropertyName("token")]
    public RuntimeInboundToken? Token { get; set; }
    [JsonPropertyName("default")]
    public string? PolicyDefault { get; set; }
    [JsonPropertyName("rules")]
    public RuntimeInboundPolicyRule[]? PolicyRules { get; set; }
    [JsonPropertyName("since_unix_ms")]
    public ulong? SinceUnixMs { get; set; }
    [JsonPropertyName("interval_ms")]
    public ulong? IntervalMs { get; set; }
    [JsonPropertyName("intent_prefix")]
    public string? IntentPrefix { get; set; }
    [JsonPropertyName("query_spec_id")]
    public string? QuerySpecId { get; set; }
    [JsonPropertyName("version")]
    public string? QuerySpecVersion { get; set; }
    [JsonPropertyName("descriptor")]
    public JsonNode? Descriptor { get; set; }
    [JsonPropertyName("projection_id")]
    public string? ProjectionId { get; set; }
    [JsonPropertyName("target_checkpoint")]
    public JsonNode? TargetCheckpoint { get; set; }
    [JsonPropertyName("page_token")]
    public string? PageToken { get; set; }
    [JsonPropertyName("limit")]
    public ulong? Limit { get; set; }
    [JsonPropertyName("reason")]
    public string? InvalidationReason { get; set; }
    [JsonPropertyName("state_filter")]
    public string? StateFilter { get; set; }
    [JsonPropertyName("archive_ref")]
    public string? ArchiveRef { get; set; }
    [JsonPropertyName("mode")]
    public string? ArchiveMode { get; set; }
    [JsonPropertyName("import_mode")]
    public string? ImportMode { get; set; }
    [JsonPropertyName("expected_checkpoint")]
    public JsonNode? ExpectedCheckpoint { get; set; }
    [JsonPropertyName("parent_room_id")]
    public string? ParentRoomId { get; set; }
    [JsonPropertyName("child_room_id")]
    public string? ChildRoomId { get; set; }
    [JsonPropertyName("child_purpose")]
    public string? ChildPurpose { get; set; }
    [JsonPropertyName("created_by")]
    public string? CreatedBy { get; set; }
    [JsonPropertyName("promotion_policy_id")]
    public string? PromotionPolicyId { get; set; }
    [JsonPropertyName("parent_checkpoint")]
    public JsonNode? ParentCheckpoint { get; set; }
    [JsonPropertyName("room_id")]
    public string? TargetRoomId { get; set; }
    [JsonPropertyName("child_checkpoint_hash")]
    public string? ChildCheckpointHash { get; set; }
    [JsonPropertyName("payload_ref")]
    public string? PayloadRef { get; set; }
    [JsonPropertyName("idempotency_key")]
    public string? IdempotencyKey { get; set; }
    [JsonPropertyName("proposal_id")]
    public string? ProposalId { get; set; }
    [JsonPropertyName("blobs")]
    public RuntimeInboundBlob[]? Blobs { get; set; }
}

public sealed record TopologyChildStubState(
    string ChildRoomId,
    string ChildPurpose,
    string PromotionPolicyId,
    string CreatedBy,
    JsonObject Lineage
);

public sealed record TopologyPromotionStubState(
    string ProposalId,
    string ParentRoomId,
    string ChildRoomId,
    string ChildCheckpointHash,
    string PayloadRef,
    string ProposalDigest,
    bool Validated,
    bool Applied
);

public sealed class RuntimeInboundBlob
{
    [JsonPropertyName("hash")]
    public string? Hash { get; set; }

    [JsonPropertyName("data")]
    public string? Data { get; set; }

    [JsonPropertyName("data_b64")]
    public string? DataB64 { get; set; }
}

public sealed class RuntimeInboundCaps
{
    [JsonPropertyName("supports_ibf")]
    public bool SupportsIbf { get; set; } = true;

    [JsonPropertyName("supports_mst")]
    public bool SupportsMst { get; set; } = true;
}

public sealed class RuntimeInboundToken
{
    [JsonPropertyName("peer_pubkey")]
    public string? PeerPubkey { get; set; }
    [JsonPropertyName("peer_pubkey_hex")]
    public string? PeerPubkeyHex { get; set; }
    [JsonPropertyName("expiry")]
    public ulong? Expiry { get; set; }
    [JsonPropertyName("expiry_secs")]
    public ulong? ExpirySecs { get; set; }
    [JsonPropertyName("caps")]
    public string[]? Caps { get; set; }
    [JsonPropertyName("capabilities")]
    public string[]? Capabilities { get; set; }
    [JsonPropertyName("capability_profile_version")]
    public string? CapabilityProfileVersion { get; set; }
    [JsonPropertyName("profile_version")]
    public string? ProfileVersion { get; set; }
    [JsonPropertyName("sig")]
    public string? Sig { get; set; }
    [JsonPropertyName("sig_hex")]
    public string? SigHex { get; set; }
    [JsonPropertyName("continuity")]
    public RuntimeInboundTokenContinuity? Continuity { get; set; }

    public string? GetPeerPubkey() =>
        !string.IsNullOrWhiteSpace(PeerPubkey) ? PeerPubkey : PeerPubkeyHex;

    public ulong? GetExpiry() => Expiry ?? ExpirySecs;

    public string[]? GetCapabilities() =>
        Caps is { Length: > 0 } ? Caps : Capabilities;

    public string? GetCapabilityProfileVersion() =>
        !string.IsNullOrWhiteSpace(CapabilityProfileVersion) ? CapabilityProfileVersion : ProfileVersion;

    public string? GetSignature() =>
        !string.IsNullOrWhiteSpace(Sig) ? Sig : SigHex;
}

public sealed class RuntimeInboundTokenContinuity
{
    [JsonPropertyName("predecessor_peer_pubkey")]
    public string? PredecessorPeerPubkey { get; set; }
    [JsonPropertyName("predecessor_peer_pubkey_hex")]
    public string? PredecessorPeerPubkeyHex { get; set; }
    [JsonPropertyName("overlap_not_after")]
    public ulong? OverlapNotAfter { get; set; }
    [JsonPropertyName("overlap_not_after_epoch_secs")]
    public ulong? OverlapNotAfterEpochSecs { get; set; }
    [JsonPropertyName("revoked_predecessors")]
    public string[]? RevokedPredecessors { get; set; }

    public string? GetPredecessorPeerPubkey() =>
        !string.IsNullOrWhiteSpace(PredecessorPeerPubkey) ? PredecessorPeerPubkey : PredecessorPeerPubkeyHex;

    public ulong? GetOverlapNotAfter() => OverlapNotAfter ?? OverlapNotAfterEpochSecs;

    public string[]? GetRevokedPredecessors() => RevokedPredecessors;
}

public sealed class RuntimeInboundPolicyRule
{
    [JsonPropertyName("path_glob")]
    public string? PathGlob { get; set; }
    [JsonPropertyName("can_write")]
    public string[]? CanWrite { get; set; }
}
