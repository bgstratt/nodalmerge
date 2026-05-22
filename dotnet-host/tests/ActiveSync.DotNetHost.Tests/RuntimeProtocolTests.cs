using ActiveSync.DotNetHost.Runtime;

namespace ActiveSync.DotNetHost.Tests;

public class RuntimeProtocolTests
{
    [Fact]
    public void Hello_maps_to_ensure_open_client_hello_and_initial_request_pack_commands()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(42);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Equal(4, result.CommandJsons.Count);
        Assert.Contains("EnsureRoom", result.CommandJsons[0]);
        Assert.Contains("OpenSession", result.CommandJsons[1]);
        Assert.Contains("ClientHello", result.CommandJsons[2]);
        Assert.Contains("RequestServerPack", result.CommandJsons[3]);
        Assert.Contains("\"known_ids\":[]", result.CommandJsons[3]);
    }

    [Fact]
    public void Hello_handshake_guardrail_keeps_bootstrap_command_order_and_shapes()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(77);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"frontier\":[]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.True(result.CommandJsons.Count >= 4, "hello bootstrap must emit at least 4 commands");
        Assert.Contains("EnsureRoom", result.CommandJsons[0]);
        Assert.Contains("OpenSession", result.CommandJsons[1]);
        Assert.Contains("\"session_id\":77", result.CommandJsons[1]);
        Assert.Contains("ClientHello", result.CommandJsons[2]);
        Assert.Contains("\"peer_pubkey_hex\":\"peer-a\"", result.CommandJsons[2]);
        Assert.Contains("RequestServerPack", result.CommandJsons[3]);
        Assert.Contains("\"known_ids\":[]", result.CommandJsons[3]);
    }

    [Fact]
    public void Hello_with_token_passes_token_into_client_hello_payload()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(42);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"token\":{\"peer_pubkey\":\"ab\",\"expiry\":123,\"caps\":[\"read:world/**\"],\"sig\":\"cd\"}}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Contains("\"token\"", result.CommandJsons[2]);
        Assert.Contains("\"peer_pubkey\":\"ab\"", result.CommandJsons[2]);
        Assert.Contains("\"expiry\":123", result.CommandJsons[2]);
    }

    [Fact]
    public void Hello_with_token_missing_sig_returns_failure()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(42);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"token\":{\"peer_pubkey\":\"ab\",\"expiry\":123}}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("hello.token.sig is required", result.Error);
    }

    [Fact]
    public void Hello_marks_state_as_server_peer_when_pubkey_matches_configured_server_peer()
    {
        var mapper = new RuntimeProtocolMapper("server-peer-hex");
        var state = new RuntimeConnectionState(42);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"server-peer-hex\",\"frontier\":[]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.True(state.IsServerPeer);
    }

        [Fact]
        public void Identity_continuity_proof_allows_successor_signer_during_overlap_window()
        {
                var mapper = new RuntimeProtocolMapper();
                var state = new RuntimeConnectionState(42);
                var overlapNotAfter = (ulong)(DateTimeOffset.UtcNow.ToUnixTimeSeconds() + 60);

                var result = mapper.MapIncomingMessageToCommandJsons(
                        """
                        {
                            "type":"hello",
                            "room":"room-a",
                            "pubkey":"peer-b",
                            "token":{
                                "peer_pubkey":"peer-b",
                                "expiry":123,
                                "caps":["read:world/**"],
                                "sig":"cd",
                                "continuity":{
                                    "predecessor_peer_pubkey":"peer-a",
                                    "overlap_not_after":{{OVERLAP}},
                                    "revoked_predecessors":[]
                                }
                            }
                        }
                        """.Replace("{{OVERLAP}}", overlapNotAfter.ToString()),
                        state
                );

                Assert.True(result.IsSuccess);
                Assert.Contains("\"continuity\"", result.CommandJsons[2]);
                Assert.Contains("\"predecessor_peer_pubkey\":\"peer-a\"", result.CommandJsons[2]);
        }

        [Fact]
        public void Identity_continuity_proof_rejects_after_overlap_window_expiry()
        {
                var mapper = new RuntimeProtocolMapper();
                var state = new RuntimeConnectionState(42);

                var result = mapper.MapIncomingMessageToCommandJsons(
                        "{" +
                        "\"type\":\"hello\"," +
                        "\"room\":\"room-a\"," +
                        "\"pubkey\":\"peer-b\"," +
                        "\"token\":{" +
                        "\"peer_pubkey\":\"peer-b\"," +
                        "\"expiry\":123," +
                        "\"caps\":[\"read:world/**\"]," +
                        "\"sig\":\"cd\"," +
                        "\"continuity\":{" +
                        "\"predecessor_peer_pubkey\":\"peer-a\"," +
                        "\"overlap_not_after\":1," +
                        "\"revoked_predecessors\":[]" +
                        "}" +
                        "}" +
                        "}",
                        state
                );

                Assert.False(result.IsSuccess);
                Assert.Equal("hello.token.continuity overlap window expired", result.Error);
        }

        [Fact]
        public void Identity_continuity_proof_rejects_revoked_predecessor_key()
        {
                var mapper = new RuntimeProtocolMapper();
                var state = new RuntimeConnectionState(42);
                var overlapNotAfter = (ulong)(DateTimeOffset.UtcNow.ToUnixTimeSeconds() + 60);

                var result = mapper.MapIncomingMessageToCommandJsons(
                        """
                        {
                            "type":"hello",
                            "room":"room-a",
                            "pubkey":"peer-b",
                            "token":{
                                "peer_pubkey":"peer-b",
                                "expiry":123,
                                "caps":["read:world/**"],
                                "sig":"cd",
                                "continuity":{
                                    "predecessor_peer_pubkey":"peer-a",
                                    "overlap_not_after":{{OVERLAP}},
                                    "revoked_predecessors":["peer-a"]
                                }
                            }
                        }
                        """.Replace("{{OVERLAP}}", overlapNotAfter.ToString()),
                        state
                );

                Assert.False(result.IsSuccess);
                Assert.Equal("hello.token.continuity predecessor key is revoked", result.Error);
        }

    [Fact]
    public void Event_mapper_converts_welcome_prepared_to_welcome_wire_message()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[{\"WelcomePrepared\":{\"room_id\":\"room-a\",\"session_id\":42,\"negotiated\":{\"supports_ibf\":true,\"supports_mst\":false},\"missing_from_server_count\":0}}]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"welcome\"", result.OutboundMessages[0]);
        Assert.Contains("\"room\":\"room-a\"", result.OutboundMessages[0]);
    }

    [Fact]
    public void Ensure_room_message_maps_without_prior_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(77);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"ensure-room\",\"room\":\"room-b\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("EnsureRoom", result.CommandJsons[0]);
    }

    [Fact]
    public void Open_session_message_maps_without_prior_hello_when_room_and_pubkey_present()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(99);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"open-session\",\"room\":\"room-c\",\"pubkey\":\"peer-c\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("OpenSession", result.CommandJsons[0]);
    }

    [Fact]
    public void Close_session_sets_close_intent()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(42)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"close-session\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.True(result.ShouldCloseConnection);
    }

    [Fact]
    public void Set_room_key_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("room.admin");

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-room-key\",\"pubkey\":\"11\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("SetRoomKey", result.CommandJsons[0]);
        Assert.Contains("\"pubkey_hex\":\"11\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Set_room_key_requires_pubkey()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("room.admin");

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-room-key\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("set-room-key.pubkey is required", result.Error);
    }

    [Fact]
    public void Hello_with_session_id_overrides_connection_default_and_is_reused_for_close()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(42);

        var helloResult = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\",\"session_id\":9001}",
            state
        );

        Assert.True(helloResult.IsSuccess);
        Assert.Equal(9001ul, state.SessionId);
        Assert.Contains("\"session_id\":9001", helloResult.CommandJsons[1]);
        Assert.Contains("\"session_id\":9001", helloResult.CommandJsons[2]);

        var closeResult = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"close-session\"}",
            state
        );

        Assert.True(closeResult.IsSuccess);
        Assert.Contains("\"session_id\":9001", closeResult.CommandJsons[0]);
    }

    [Fact]
    public void Open_session_with_session_id_updates_state_for_follow_on_close()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            RoomId = "room-a"
        };

        var openResult = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"open-session\",\"pubkey\":\"peer-a\",\"session_id\":55}",
            state
        );

        Assert.True(openResult.IsSuccess);
        Assert.Equal(55ul, state.SessionId);
        Assert.Contains("\"session_id\":55", openResult.CommandJsons[0]);

        var closeResult = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"close-session\"}",
            state
        );

        Assert.True(closeResult.IsSuccess);
        Assert.Contains("\"session_id\":55", closeResult.CommandJsons[0]);
    }

    [Fact]
    public void Incoming_invalid_json_returns_failure()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var result = mapper.MapIncomingMessageToCommandJsons("{not-json", state);

        Assert.False(result.IsSuccess);
        Assert.Equal("invalid JSON", result.Error);
    }

    [Fact]
    public void Incoming_missing_type_returns_failure()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var result = mapper.MapIncomingMessageToCommandJsons("{}", state);

        Assert.False(result.IsSuccess);
        Assert.Equal("missing message type", result.Error);
    }

    [Fact]
    public void Noop_requires_initialized_state()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"noop\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("hello must be sent first", result.Error);
    }

    [Fact]
    public void Hello_cannot_be_processed_twice_on_same_connection()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var first = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\"}",
            state
        );
        Assert.True(first.IsSuccess);

        var second = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\"}",
            state
        );

        Assert.False(second.IsSuccess);
        Assert.Equal("hello already processed for this connection", second.Error);
    }

    [Fact]
    public void Unsupported_message_type_returns_failure()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(42)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"unknown-type\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("unsupported message type for PR7 step 9", result.Error);
    }

    [Fact]
    public void Ensure_room_with_mismatched_room_after_hello_returns_failure()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var hello = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\"}",
            state
        );
        Assert.True(hello.IsSuccess);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"ensure-room\",\"room\":\"room-b\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("ensure-room.room must match initialized room", result.Error);
    }

    [Fact]
    public void Open_session_with_mismatched_room_after_hello_returns_failure()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var hello = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\"}",
            state
        );
        Assert.True(hello.IsSuccess);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"open-session\",\"room\":\"room-b\",\"pubkey\":\"peer-a\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("open-session.room must match initialized room", result.Error);
    }

    [Fact]
    public void Client_hello_with_mismatched_pubkey_after_hello_returns_failure()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var hello = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\"}",
            state
        );
        Assert.True(hello.IsSuccess);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"client-hello\",\"room\":\"room-a\",\"pubkey\":\"peer-b\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("client-hello.pubkey must match initialized pubkey", result.Error);
    }

    [Fact]
    public void Event_mapper_converts_all_supported_event_variants()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"RoomEnsured\":{\"room_id\":\"room-a\"}}," +
            "{\"SessionOpened\":{\"room_id\":\"room-a\",\"session_id\":42}}," +
            "{\"SessionClosed\":{\"room_id\":\"room-a\",\"session_id\":42}}," +
            "\"NoopAck\"" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Equal(4, result.OutboundMessages.Count);
        Assert.Contains("\"type\":\"room-ensured\"", result.OutboundMessages[0]);
        Assert.Contains("\"type\":\"session-opened\"", result.OutboundMessages[1]);
        Assert.Contains("\"type\":\"session-closed\"", result.OutboundMessages[2]);
        Assert.Contains("\"type\":\"noop-ack\"", result.OutboundMessages[3]);
    }

    [Fact]
    public void Event_mapper_rejects_invalid_json()
    {
        var mapper = new RuntimeProtocolMapper();

        var result = mapper.MapEventsJsonToOutboundMessages("{not-json");

        Assert.False(result.IsSuccess);
        Assert.Equal("host event decode failed", result.Error);
    }

    [Fact]
    public void Event_mapper_rejects_non_array_payload()
    {
        var mapper = new RuntimeProtocolMapper();

        var result = mapper.MapEventsJsonToOutboundMessages("{}");

        Assert.False(result.IsSuccess);
        Assert.Equal("host event payload must be an array", result.Error);
    }

    [Fact]
    public void Map_set_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1);

        var hello = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"peer-a\"}",
            state
        );
        Assert.True(hello.IsSuccess);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"map-set\",\"namespace\":\"world\",\"key\":\"player1\",\"value\":{\"x\":10}}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("MapSet", result.CommandJsons[0]);
        Assert.Contains("\"namespace\":\"world\"", result.CommandJsons[0]);
        Assert.Contains("\"key\":\"player1\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Map_get_requires_key()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"map-get\",\"namespace\":\"world\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("map-get.key is required", result.Error);
    }

    [Fact]
    public void Map_all_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"map-all\",\"namespace\":\"world\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("MapAll", result.CommandJsons[0]);
    }

    [Fact]
    public void Event_mapper_converts_map_events_to_runtime_messages()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"MapValueUpserted\":{\"room_id\":\"room-a\",\"namespace\":\"world\",\"key\":\"player1\",\"value\":{\"x\":10}}}," +
            "{\"MapValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"world\",\"key\":\"player1\",\"value\":{\"x\":10}}}," +
            "{\"MapValueDeleted\":{\"room_id\":\"room-a\",\"namespace\":\"world\",\"key\":\"player1\",\"found\":true}}," +
            "{\"MapEntriesListed\":{\"room_id\":\"room-a\",\"namespace\":\"world\",\"entries\":[{\"key\":\"player1\",\"value\":{\"x\":10}}]}}" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Equal(4, result.OutboundMessages.Count);
        Assert.Contains("\"type\":\"map-set-ack\"", result.OutboundMessages[0]);
        Assert.Contains("\"type\":\"map-value\"", result.OutboundMessages[1]);
        Assert.Contains("\"type\":\"map-delete-ack\"", result.OutboundMessages[2]);
        Assert.Contains("\"type\":\"map-all\"", result.OutboundMessages[3]);
    }

    [Fact]
    public void Text_insert_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"text-insert\",\"namespace\":\"doc\",\"key\":\"title\",\"ch\":\"a\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("TextInsert", result.CommandJsons[0]);
        Assert.Contains("\"key\":\"title\"", result.CommandJsons[0]);
        Assert.Contains("\"ch\":\"a\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Text_delete_requires_target_id()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"text-delete\",\"namespace\":\"doc\",\"key\":\"title\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("text-delete.target_id is required", result.Error);
    }

    [Fact]
    public void Text_get_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"text-get\",\"namespace\":\"doc\",\"key\":\"title\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("TextGet", result.CommandJsons[0]);
    }

    [Fact]
    public void Event_mapper_converts_text_events_to_runtime_messages()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"TextValueInserted\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"id\":\"text-1\",\"ch\":\"a\"}}," +
            "{\"TextValueDeleted\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"target_id\":\"text-1\",\"found\":true}}," +
            "{\"TextValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"title\",\"value\":\"a\",\"entries\":[{\"id\":\"text-1\",\"ch\":\"a\"}]}}" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Equal(3, result.OutboundMessages.Count);
        Assert.Contains("\"type\":\"text-insert-ack\"", result.OutboundMessages[0]);
        Assert.Contains("\"type\":\"text-delete-ack\"", result.OutboundMessages[1]);
        Assert.Contains("\"type\":\"text-value\"", result.OutboundMessages[2]);
    }

    [Fact]
    public void List_push_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"list-push\",\"namespace\":\"doc\",\"key\":\"items\",\"value\":\"a\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("ListPush", result.CommandJsons[0]);
    }

    [Fact]
    public void List_insert_requires_index()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"list-insert\",\"namespace\":\"doc\",\"key\":\"items\",\"value\":\"a\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("list-insert.index is required", result.Error);
    }

    [Fact]
    public void List_get_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"list-get\",\"namespace\":\"doc\",\"key\":\"items\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("ListGet", result.CommandJsons[0]);
    }

    [Fact]
    public void List_move_requires_from_index()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"list-move\",\"namespace\":\"doc\",\"key\":\"items\",\"to_index\":0}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("list-move.from_index is required", result.Error);
    }

    [Fact]
    public void List_update_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"list-update\",\"namespace\":\"doc\",\"key\":\"items\",\"index\":1,\"value\":\"b-updated\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("ListUpdate", result.CommandJsons[0]);
    }

    [Fact]
    public void Event_mapper_converts_list_events_to_runtime_messages()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"ListValuePushed\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"items\",\"id\":\"list-1\",\"index\":0,\"value\":\"a\"}}," +
            "{\"ListValueInserted\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"items\",\"id\":\"list-2\",\"index\":0,\"value\":\"b\"}}," +
            "{\"ListValueDeleted\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"items\",\"index\":1,\"found\":true,\"removed\":\"a\"}}," +
            "{\"ListValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"items\",\"entries\":[{\"id\":\"list-2\",\"value\":\"b\"}]}}," +
            "{\"ListValueMoved\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"items\",\"from_index\":2,\"to_index\":0,\"found\":true,\"id\":\"list-3\"}}," +
            "{\"ListValueUpdated\":{\"room_id\":\"room-a\",\"namespace\":\"doc\",\"key\":\"items\",\"index\":1,\"found\":true,\"id\":\"list-2\",\"value\":\"b-updated\"}}" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Equal(6, result.OutboundMessages.Count);
        Assert.Contains("\"type\":\"list-push-ack\"", result.OutboundMessages[0]);
        Assert.Contains("\"type\":\"list-insert-ack\"", result.OutboundMessages[1]);
        Assert.Contains("\"type\":\"list-delete-ack\"", result.OutboundMessages[2]);
        Assert.Contains("\"type\":\"list-value\"", result.OutboundMessages[3]);
        Assert.Contains("\"type\":\"list-move-ack\"", result.OutboundMessages[4]);
        Assert.Contains("\"type\":\"list-update-ack\"", result.OutboundMessages[5]);
    }

    [Fact]
    public void Blob_set_requires_data_b64()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"blob-set\",\"namespace\":\"assets\",\"hash\":\"sha256:abc\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("blob-set.data_b64 is required", result.Error);
    }

    [Fact]
    public void Blob_get_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"blob-get\",\"namespace\":\"assets\",\"hash\":\"sha256:abc\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("BlobGet", result.CommandJsons[0]);
    }

    [Fact]
    public void Blob_get_many_requires_hashes()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"blob-get-many\",\"namespace\":\"assets\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("blob-get-many.hashes is required", result.Error);
    }

    [Fact]
    public void Request_upload_requires_hash()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"request-upload\",\"namespace\":\"assets\",\"size\":1024}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("request-upload.hash is required", result.Error);
    }

    [Fact]
    public void Request_upload_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"request-upload\",\"namespace\":\"assets\",\"hash\":\"sha256:abc\",\"size\":1024,\"content_type\":\"audio/aac\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("RequestUpload", result.CommandJsons[0]);
        Assert.Contains("\"size_bytes\":1024", result.CommandJsons[0]);
    }

    [Fact]
    public void Blob_request_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"blob-request\",\"namespace\":\"assets\",\"hashes\":[\"sha256:abc\",\"sha256:def\"]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("BlobRequest", result.CommandJsons[0]);
    }

    [Fact]
    public void Presence_set_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"presence\",\"data\":{\"state\":\"online\"},\"ttl_ms\":30000,\"now_unix_ms\":1700000100}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("PresenceSet", result.CommandJsons[0]);
        Assert.Contains("\"session_id\":7", result.CommandJsons[0]);
    }

    [Fact]
    public void Presence_get_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"presence-get\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("PresenceGetAll", result.CommandJsons[0]);
    }

    [Fact]
    public void Presence_sweep_requires_now_unix_ms()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"presence-sweep\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("presence-sweep.now_unix_ms is required", result.Error);
    }

    [Fact]
    public void Subscribe_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"subscribe\",\"patterns\":[\"world/**\",\"chat/*\"]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("Subscribe", result.CommandJsons[0]);
        Assert.Contains("\"session_id\":7", result.CommandJsons[0]);
        Assert.Contains("world/**", result.CommandJsons[0]);
    }

    [Fact]
    public void Set_policy_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("policy.admin");

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-policy\",\"default\":\"deny\",\"rules\":[{\"path_glob\":\"world/**\",\"can_write\":[\"11\"]}]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("SetPolicy", result.CommandJsons[0]);
        Assert.Contains("\"default\":\"deny\"", result.CommandJsons[0]);
        Assert.Contains("\"path_glob\":\"world/**\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Set_policy_rejects_unknown_default()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("policy.admin");

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-policy\",\"default\":\"custom\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("set-policy: unknown default 'custom', use 'allow' or 'deny'", result.Error);
    }

    [Fact]
    public void Set_policy_rejects_missing_rule_path_glob()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("policy.admin");

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-policy\",\"rules\":[{\"can_write\":[\"11\"]}]}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("set-policy: each rule must have a 'path_glob' string", result.Error);
    }

    [Fact]
    public void Pack_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"pack\",\"nodes\":\"AQID\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("ImportPack", result.CommandJsons[0]);
        Assert.Contains("\"nodes_b64\":\"AQID\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Set_room_key_rejects_without_room_admin_capability()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-room-key\",\"pubkey\":\"11\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("reject.control_plane_forbidden: command=set-room-key requires=room.admin", result.Error);
    }

    [Fact]
    public void Set_policy_rejects_without_policy_admin_capability()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-policy\",\"default\":\"allow\",\"rules\":[]}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("reject.control_plane_forbidden: command=set-policy requires=policy.admin", result.Error);
    }

    [Fact]
    public void Set_policy_allows_server_peer_without_policy_admin_capability()
    {
        var mapper = new RuntimeProtocolMapper("server-peer-hex");
        var state = new RuntimeConnectionState(1);

        var hello = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"hello\",\"room\":\"room-a\",\"pubkey\":\"server-peer-hex\",\"frontier\":[]}",
            state
        );
        Assert.True(hello.IsSuccess);

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"set-policy\",\"default\":\"allow\",\"rules\":[]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("SetPolicy", result.CommandJsons[0]);
    }

    [Fact]
    public void Start_tick_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("tick.admin");

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"start-tick\",\"interval_ms\":33,\"intent_prefix\":\"intent/\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("StartTick", result.CommandJsons[0]);
        Assert.Contains("\"interval_ms\":33", result.CommandJsons[0]);
        Assert.Contains("\"intent_prefix\":\"intent/\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Stop_tick_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };
        state.SessionCapabilities.Add("tick.admin");

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"stop-tick\"}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("\"command\":\"StopTick\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Start_tick_rejects_without_tick_admin_capability()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"start-tick\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("reject.control_plane_forbidden: command=start-tick requires=tick.admin", result.Error);
    }

    [Fact]
    public void Stop_tick_rejects_without_tick_admin_capability()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(1)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"stop-tick\"}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("reject.control_plane_forbidden: command=stop-tick requires=tick.admin", result.Error);
    }

    [Fact]
    public void Request_server_pack_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"request-server-pack\",\"known\":[\"11\"]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("RequestServerPack", result.CommandJsons[0]);
        Assert.Contains("\"known_ids\":[\"11\"]", result.CommandJsons[0]);
    }

    [Fact]
    public void Mst_request_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"mst-request\",\"paths\":[\"\"]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("MstRequest", result.CommandJsons[0]);
        Assert.Contains("\"paths\":[\"\"]", result.CommandJsons[0]);
    }

    [Fact]
    public void Mst_done_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"mst-done\",\"ids\":[\"11\"]}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("MstDone", result.CommandJsons[0]);
        Assert.Contains("\"ids\":[\"11\"]", result.CommandJsons[0]);
    }

    [Fact]
    public void Recent_conflicts_maps_to_host_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"recent-conflicts\",\"since_unix_ms\":1700000000}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("GetRecentConflicts", result.CommandJsons[0]);
        Assert.Contains("\"since_unix_ms\":1700000000", result.CommandJsons[0]);
    }

    [Fact]
    public void Webrtc_offer_maps_to_host_relay_command_after_hello()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"webrtc-offer\",\"to\":\"peer-b\",\"sdp\":{\"type\":\"offer\",\"sdp\":\"v=0\"}}",
            state
        );

        Assert.True(result.IsSuccess);
        Assert.Single(result.CommandJsons);
        Assert.Contains("RelayPeerSignal", result.CommandJsons[0]);
        Assert.Contains("\"msg_type\":\"webrtc-offer\"", result.CommandJsons[0]);
        Assert.Contains("\"to_peer_pubkey\":\"peer-b\"", result.CommandJsons[0]);
    }

    [Fact]
    public void Webrtc_relay_requires_to_field()
    {
        var mapper = new RuntimeProtocolMapper();
        var state = new RuntimeConnectionState(7)
        {
            IsInitialized = true,
            RoomId = "room-a",
            PeerPubkeyHex = "peer-a"
        };

        var result = mapper.MapIncomingMessageToCommandJsons(
            "{\"type\":\"webrtc-ice\",\"candidate\":{\"candidate\":\"abc\"}}",
            state
        );

        Assert.False(result.IsSuccess);
        Assert.Equal("webrtc-ice.to is required", result.Error);
    }

    [Fact]
    public void Event_mapper_converts_blob_events_to_runtime_messages()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"BlobValueStored\":{\"room_id\":\"room-a\",\"namespace\":\"assets\",\"hash\":\"sha256:abc\",\"stored\":true}}," +
            "{\"BlobValueRead\":{\"room_id\":\"room-a\",\"namespace\":\"assets\",\"hash\":\"sha256:abc\",\"found\":true,\"data_b64\":\"QUJD\"}}," +
            "{\"BlobValuesRead\":{\"room_id\":\"room-a\",\"namespace\":\"assets\",\"entries\":[{\"hash\":\"sha256:abc\",\"data_b64\":\"QUJD\"}],\"missing\":[\"sha256:missing\"]}}," +
            "{\"UploadGranted\":{\"room_id\":\"room-a\",\"namespace\":\"assets\",\"hash\":\"sha256:abc\",\"url\":\"https://upload.example/blob\",\"expires_at_unix\":1700000000}}," +
            "{\"UploadDenied\":{\"room_id\":\"room-a\",\"namespace\":\"assets\",\"hash\":\"sha256:denied\",\"reason\":\"quota exceeded\"}}," +
            "{\"BlobRedirectPrepared\":{\"room_id\":\"room-a\",\"namespace\":\"assets\",\"redirects\":[{\"hash\":\"sha256:abc\",\"url\":\"https://download.example/blob\",\"expires_at_unix\":1700000001}]}}," +
            "{\"BlobPackPrepared\":{\"room_id\":\"room-a\",\"namespace\":\"assets\",\"blobs\":[{\"hash\":\"sha256:abc\",\"data_b64\":\"QUJD\"}],\"requested\":[\"sha256:abc\",\"sha256:missing\"]}}," +
            "{\"RoomLocked\":{\"room_id\":\"room-a\",\"pubkey_hex\":\"11\"}}," +
            "{\"SetRoomKeyRejected\":{\"room_id\":\"room-a\",\"msg\":\"set-room-key: room already locked\"}}," +
            "{\"PresenceValueSet\":{\"room_id\":\"room-a\",\"session_id\":7,\"from_peer_pubkey\":\"peer-7\",\"data\":{\"state\":\"online\"},\"joined\":true}}," +
            "{\"PresenceValueRemoved\":{\"room_id\":\"room-a\",\"session_id\":7,\"from_peer_pubkey\":\"peer-7\",\"reason\":\"stale\"}}," +
            "{\"PresenceValuesListed\":{\"room_id\":\"room-a\",\"entries\":[{\"session_id\":7,\"from_peer_pubkey\":\"peer-7\",\"data\":{\"state\":\"online\"},\"expires_at_unix_ms\":1700001000}]}}," +
            "{\"SubscriptionUpdated\":{\"room_id\":\"room-a\",\"session_id\":7,\"patterns\":[\"world/**\"]}}," +
            "{\"PolicySet\":{\"room_id\":\"room-a\"}}," +
            "{\"SetPolicyRejected\":{\"room_id\":\"room-a\",\"msg\":\"set-policy: unknown default 'custom', use 'allow' or 'deny'\"}}," +
            "{\"PackImported\":{\"room_id\":\"room-a\",\"incoming_count\":2,\"accepted_count\":1,\"rejected_count\":1}}," +
            "{\"ServerPackPrepared\":{\"room_id\":\"room-a\",\"nodes_b64\":\"AQID\",\"root_hex\":\"ff\"}}," +
            "{\"MstResponsePrepared\":{\"room_id\":\"room-a\",\"nodes\":[{\"path\":\"\",\"hash\":\"00\",\"keys\":[\"11\"],\"children\":{}}]}}," +
            "{\"PeerSignalRelayed\":{\"room_id\":\"room-a\",\"from_peer_pubkey\":\"peer-a\",\"msg_type\":\"webrtc-ice\",\"to_peer_pubkey\":\"peer-b\",\"payload\":{\"candidate\":{\"candidate\":\"abc\"}}}}" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Equal(19, result.OutboundMessages.Count);
        Assert.Contains("\"type\":\"blob-set-ack\"", result.OutboundMessages[0]);
        Assert.Contains("\"type\":\"blob-value\"", result.OutboundMessages[1]);
        Assert.Contains("\"type\":\"blob-values\"", result.OutboundMessages[2]);
        Assert.Contains("\"type\":\"upload-granted\"", result.OutboundMessages[3]);
        Assert.Contains("\"type\":\"upload-denied\"", result.OutboundMessages[4]);
        Assert.Contains("\"type\":\"blob-redirect\"", result.OutboundMessages[5]);
        Assert.Contains("\"type\":\"blob-pack\"", result.OutboundMessages[6]);
        Assert.Contains("\"type\":\"room-locked\"", result.OutboundMessages[7]);
        Assert.Contains("\"type\":\"set-room-key-rejected\"", result.OutboundMessages[8]);
        Assert.Contains("\"type\":\"presence\"", result.OutboundMessages[9]);
        Assert.Contains("\"type\":\"presence-leave\"", result.OutboundMessages[10]);
        Assert.Contains("\"type\":\"presence-snapshot\"", result.OutboundMessages[11]);
        Assert.Contains("\"type\":\"subscribe-ack\"", result.OutboundMessages[12]);
        Assert.Contains("\"type\":\"policy-set\"", result.OutboundMessages[13]);
        Assert.Contains("\"type\":\"set-policy-rejected\"", result.OutboundMessages[14]);
        Assert.Contains("\"type\":\"pack-ack\"", result.OutboundMessages[15]);
        Assert.Contains("\"type\":\"pack\"", result.OutboundMessages[16]);
        Assert.Contains("\"type\":\"mst-response\"", result.OutboundMessages[17]);
        Assert.Contains("\"type\":\"webrtc-ice\"", result.OutboundMessages[18]);
    }

    [Fact]
    public void Event_mapper_converts_conflict_events_to_runtime_messages()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"ConflictsObserved\":{\"room_id\":\"room-a\",\"entries\":[{\"at_unix_ms\":1700000001,\"event\":{\"kind\":\"map_overwrite\",\"key\":\"world/greeting\",\"winner_author\":[1],\"winner_lamport\":2,\"winner_op\":{\"kind\":\"set\",\"value\":[104,105]},\"loser_author\":[2],\"loser_lamport\":1,\"loser_op\":{\"kind\":\"set\",\"value\":[98,121,101]}}}]}}" +
            "," +
            "{\"RecentConflictsListed\":{\"room_id\":\"room-a\",\"entries\":[{\"at_unix_ms\":1700000001,\"event\":{\"kind\":\"map_overwrite\",\"key\":\"world/greeting\",\"winner_author\":[1],\"winner_lamport\":2,\"winner_op\":{\"kind\":\"set\",\"value\":[104,105]},\"loser_author\":[2],\"loser_lamport\":1,\"loser_op\":{\"kind\":\"set\",\"value\":[98,121,101]}}}]}}" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Equal(2, result.OutboundMessages.Count);
        Assert.Contains("\"type\":\"conflict\"", result.OutboundMessages[0]);
        Assert.Contains("\"room\":\"room-a\"", result.OutboundMessages[0]);
        Assert.Contains("\"type\":\"recent-conflicts\"", result.OutboundMessages[1]);
        Assert.Contains("\"entries\"", result.OutboundMessages[1]);
    }

    [Fact]
    public void Event_mapper_pack_imported_with_kept_nodes_maps_to_pack_ack_counts()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"PackImported\":{\"room_id\":\"room-a\",\"incoming_count\":2,\"accepted_count\":1,\"rejected_count\":1}}" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"pack-ack\"", result.OutboundMessages[0]);
        Assert.Contains("\"incoming_count\":2", result.OutboundMessages[0]);
        Assert.Contains("\"accepted_count\":1", result.OutboundMessages[0]);
        Assert.Contains("\"rejected_count\":1", result.OutboundMessages[0]);
    }

    [Fact]
    public void Event_mapper_pack_imported_with_zero_kept_nodes_maps_to_pack_ack_counts()
    {
        var mapper = new RuntimeProtocolMapper();
        var eventsJson =
            "[" +
            "{\"PackImported\":{\"room_id\":\"room-a\",\"incoming_count\":2,\"accepted_count\":0,\"rejected_count\":2}}" +
            "]";

        var result = mapper.MapEventsJsonToOutboundMessages(eventsJson);

        Assert.True(result.IsSuccess);
        Assert.Single(result.OutboundMessages);
        Assert.Contains("\"type\":\"pack-ack\"", result.OutboundMessages[0]);
        Assert.Contains("\"incoming_count\":2", result.OutboundMessages[0]);
        Assert.Contains("\"accepted_count\":0", result.OutboundMessages[0]);
        Assert.Contains("\"rejected_count\":2", result.OutboundMessages[0]);
    }
}
