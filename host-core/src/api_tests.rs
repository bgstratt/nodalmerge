use crate::api::{CapabilitySet, ClientHelloPayload, CommandEnvelope, HostCommand, HostEvent, PolicyRulePayload};
use crate::engine::{HostEngine, shape_catchup_pack_payload_b64};
use crate::errors::HostCoreError;
use crate::traits::{HostBlobUrlResolver, PresignedBlobUrl};
use activesync_core::{MapOp, Op, RoomToken, StateGraph};
use ed25519_dalek::SigningKey;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug)]
struct StaticBlobUrlResolver;

impl HostBlobUrlResolver for StaticBlobUrlResolver {
    fn resolve_put_url(
        &self,
        _room_id: &str,
        _namespace: &str,
        hash: &str,
        _size_bytes: u64,
        _content_type: Option<&str>,
    ) -> Option<PresignedBlobUrl> {
        Some(PresignedBlobUrl {
            url: format!("https://upload.example/{hash}"),
            expires_at_unix: 1_700_000_000,
        })
    }

    fn resolve_get_url(
        &self,
        _room_id: &str,
        _namespace: &str,
        hash: &str,
    ) -> Option<PresignedBlobUrl> {
        Some(PresignedBlobUrl {
            url: format!("https://download.example/{hash}"),
            expires_at_unix: 1_700_000_123,
        })
    }
}

#[test]
fn ensure_room_then_open_session_then_hello_prepares_welcome() {
    let mut engine = HostEngine::new();

    let ensured = engine
        .apply(CommandEnvelope::new("room-a", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    assert_eq!(
        ensured.events,
        vec![HostEvent::RoomEnsured {
            room_id: "room-a".to_string()
        }]
    );

    let opened = engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::OpenSession {
                session_id: 1,
                peer_pubkey_hex: "peer-hex".to_string(),
            },
        ))
        .expect("open session should succeed");
    assert_eq!(
        opened.events,
        vec![HostEvent::SessionOpened {
            room_id: "room-a".to_string(),
            session_id: 1,
        }]
    );

    let hello = engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::ClientHello {
                session_id: 1,
                hello: ClientHelloPayload {
                    peer_pubkey_hex: "peer-hex".to_string(),
                    client_frontier: vec![],
                    capabilities: CapabilitySet {
                        supports_ibf: true,
                        supports_mst: false,
                    },
                    token: None,
                },
            },
        ))
        .expect("hello should succeed");

    assert_eq!(
        hello.events,
        vec![HostEvent::WelcomePrepared {
            room_id: "room-a".to_string(),
            session_id: 1,
            negotiated: CapabilitySet {
                supports_ibf: true,
                supports_mst: false,
            },
            missing_from_server_count: 0,
        }]
    );
}

#[test]
fn open_session_without_room_fails() {
    let mut engine = HostEngine::new();
    let err = engine
        .apply(CommandEnvelope::new(
            "missing",
            HostCommand::OpenSession {
                session_id: 7,
                peer_pubkey_hex: "peer".to_string(),
            },
        ))
        .expect_err("open without room should fail");
    assert_eq!(err, HostCoreError::RoomNotFound);
}

#[test]
fn duplicate_session_open_fails() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-a", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::OpenSession {
                session_id: 42,
                peer_pubkey_hex: "peer".to_string(),
            },
        ))
        .expect("first open should succeed");

    let err = engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::OpenSession {
                session_id: 42,
                peer_pubkey_hex: "peer".to_string(),
            },
        ))
        .expect_err("duplicate open should fail");
    assert_eq!(err, HostCoreError::SessionAlreadyOpen);
}

#[test]
fn hello_with_wrong_peer_fails() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-a", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::OpenSession {
                session_id: 9,
                peer_pubkey_hex: "peer-1".to_string(),
            },
        ))
        .expect("open should succeed");

    let err = engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::ClientHello {
                session_id: 9,
                hello: ClientHelloPayload {
                    peer_pubkey_hex: "peer-2".to_string(),
                    client_frontier: vec![],
                    capabilities: CapabilitySet::default(),
                    token: None,
                },
            },
        ))
        .expect_err("peer mismatch should fail");
    assert_eq!(err, HostCoreError::ProtocolViolation);
}

#[test]
fn close_session_emits_session_closed() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-a", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::OpenSession {
                session_id: 99,
                peer_pubkey_hex: "peer".to_string(),
            },
        ))
        .expect("open should succeed");

    let closed = engine
        .apply(CommandEnvelope::new(
            "room-a",
            HostCommand::CloseSession { session_id: 99 },
        ))
        .expect("close should succeed");

    assert_eq!(
        closed.events,
        vec![HostEvent::SessionClosed {
            room_id: "room-a".to_string(),
            session_id: 99,
        }]
    );
}

#[test]
fn map_set_get_delete_all_roundtrip_emits_expected_events() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-map", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let set_result = engine
        .apply(CommandEnvelope::new(
            "room-map",
            HostCommand::MapSet {
                namespace: "world".to_string(),
                key: "player1".to_string(),
                value: json!({ "x": 10, "y": 20 }),
            },
        ))
        .expect("map set should succeed");
    assert_eq!(
        set_result.events,
        vec![HostEvent::MapValueUpserted {
            room_id: "room-map".to_string(),
            namespace: "world".to_string(),
            key: "player1".to_string(),
            value: json!({ "x": 10, "y": 20 }),
        }]
    );

    let get_result = engine
        .apply(CommandEnvelope::new(
            "room-map",
            HostCommand::MapGet {
                namespace: "world".to_string(),
                key: "player1".to_string(),
            },
        ))
        .expect("map get should succeed");
    assert_eq!(
        get_result.events,
        vec![HostEvent::MapValueRead {
            room_id: "room-map".to_string(),
            namespace: "world".to_string(),
            key: "player1".to_string(),
            value: Some(json!({ "x": 10, "y": 20 })),
        }]
    );

    let all_result = engine
        .apply(CommandEnvelope::new(
            "room-map",
            HostCommand::MapAll {
                namespace: "world".to_string(),
            },
        ))
        .expect("map all should succeed");
    assert_eq!(all_result.events.len(), 1);
    let HostEvent::MapEntriesListed { entries, .. } = &all_result.events[0] else {
        panic!("expected MapEntriesListed event");
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].key, "player1");
    assert_eq!(entries[0].value, json!({ "x": 10, "y": 20 }));

    let delete_result = engine
        .apply(CommandEnvelope::new(
            "room-map",
            HostCommand::MapDelete {
                namespace: "world".to_string(),
                key: "player1".to_string(),
            },
        ))
        .expect("map delete should succeed");
    assert_eq!(
        delete_result.events,
        vec![HostEvent::MapValueDeleted {
            room_id: "room-map".to_string(),
            namespace: "world".to_string(),
            key: "player1".to_string(),
            found: true,
        }]
    );
}

#[test]
fn map_set_with_empty_key_is_invalid_command() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-map", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let err = engine
        .apply(CommandEnvelope::new(
            "room-map",
            HostCommand::MapSet {
                namespace: "world".to_string(),
                key: String::new(),
                value: json!(true),
            },
        ))
        .expect_err("empty map key should fail");
    assert_eq!(err, HostCoreError::InvalidCommand);
}

#[test]
fn text_insert_get_delete_roundtrip_emits_expected_events() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-text", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let insert_a = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextInsert {
                namespace: "doc".to_string(),
                key: "title".to_string(),
                after_id: None,
                ch: "a".to_string(),
            },
        ))
        .expect("text insert should succeed");

    let first_id = match &insert_a.events[0] {
        HostEvent::TextValueInserted { id, .. } => id.clone(),
        other => panic!("unexpected event from first insert: {other:?}"),
    };

    let insert_b = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextInsert {
                namespace: "doc".to_string(),
                key: "title".to_string(),
                after_id: Some(first_id.clone()),
                ch: "b".to_string(),
            },
        ))
        .expect("second insert should succeed");

    let second_id = match &insert_b.events[0] {
        HostEvent::TextValueInserted { id, .. } => id.clone(),
        other => panic!("unexpected event from second insert: {other:?}"),
    };

    let read_before_delete = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextGet {
                namespace: "doc".to_string(),
                key: "title".to_string(),
            },
        ))
        .expect("text get should succeed");

    assert_eq!(read_before_delete.events.len(), 1);
    let HostEvent::TextValueRead { value, entries, .. } = &read_before_delete.events[0] else {
        panic!("expected TextValueRead event");
    };
    assert_eq!(value, "ab");
    assert_eq!(entries.len(), 2);

    let delete = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextDelete {
                namespace: "doc".to_string(),
                key: "title".to_string(),
                target_id: second_id,
            },
        ))
        .expect("text delete should succeed");

    assert_eq!(
        delete.events,
        vec![HostEvent::TextValueDeleted {
            room_id: "room-text".to_string(),
            namespace: "doc".to_string(),
            key: "title".to_string(),
            target_id: insert_b
                .events
                .iter()
                .find_map(|event| {
                    if let HostEvent::TextValueInserted { id, .. } = event {
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .expect("insert event should carry text id"),
            found: true,
        }]
    );

    let read_after_delete = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextGet {
                namespace: "doc".to_string(),
                key: "title".to_string(),
            },
        ))
        .expect("text get after delete should succeed");

    let HostEvent::TextValueRead { value, .. } = &read_after_delete.events[0] else {
        panic!("expected TextValueRead after delete");
    };
    assert_eq!(value, "a");

    let read_canonical = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextGetCanonical {
                namespace: "doc".to_string(),
                key: "title".to_string(),
            },
        ))
        .expect("canonical text get should succeed");

    let HostEvent::TextValueRead { value, .. } = &read_canonical.events[0] else {
        panic!("expected TextValueRead from canonical text get");
    };
    assert_eq!(value, "a");
}

#[test]
fn text_insert_with_invalid_after_id_is_invalid_command() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-text", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let err = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextInsert {
                namespace: "doc".to_string(),
                key: "title".to_string(),
                after_id: Some("missing-id".to_string()),
                ch: "z".to_string(),
            },
        ))
        .expect_err("missing anchor should fail");

    assert_eq!(err, HostCoreError::InvalidCommand);
}

#[test]
fn text_insert_requires_single_character_payload() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-text", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let err = engine
        .apply(CommandEnvelope::new(
            "room-text",
            HostCommand::TextInsert {
                namespace: "doc".to_string(),
                key: "title".to_string(),
                after_id: None,
                ch: "ab".to_string(),
            },
        ))
        .expect_err("multi-character payload should fail");

    assert_eq!(err, HostCoreError::InvalidCommand);
}

#[test]
fn list_push_insert_delete_get_roundtrip_emits_expected_events() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-list", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let push = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListPush {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                value: json!("a"),
            },
        ))
        .expect("list push should succeed");

    assert_eq!(push.events.len(), 1);
    let first_id = match &push.events[0] {
        HostEvent::ListValuePushed { id, index, .. } => {
            assert_eq!(*index, 0);
            id.clone()
        }
        other => panic!("expected ListValuePushed event, got {other:?}"),
    };

    let insert = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListInsert {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                index: 0,
                value: json!("b"),
            },
        ))
        .expect("list insert should succeed");

    assert_eq!(insert.events.len(), 1);
    let second_id = match &insert.events[0] {
        HostEvent::ListValueInserted { id, index, .. } => {
            assert_eq!(*index, 0);
            id.clone()
        }
        other => panic!("expected ListValueInserted event, got {other:?}"),
    };

    let get = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListGet {
                namespace: "doc".to_string(),
                key: "items".to_string(),
            },
        ))
        .expect("list get should succeed");

    let HostEvent::ListValueRead { entries, .. } = &get.events[0] else {
        panic!("expected ListValueRead event");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, second_id);
    assert_eq!(entries[0].value, json!("b"));
    assert_eq!(entries[1].id, first_id);
    assert_eq!(entries[1].value, json!("a"));

    let delete = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListDelete {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                index: 1,
            },
        ))
        .expect("list delete should succeed");

    assert_eq!(
        delete.events,
        vec![HostEvent::ListValueDeleted {
            room_id: "room-list".to_string(),
            namespace: "doc".to_string(),
            key: "items".to_string(),
            index: 1,
            found: true,
            removed: Some(json!("a")),
        }]
    );
}

#[test]
fn list_insert_out_of_range_is_invalid_command() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-list", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let err = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListInsert {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                index: 1,
                value: json!("x"),
            },
        ))
        .expect_err("out of range insert should fail");

    assert_eq!(err, HostCoreError::InvalidCommand);
}

#[test]
fn list_move_and_update_emit_expected_events_and_state() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-list", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    for value in [json!("a"), json!("b"), json!("c")] {
        engine
            .apply(CommandEnvelope::new(
                "room-list",
                HostCommand::ListPush {
                    namespace: "doc".to_string(),
                    key: "items".to_string(),
                    value,
                },
            ))
            .expect("list push should succeed");
    }

    let move_result = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListMove {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                from_index: 2,
                to_index: 0,
            },
        ))
        .expect("list move should succeed");

    let moved_id = match &move_result.events[0] {
        HostEvent::ListValueMoved {
            found,
            id,
            from_index,
            to_index,
            ..
        } => {
            assert!(*found);
            assert_eq!(*from_index, 2);
            assert_eq!(*to_index, 0);
            id.clone().expect("move should include moved id")
        }
        other => panic!("expected ListValueMoved event, got {other:?}"),
    };

    let update_result = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListUpdate {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                index: 1,
                value: json!("b-updated"),
            },
        ))
        .expect("list update should succeed");

    assert_eq!(
        update_result.events,
        vec![HostEvent::ListValueUpdated {
            room_id: "room-list".to_string(),
            namespace: "doc".to_string(),
            key: "items".to_string(),
            index: 1,
            found: true,
            id: Some("list-1".to_string()),
            value: Some(json!("b-updated")),
        }]
    );

    let get_result = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListGet {
                namespace: "doc".to_string(),
                key: "items".to_string(),
            },
        ))
        .expect("list get should succeed");

    let HostEvent::ListValueRead { entries, .. } = &get_result.events[0] else {
        panic!("expected ListValueRead event");
    };

    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].id, moved_id);
    assert_eq!(entries[0].value, json!("c"));
    assert_eq!(entries[1].id, "list-1");
    assert_eq!(entries[1].value, json!("b-updated"));
    assert_eq!(entries[2].id, "list-2");
    assert_eq!(entries[2].value, json!("b"));
}

#[test]
fn list_move_and_update_out_of_range_emit_not_found_ack() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-list", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let move_result = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListMove {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                from_index: 0,
                to_index: 1,
            },
        ))
        .expect("out of range move should not fail command");
    assert_eq!(
        move_result.events,
        vec![HostEvent::ListValueMoved {
            room_id: "room-list".to_string(),
            namespace: "doc".to_string(),
            key: "items".to_string(),
            from_index: 0,
            to_index: 1,
            found: false,
            id: None,
        }]
    );

    let update_result = engine
        .apply(CommandEnvelope::new(
            "room-list",
            HostCommand::ListUpdate {
                namespace: "doc".to_string(),
                key: "items".to_string(),
                index: 0,
                value: json!("x"),
            },
        ))
        .expect("out of range update should not fail command");
    assert_eq!(
        update_result.events,
        vec![HostEvent::ListValueUpdated {
            room_id: "room-list".to_string(),
            namespace: "doc".to_string(),
            key: "items".to_string(),
            index: 0,
            found: false,
            id: None,
            value: None,
        }]
    );
}

#[test]
fn blob_set_get_roundtrip_emits_expected_events() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let set_first = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobSet {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
                data_b64: "QUJD".to_string(),
            },
        ))
        .expect("blob set should succeed");

    assert_eq!(
        set_first.events,
        vec![HostEvent::BlobValueStored {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            stored: true,
        }]
    );

    let set_again = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobSet {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
                data_b64: "QUJD".to_string(),
            },
        ))
        .expect("blob set replay should succeed");

    assert_eq!(
        set_again.events,
        vec![HostEvent::BlobValueStored {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            stored: false,
        }]
    );

    let get_hit = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobGet {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
            },
        ))
        .expect("blob get should succeed");

    assert_eq!(
        get_hit.events,
        vec![HostEvent::BlobValueRead {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            found: true,
            data_b64: Some("QUJD".to_string()),
        }]
    );

    let get_miss = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobGet {
                namespace: "assets".to_string(),
                hash: "sha256:missing".to_string(),
            },
        ))
        .expect("missing blob get should succeed");

    assert_eq!(
        get_miss.events,
        vec![HostEvent::BlobValueRead {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:missing".to_string(),
            found: false,
            data_b64: None,
        }]
    );
}

#[test]
fn blob_set_requires_hash_and_data() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let err = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobSet {
                namespace: "assets".to_string(),
                hash: "".to_string(),
                data_b64: "QUJD".to_string(),
            },
        ))
        .expect_err("empty hash should fail");
    assert_eq!(err, HostCoreError::InvalidCommand);
}

#[test]
fn blob_get_many_returns_found_and_missing_hashes() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobSet {
                namespace: "assets".to_string(),
                hash: "sha256:a".to_string(),
                data_b64: "QQ==".to_string(),
            },
        ))
        .expect("blob set a should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobSet {
                namespace: "assets".to_string(),
                hash: "sha256:b".to_string(),
                data_b64: "Qg==".to_string(),
            },
        ))
        .expect("blob set b should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobGetMany {
                namespace: "assets".to_string(),
                hashes: vec![
                    "sha256:b".to_string(),
                    "sha256:missing".to_string(),
                    "sha256:a".to_string(),
                ],
            },
        ))
        .expect("blob get many should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::BlobValuesRead {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            entries: vec![
                crate::api::BlobEntry {
                    hash: "sha256:b".to_string(),
                    data_b64: "Qg==".to_string(),
                },
                crate::api::BlobEntry {
                    hash: "sha256:a".to_string(),
                    data_b64: "QQ==".to_string(),
                }
            ],
            missing: vec!["sha256:missing".to_string()],
        }]
    );
}

#[test]
fn request_upload_emits_upload_denied_when_callback_unconfigured() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::RequestUpload {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
                size_bytes: 128,
                content_type: Some("audio/aac".to_string()),
            },
        ))
        .expect("request upload should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::UploadDenied {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            reason: "use-ws".to_string(),
        }]
    );
}

#[test]
fn request_upload_requires_hash_and_positive_size() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let empty_hash_err = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::RequestUpload {
                namespace: "assets".to_string(),
                hash: String::new(),
                size_bytes: 128,
                content_type: None,
            },
        ))
        .expect_err("empty hash should fail");
    assert_eq!(empty_hash_err, HostCoreError::InvalidCommand);

    let zero_size_err = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::RequestUpload {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
                size_bytes: 0,
                content_type: None,
            },
        ))
        .expect_err("zero size should fail");
    assert_eq!(zero_size_err, HostCoreError::InvalidCommand);
}

#[test]
fn request_upload_emits_upload_granted_when_callback_configured() {
    let mut engine = HostEngine::new().with_blob_url_resolver(Arc::new(StaticBlobUrlResolver));
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::RequestUpload {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
                size_bytes: 128,
                content_type: Some("audio/aac".to_string()),
            },
        ))
        .expect("request upload should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::UploadGranted {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            url: "https://upload.example/sha256:abc".to_string(),
            expires_at_unix: 1_700_000_000,
        }]
    );
}

#[test]
fn blob_request_returns_blob_pack_when_redirect_callback_unconfigured() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobSet {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
                data_b64: "QUJD".to_string(),
            },
        ))
        .expect("blob set should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobRequest {
                namespace: "assets".to_string(),
                hashes: vec!["sha256:abc".to_string(), "sha256:missing".to_string()],
            },
        ))
        .expect("blob request should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::BlobPackPrepared {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            blobs: vec![crate::api::BlobEntry {
                hash: "sha256:abc".to_string(),
                data_b64: "QUJD".to_string(),
            }],
            requested: vec!["sha256:abc".to_string(), "sha256:missing".to_string()],
        }]
    );
}

#[test]
fn blob_request_returns_blob_redirect_when_callback_configured() {
    let mut engine = HostEngine::new().with_blob_url_resolver(Arc::new(StaticBlobUrlResolver));
    engine
        .apply(CommandEnvelope::new("room-blob", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobSet {
                namespace: "assets".to_string(),
                hash: "sha256:abc".to_string(),
                data_b64: "QUJD".to_string(),
            },
        ))
        .expect("blob set should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-blob",
            HostCommand::BlobRequest {
                namespace: "assets".to_string(),
                hashes: vec!["sha256:abc".to_string(), "sha256:missing".to_string()],
            },
        ))
        .expect("blob request should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::BlobRedirectPrepared {
            room_id: "room-blob".to_string(),
            namespace: "assets".to_string(),
            redirects: vec![crate::api::BlobRedirectEntry {
                hash: "sha256:abc".to_string(),
                url: "https://download.example/sha256:abc".to_string(),
                expires_at_unix: 1_700_000_123,
            }],
        }]
    );
}

#[test]
fn presence_set_update_get_and_leave_emit_expected_events() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-presence", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::OpenSession {
                session_id: 7,
                peer_pubkey_hex: "peer-7".to_string(),
            },
        ))
        .expect("open session should succeed");

    let join = engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::PresenceSet {
                session_id: 7,
                data: json!({ "state": "online" }),
                ttl_ms: None,
                now_unix_ms: None,
            },
        ))
        .expect("presence set should succeed");
    assert_eq!(
        join.events,
        vec![HostEvent::PresenceValueSet {
            room_id: "room-presence".to_string(),
            session_id: 7,
            from_peer_pubkey: "peer-7".to_string(),
            data: json!({ "state": "online" }),
            joined: true,
        }]
    );

    let update = engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::PresenceSet {
                session_id: 7,
                data: json!({ "state": "idle" }),
                ttl_ms: None,
                now_unix_ms: None,
            },
        ))
        .expect("presence update should succeed");
    assert_eq!(
        update.events,
        vec![HostEvent::PresenceValueSet {
            room_id: "room-presence".to_string(),
            session_id: 7,
            from_peer_pubkey: "peer-7".to_string(),
            data: json!({ "state": "idle" }),
            joined: false,
        }]
    );

    let listed = engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::PresenceGetAll,
        ))
        .expect("presence get all should succeed");
    assert_eq!(
        listed.events,
        vec![HostEvent::PresenceValuesListed {
            room_id: "room-presence".to_string(),
            entries: vec![crate::api::PresenceEntry {
                session_id: 7,
                from_peer_pubkey: "peer-7".to_string(),
                data: json!({ "state": "idle" }),
                expires_at_unix_ms: None,
            }],
        }]
    );

    let close = engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::CloseSession { session_id: 7 },
        ))
        .expect("close session should succeed");
    assert_eq!(
        close.events,
        vec![
            HostEvent::SessionClosed {
                room_id: "room-presence".to_string(),
                session_id: 7,
            },
            HostEvent::PresenceValueRemoved {
                room_id: "room-presence".to_string(),
                session_id: 7,
                from_peer_pubkey: "peer-7".to_string(),
                reason: "leave".to_string(),
            }
        ]
    );
}

#[test]
fn presence_sweep_removes_stale_entries() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-presence", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::OpenSession {
                session_id: 7,
                peer_pubkey_hex: "peer-7".to_string(),
            },
        ))
        .expect("open session should succeed");

    engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::PresenceSet {
                session_id: 7,
                data: json!({ "state": "online" }),
                ttl_ms: Some(5_000),
                now_unix_ms: Some(100_000),
            },
        ))
        .expect("presence set with ttl should succeed");

    let sweep_before_expiry = engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::PresenceSweep { now_unix_ms: 104_000 },
        ))
        .expect("presence sweep should succeed");
    assert!(sweep_before_expiry.events.is_empty());

    let sweep_after_expiry = engine
        .apply(CommandEnvelope::new(
            "room-presence",
            HostCommand::PresenceSweep { now_unix_ms: 105_000 },
        ))
        .expect("presence sweep should succeed");
    assert_eq!(
        sweep_after_expiry.events,
        vec![HostEvent::PresenceValueRemoved {
            room_id: "room-presence".to_string(),
            session_id: 7,
            from_peer_pubkey: "peer-7".to_string(),
            reason: "stale".to_string(),
        }]
    );
}

#[test]
fn subscribe_updates_patterns_for_session() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-sub", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-sub",
            HostCommand::OpenSession {
                session_id: 11,
                peer_pubkey_hex: "peer-11".to_string(),
            },
        ))
        .expect("open session should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-sub",
            HostCommand::Subscribe {
                session_id: 11,
                patterns: vec!["world/**".to_string(), "chat/*".to_string()],
            },
        ))
        .expect("subscribe should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::SubscriptionUpdated {
            room_id: "room-sub".to_string(),
            session_id: 11,
            patterns: vec!["world/**".to_string(), "chat/*".to_string()],
        }]
    );
}

#[test]
fn subscribe_with_empty_patterns_normalizes_to_everything() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-sub", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-sub",
            HostCommand::OpenSession {
                session_id: 12,
                peer_pubkey_hex: "peer-12".to_string(),
            },
        ))
        .expect("open session should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-sub",
            HostCommand::Subscribe {
                session_id: 12,
                patterns: vec![],
            },
        ))
        .expect("subscribe should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::SubscriptionUpdated {
            room_id: "room-sub".to_string(),
            session_id: 12,
            patterns: vec!["**".to_string()],
        }]
    );
}

#[test]
fn set_room_key_locks_room_and_rejects_relock() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-auth", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let lock = engine
        .apply(CommandEnvelope::new(
            "room-auth",
            HostCommand::SetRoomKey {
                pubkey_hex: "11".repeat(32),
            },
        ))
        .expect("set-room-key should succeed");
    assert_eq!(
        lock.events,
        vec![HostEvent::RoomLocked {
            room_id: "room-auth".to_string(),
            pubkey_hex: "11".repeat(32),
        }]
    );

    let relock = engine
        .apply(CommandEnvelope::new(
            "room-auth",
            HostCommand::SetRoomKey {
                pubkey_hex: "22".repeat(32),
            },
        ))
        .expect("re-lock should return rejection event");
    assert_eq!(
        relock.events,
        vec![HostEvent::SetRoomKeyRejected {
            room_id: "room-auth".to_string(),
            msg: "room already locked".to_string(),
        }]
    );
}

#[test]
fn client_hello_requires_valid_token_when_room_is_locked() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-auth", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let room_key = SigningKey::from_bytes(&[0x31u8; 32]);
    let peer_key = SigningKey::from_bytes(&[0x32u8; 32]);
    let peer_pubkey_hex = peer_key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>();

    engine
        .apply(CommandEnvelope::new(
            "room-auth",
            HostCommand::SetRoomKey {
                pubkey_hex: room_key.verifying_key().to_bytes().iter().map(|b| format!("{b:02x}")).collect(),
            },
        ))
        .expect("set-room-key should succeed");

    engine
        .apply(CommandEnvelope::new(
            "room-auth",
            HostCommand::OpenSession {
                session_id: 51,
                peer_pubkey_hex: peer_pubkey_hex.clone(),
            },
        ))
        .expect("open session should succeed");

    let missing_token_err = engine
        .apply(CommandEnvelope::new(
            "room-auth",
            HostCommand::ClientHello {
                session_id: 51,
                hello: ClientHelloPayload {
                    peer_pubkey_hex: peer_pubkey_hex.clone(),
                    client_frontier: vec![],
                    capabilities: CapabilitySet::default(),
                    token: None,
                },
            },
        ))
        .expect_err("locked room hello without token should fail");
    assert_eq!(missing_token_err, HostCoreError::AuthViolation);

    let token = RoomToken::sign(
        "room-auth",
        &peer_key.verifying_key().to_bytes(),
        u64::MAX,
        &[],
        &room_key,
    );

    let accepted = engine
        .apply(CommandEnvelope::new(
            "room-auth",
            HostCommand::ClientHello {
                session_id: 51,
                hello: ClientHelloPayload {
                    peer_pubkey_hex,
                    client_frontier: vec![],
                    capabilities: CapabilitySet::default(),
                    token: Some(crate::api::HelloTokenPayload {
                        peer_pubkey: token.peer_pubkey_hex(),
                        expiry: token.expiry_secs,
                        caps: token.capabilities.clone(),
                        sig: token.sig_hex(),
                    }),
                },
            },
        ))
        .expect("locked room hello with valid token should succeed");

    assert_eq!(accepted.events.len(), 1);
    assert!(matches!(accepted.events[0], HostEvent::WelcomePrepared { .. }));
}

#[test]
fn set_policy_accepts_valid_default_and_rules() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-policy", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-policy",
            HostCommand::SetPolicy {
                default: "deny".to_string(),
                rules: vec![PolicyRulePayload {
                    path_glob: "world/**".to_string(),
                    can_write: vec!["11".repeat(32)],
                }],
            },
        ))
        .expect("set-policy should succeed");

    assert_eq!(
        result.events,
        vec![HostEvent::PolicySet {
            room_id: "room-policy".to_string(),
        }]
    );
}

#[test]
fn set_policy_rejects_unknown_default() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-policy", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-policy",
            HostCommand::SetPolicy {
                default: "custom".to_string(),
                rules: vec![],
            },
        ))
        .expect("set-policy should return rejection event");

    assert_eq!(
        result.events,
        vec![HostEvent::SetPolicyRejected {
            room_id: "room-policy".to_string(),
            msg: "set-policy: unknown default 'custom', use 'allow' or 'deny'".to_string(),
        }]
    );
}

#[test]
fn set_policy_rejects_invalid_can_write_pubkey_hex() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-policy", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let result = engine
        .apply(CommandEnvelope::new(
            "room-policy",
            HostCommand::SetPolicy {
                default: "allow".to_string(),
                rules: vec![PolicyRulePayload {
                    path_glob: "world/**".to_string(),
                    can_write: vec!["badhex".to_string()],
                }],
            },
        ))
        .expect("set-policy should return rejection event");

    assert_eq!(
        result.events,
        vec![HostEvent::SetPolicyRejected {
            room_id: "room-policy".to_string(),
            msg: "set-policy: invalid pubkey hex 'badhex'".to_string(),
        }]
    );
}

fn signed_single_node_pack_b64() -> String {
    let mut graph = StateGraph::new();
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let id = graph
        .apply_local(
            &signing_key,
            1,
            vec![Op::Map(MapOp::Set {
                key: "world/greeting".to_string(),
                value: b"hello".to_vec(),
            })],
        )
        .expect("local apply should succeed");
    let nodes = graph.get_nodes(&[id]);
    shape_catchup_pack_payload_b64(&nodes)
}

fn signed_map_set_pack_b64(seed: u8, key: &str, value: &[u8]) -> String {
    let mut graph = StateGraph::new();
    let signing_key = SigningKey::from_bytes(&[seed; 32]);
    let id = graph
        .apply_local(
            &signing_key,
            1,
            vec![Op::Map(MapOp::Set {
                key: key.to_string(),
                value: value.to_vec(),
            })],
        )
        .expect("local apply should succeed");
    let nodes = graph.get_nodes(&[id]);
    shape_catchup_pack_payload_b64(&nodes)
}

#[test]
fn import_pack_and_request_server_pack_roundtrip() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-sync", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let import = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::ImportPack {
                nodes_b64: signed_single_node_pack_b64(),
            },
        ))
        .expect("import-pack should succeed");

    assert_eq!(import.events.len(), 1);
    assert!(matches!(
        &import.events[0],
        HostEvent::PackImported {
            incoming_count: 1,
            accepted_count: 1,
            rejected_count: 0,
            ..
        }
    ));

    let request = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::RequestServerPack {
                known_ids: vec![],
            },
        ))
        .expect("request-server-pack should succeed");

    assert_eq!(request.events.len(), 1);
    assert!(matches!(
        &request.events[0],
        HostEvent::ServerPackPrepared { nodes_b64, .. } if !nodes_b64.is_empty()
    ));
}

#[test]
fn mst_request_and_mst_done_emit_sync_events() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-sync", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::ImportPack {
                nodes_b64: signed_single_node_pack_b64(),
            },
        ))
        .expect("import-pack should succeed");

    let mst_request = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::MstRequest {
                paths: vec!["".to_string()],
            },
        ))
        .expect("mst-request should succeed");

    assert!(matches!(
        &mst_request.events[0],
        HostEvent::MstResponsePrepared { nodes, .. } if !nodes.is_empty()
    ));

    let mst_nodes = match &mst_request.events[0] {
        HostEvent::MstResponsePrepared { nodes, .. } => nodes,
        _ => panic!("expected mst response event"),
    };
    let first_key_hex = mst_nodes[0]["keys"][0]
        .as_str()
        .expect("mst key hex should be present")
        .to_string();

    let mst_done = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::MstDone {
                ids: vec![first_key_hex],
            },
        ))
        .expect("mst-done should succeed");

    assert!(matches!(
        &mst_done.events[0],
        HostEvent::ServerPackPrepared { nodes_b64, .. } if !nodes_b64.is_empty()
    ));
}

#[test]
fn import_pack_emits_conflicts_observed_and_recent_conflicts() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-sync", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::ImportPack {
                nodes_b64: signed_map_set_pack_b64(11, "world/greeting", b"hello"),
            },
        ))
        .expect("first import-pack should succeed");

    let second_import = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::ImportPack {
                nodes_b64: signed_map_set_pack_b64(12, "world/greeting", b"hola"),
            },
        ))
        .expect("second import-pack should succeed");

    assert_eq!(second_import.events.len(), 2);
    assert!(matches!(
        &second_import.events[0],
        HostEvent::PackImported { .. }
    ));
    assert!(matches!(
        &second_import.events[1],
        HostEvent::ConflictsObserved { entries, .. } if !entries.is_empty()
    ));

    let listed = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::GetRecentConflicts { since_unix_ms: None },
        ))
        .expect("recent conflicts should succeed");
    assert!(matches!(
        &listed.events[0],
        HostEvent::RecentConflictsListed { entries, .. } if !entries.is_empty()
    ));
}

#[test]
fn import_pack_dedups_conflict_events_by_fingerprint() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-sync", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let first_pack = signed_map_set_pack_b64(21, "world/title", b"a");
    let second_pack = signed_map_set_pack_b64(22, "world/title", b"b");

    engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::ImportPack {
                nodes_b64: first_pack,
            },
        ))
        .expect("first import should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::ImportPack {
                nodes_b64: second_pack.clone(),
            },
        ))
        .expect("second import should succeed");

    let replay = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::ImportPack {
                nodes_b64: second_pack,
            },
        ))
        .expect("replay import should succeed");

    assert!(matches!(
        &replay.events[0],
        HostEvent::PackImported { .. }
    ));
    assert_eq!(replay.events.len(), 1);

    let filtered = engine
        .apply(CommandEnvelope::new(
            "room-sync",
            HostCommand::GetRecentConflicts {
                since_unix_ms: Some(u64::MAX),
            },
        ))
        .expect("recent conflicts filter should succeed");
    assert!(matches!(
        &filtered.events[0],
        HostEvent::RecentConflictsListed { entries, .. } if entries.is_empty()
    ));
}

#[test]
fn relay_peer_signal_emits_peer_signal_relayed_event() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-relay", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");
    engine
        .apply(CommandEnvelope::new(
            "room-relay",
            HostCommand::OpenSession {
                session_id: 7,
                peer_pubkey_hex: "peer-a".to_string(),
            },
        ))
        .expect("open session should succeed");

    let relayed = engine
        .apply(CommandEnvelope::new(
            "room-relay",
            HostCommand::RelayPeerSignal {
                session_id: 7,
                msg_type: "webrtc-offer".to_string(),
                to_peer_pubkey: "peer-b".to_string(),
                payload: serde_json::json!({"sdp": {"type": "offer", "sdp": "v=0"}}),
            },
        ))
        .expect("relay command should succeed");

    assert_eq!(
        relayed.events,
        vec![HostEvent::PeerSignalRelayed {
            room_id: "room-relay".to_string(),
            from_peer_pubkey: "peer-a".to_string(),
            msg_type: "webrtc-offer".to_string(),
            to_peer_pubkey: "peer-b".to_string(),
            payload: serde_json::json!({"sdp": {"type": "offer", "sdp": "v=0"}}),
        }]
    );
}

#[test]
fn relay_peer_signal_rejects_missing_session_or_empty_target() {
    let mut engine = HostEngine::new();
    engine
        .apply(CommandEnvelope::new("room-relay", HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    let missing_session = engine
        .apply(CommandEnvelope::new(
            "room-relay",
            HostCommand::RelayPeerSignal {
                session_id: 99,
                msg_type: "webrtc-ice".to_string(),
                to_peer_pubkey: "peer-b".to_string(),
                payload: serde_json::json!({"candidate": {}}),
            },
        ))
        .expect_err("missing session should fail");
    assert_eq!(missing_session, HostCoreError::SessionNotFound);

    engine
        .apply(CommandEnvelope::new(
            "room-relay",
            HostCommand::OpenSession {
                session_id: 9,
                peer_pubkey_hex: "peer-a".to_string(),
            },
        ))
        .expect("open session should succeed");

    let invalid_target = engine
        .apply(CommandEnvelope::new(
            "room-relay",
            HostCommand::RelayPeerSignal {
                session_id: 9,
                msg_type: "webrtc-answer".to_string(),
                to_peer_pubkey: "".to_string(),
                payload: serde_json::json!({"sdp": {}}),
            },
        ))
        .expect_err("empty target should fail");
    assert_eq!(invalid_target, HostCoreError::InvalidCommand);
}
