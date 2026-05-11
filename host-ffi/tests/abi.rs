use activesync_core::{MapOp, Op, StateGraph};
use activesync_host_core::api::{CapabilitySet, ClientHelloPayload, HostCommand, HostEvent};
use activesync_host_core::engine::shape_catchup_pack_payload_b64;
use activesync_host_ffi::{
    as_bytes_owned, as_bytes_owned_free, as_bytes_view, as_host_abi_version, as_host_engine,
    as_host_engine_free, as_host_engine_new, as_host_submit_command, as_host_submit_command_json,
    as_status,
};
use serde::Serialize;
use ed25519_dalek::SigningKey;

#[derive(Debug, Clone, Serialize)]
struct FfiCommandEnvelope {
    room_id: String,
    command: HostCommand,
}

fn encode_command(room_id: &str, command: HostCommand) -> Vec<u8> {
    let envelope = FfiCommandEnvelope {
        room_id: room_id.to_string(),
        command,
    };
    postcard::to_allocvec(&envelope).expect("encode command")
}

fn decode_events(bytes: &[u8]) -> Vec<HostEvent> {
    postcard::from_bytes(bytes).expect("decode events")
}

fn decode_events_json(bytes: &[u8]) -> Vec<HostEvent> {
    serde_json::from_slice(bytes).expect("decode json events")
}

#[test]
fn abi_version_is_v1() {
    assert_eq!(as_host_abi_version(), 1);
}

#[test]
fn engine_new_rejects_null_output_pointer() {
    let status = unsafe { as_host_engine_new(std::ptr::null_mut()) };
    assert_eq!(status, as_status::AS_ERR_INVALID_ARG);
}

#[test]
fn submit_rejects_malformed_payload() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let mut out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let malformed = [0xFFu8, 0xAB, 0x10];
    let status = unsafe {
        as_host_submit_command(
            engine,
            as_bytes_view {
                ptr: malformed.as_ptr(),
                len: malformed.len(),
            },
            &mut out,
        )
    };
    assert_eq!(status, as_status::AS_ERR_INVALID_ARG);

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_returns_postcard_event_payload_and_buffer_can_be_freed() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let command_bytes = encode_command("room-ffi", HostCommand::EnsureRoom);
    let mut out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let status = unsafe {
        as_host_submit_command(
            engine,
            as_bytes_view {
                ptr: command_bytes.as_ptr(),
                len: command_bytes.len(),
            },
            &mut out,
        )
    };
    assert_eq!(status, as_status::AS_OK);
    assert!(!out.ptr.is_null());
    assert!(out.len > 0);

    let out_slice = unsafe { std::slice::from_raw_parts(out.ptr, out.len) };
    let events = decode_events(out_slice);
    assert_eq!(
        events,
        vec![HostEvent::RoomEnsured {
            room_id: "room-ffi".to_string(),
        }]
    );

    unsafe { as_bytes_owned_free(out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_drives_host_core_lifecycle_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_room = encode_command("room-lifecycle", HostCommand::EnsureRoom);
    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command(
            engine,
            as_bytes_view {
                ptr: ensure_room.as_ptr(),
                len: ensure_room.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let open_session = encode_command(
        "room-lifecycle",
        HostCommand::OpenSession {
            session_id: 77,
            peer_pubkey_hex: "peer-77".to_string(),
        },
    );
    let mut open_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let open_status = unsafe {
        as_host_submit_command(
            engine,
            as_bytes_view {
                ptr: open_session.as_ptr(),
                len: open_session.len(),
            },
            &mut open_out,
        )
    };
    assert_eq!(open_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(open_out) };

    let hello = encode_command(
        "room-lifecycle",
        HostCommand::ClientHello {
            session_id: 77,
            hello: ClientHelloPayload {
                peer_pubkey_hex: "peer-77".to_string(),
                client_frontier: vec![],
                capabilities: CapabilitySet {
                    supports_ibf: true,
                    supports_mst: false,
                },
                token: None,
            },
        },
    );
    let mut hello_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let hello_status = unsafe {
        as_host_submit_command(
            engine,
            as_bytes_view {
                ptr: hello.as_ptr(),
                len: hello.len(),
            },
            &mut hello_out,
        )
    };
    assert_eq!(hello_status, as_status::AS_OK);

    let hello_events = decode_events(unsafe { std::slice::from_raw_parts(hello_out.ptr, hello_out.len) });
    assert_eq!(
        hello_events,
        vec![HostEvent::WelcomePrepared {
            room_id: "room-lifecycle".to_string(),
            session_id: 77,
            negotiated: CapabilitySet {
                supports_ibf: true,
                supports_mst: false,
            },
            missing_from_server_count: 0,
        }]
    );

    unsafe { as_bytes_owned_free(hello_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_accepts_typed_envelope_and_returns_json_events() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let json = serde_json::json!({
        "room_id": "room-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: json.as_ptr(),
                len: json.len(),
            },
            &mut out,
        )
    };
    assert_eq!(status, as_status::AS_OK);

    let out_slice = unsafe { std::slice::from_raw_parts(out.ptr, out.len) };
    let events = decode_events_json(out_slice);
    assert_eq!(
        events,
        vec![HostEvent::RoomEnsured {
            room_id: "room-json".to_string(),
        }]
    );

    unsafe { as_bytes_owned_free(out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_map_crud_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-map-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let set_json = serde_json::json!({
        "room_id": "room-map-json",
        "command": {
            "MapSet": {
                "namespace": "world",
                "key": "player1",
                "value": { "x": 10, "y": 20 }
            }
        }
    })
    .to_string();

    let mut set_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let set_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: set_json.as_ptr(),
                len: set_json.len(),
            },
            &mut set_out,
        )
    };
    assert_eq!(set_status, as_status::AS_OK);

    let set_events = decode_events_json(unsafe { std::slice::from_raw_parts(set_out.ptr, set_out.len) });
    assert_eq!(set_events.len(), 1);
    unsafe { as_bytes_owned_free(set_out) };

    let get_json = serde_json::json!({
        "room_id": "room-map-json",
        "command": {
            "MapGet": {
                "namespace": "world",
                "key": "player1"
            }
        }
    })
    .to_string();

    let mut get_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let get_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: get_json.as_ptr(),
                len: get_json.len(),
            },
            &mut get_out,
        )
    };
    assert_eq!(get_status, as_status::AS_OK);

    let get_events = decode_events_json(unsafe { std::slice::from_raw_parts(get_out.ptr, get_out.len) });
    assert_eq!(get_events.len(), 1);
    let HostEvent::MapValueRead { value, .. } = &get_events[0] else {
        panic!("expected MapValueRead event");
    };
    assert_eq!(value.as_ref().and_then(|v| v.get("x")).and_then(|x| x.as_i64()), Some(10));
    unsafe { as_bytes_owned_free(get_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_text_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-text-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let insert_json = serde_json::json!({
        "room_id": "room-text-json",
        "command": {
            "TextInsert": {
                "namespace": "doc",
                "key": "title",
                "after_id": null,
                "ch": "a"
            }
        }
    })
    .to_string();

    let mut insert_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let insert_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: insert_json.as_ptr(),
                len: insert_json.len(),
            },
            &mut insert_out,
        )
    };
    assert_eq!(insert_status, as_status::AS_OK);

    let insert_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(insert_out.ptr, insert_out.len)
    });
    assert_eq!(insert_events.len(), 1);
    let inserted_id = match &insert_events[0] {
        HostEvent::TextValueInserted { id, .. } => id.clone(),
        other => panic!("expected TextValueInserted event, got {other:?}"),
    };
    unsafe { as_bytes_owned_free(insert_out) };

    let get_json = serde_json::json!({
        "room_id": "room-text-json",
        "command": {
            "TextGet": {
                "namespace": "doc",
                "key": "title"
            }
        }
    })
    .to_string();

    let mut get_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let get_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: get_json.as_ptr(),
                len: get_json.len(),
            },
            &mut get_out,
        )
    };
    assert_eq!(get_status, as_status::AS_OK);

    let get_events = decode_events_json(unsafe { std::slice::from_raw_parts(get_out.ptr, get_out.len) });
    assert_eq!(get_events.len(), 1);
    let HostEvent::TextValueRead { value, entries, .. } = &get_events[0] else {
        panic!("expected TextValueRead event");
    };
    assert_eq!(value, "a");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, inserted_id);
    unsafe { as_bytes_owned_free(get_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_list_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-list-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let push_json = serde_json::json!({
        "room_id": "room-list-json",
        "command": {
            "ListPush": {
                "namespace": "doc",
                "key": "items",
                "value": "a"
            }
        }
    })
    .to_string();

    let mut push_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let push_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: push_json.as_ptr(),
                len: push_json.len(),
            },
            &mut push_out,
        )
    };
    assert_eq!(push_status, as_status::AS_OK);

    let push_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(push_out.ptr, push_out.len)
    });
    assert_eq!(push_events.len(), 1);
    let inserted_id = match &push_events[0] {
        HostEvent::ListValuePushed { id, index, .. } => {
            assert_eq!(*index, 0);
            id.clone()
        }
        other => panic!("expected ListValuePushed event, got {other:?}"),
    };
    unsafe { as_bytes_owned_free(push_out) };

    let insert_json = serde_json::json!({
        "room_id": "room-list-json",
        "command": {
            "ListInsert": {
                "namespace": "doc",
                "key": "items",
                "index": 0,
                "value": "b"
            }
        }
    })
    .to_string();

    let mut insert_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let insert_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: insert_json.as_ptr(),
                len: insert_json.len(),
            },
            &mut insert_out,
        )
    };
    assert_eq!(insert_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(insert_out) };

    let get_json = serde_json::json!({
        "room_id": "room-list-json",
        "command": {
            "ListGet": {
                "namespace": "doc",
                "key": "items"
            }
        }
    })
    .to_string();

    let mut get_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let get_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: get_json.as_ptr(),
                len: get_json.len(),
            },
            &mut get_out,
        )
    };
    assert_eq!(get_status, as_status::AS_OK);

    let get_events = decode_events_json(unsafe { std::slice::from_raw_parts(get_out.ptr, get_out.len) });
    assert_eq!(get_events.len(), 1);
    let HostEvent::ListValueRead { entries, .. } = &get_events[0] else {
        panic!("expected ListValueRead event");
    };
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].id, inserted_id);
    unsafe { as_bytes_owned_free(get_out) };

    let delete_json = serde_json::json!({
        "room_id": "room-list-json",
        "command": {
            "ListDelete": {
                "namespace": "doc",
                "key": "items",
                "index": 1
            }
        }
    })
    .to_string();

    let mut delete_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let delete_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: delete_json.as_ptr(),
                len: delete_json.len(),
            },
            &mut delete_out,
        )
    };
    assert_eq!(delete_status, as_status::AS_OK);

    let delete_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(delete_out.ptr, delete_out.len)
    });
    assert_eq!(delete_events.len(), 1);
    let HostEvent::ListValueDeleted {
        found, removed, ..
    } = &delete_events[0]
    else {
        panic!("expected ListValueDeleted event");
    };
    assert!(*found);
    assert_eq!(removed.as_ref().and_then(|v| v.as_str()), Some("a"));
    unsafe { as_bytes_owned_free(delete_out) };

    let move_json = serde_json::json!({
        "room_id": "room-list-json",
        "command": {
            "ListMove": {
                "namespace": "doc",
                "key": "items",
                "from_index": 0,
                "to_index": 0
            }
        }
    })
    .to_string();

    let mut move_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let move_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: move_json.as_ptr(),
                len: move_json.len(),
            },
            &mut move_out,
        )
    };
    assert_eq!(move_status, as_status::AS_OK);

    let move_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(move_out.ptr, move_out.len)
    });
    assert_eq!(move_events.len(), 1);
    let HostEvent::ListValueMoved {
        found,
        id,
        from_index,
        to_index,
        ..
    } = &move_events[0]
    else {
        panic!("expected ListValueMoved event");
    };
    assert!(*found);
    assert_eq!(*from_index, 0);
    assert_eq!(*to_index, 0);
    assert_eq!(id.as_deref(), Some("list-2"));
    unsafe { as_bytes_owned_free(move_out) };

    let update_json = serde_json::json!({
        "room_id": "room-list-json",
        "command": {
            "ListUpdate": {
                "namespace": "doc",
                "key": "items",
                "index": 0,
                "value": "b-updated"
            }
        }
    })
    .to_string();

    let mut update_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let update_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: update_json.as_ptr(),
                len: update_json.len(),
            },
            &mut update_out,
        )
    };
    assert_eq!(update_status, as_status::AS_OK);

    let update_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(update_out.ptr, update_out.len)
    });
    assert_eq!(update_events.len(), 1);
    let HostEvent::ListValueUpdated {
        found,
        id,
        value,
        ..
    } = &update_events[0]
    else {
        panic!("expected ListValueUpdated event");
    };
    assert!(*found);
    assert_eq!(id.as_deref(), Some("list-2"));
    assert_eq!(value.as_ref().and_then(|v| v.as_str()), Some("b-updated"));
    unsafe { as_bytes_owned_free(update_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_blob_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-blob-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let set_json = serde_json::json!({
        "room_id": "room-blob-json",
        "command": {
            "BlobSet": {
                "namespace": "assets",
                "hash": "sha256:abc",
                "data_b64": "QUJD"
            }
        }
    })
    .to_string();

    let mut set_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let set_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: set_json.as_ptr(),
                len: set_json.len(),
            },
            &mut set_out,
        )
    };
    assert_eq!(set_status, as_status::AS_OK);

    let set_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(set_out.ptr, set_out.len)
    });
    assert_eq!(
        set_events,
        vec![HostEvent::BlobValueStored {
            room_id: "room-blob-json".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            stored: true,
        }]
    );
    unsafe { as_bytes_owned_free(set_out) };

    let get_hit_json = serde_json::json!({
        "room_id": "room-blob-json",
        "command": {
            "BlobGet": {
                "namespace": "assets",
                "hash": "sha256:abc"
            }
        }
    })
    .to_string();

    let mut get_hit_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let get_hit_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: get_hit_json.as_ptr(),
                len: get_hit_json.len(),
            },
            &mut get_hit_out,
        )
    };
    assert_eq!(get_hit_status, as_status::AS_OK);

    let get_hit_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(get_hit_out.ptr, get_hit_out.len)
    });
    assert_eq!(
        get_hit_events,
        vec![HostEvent::BlobValueRead {
            room_id: "room-blob-json".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            found: true,
            data_b64: Some("QUJD".to_string()),
        }]
    );
    unsafe { as_bytes_owned_free(get_hit_out) };

    let get_miss_json = serde_json::json!({
        "room_id": "room-blob-json",
        "command": {
            "BlobGet": {
                "namespace": "assets",
                "hash": "sha256:missing"
            }
        }
    })
    .to_string();

    let mut get_miss_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let get_miss_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: get_miss_json.as_ptr(),
                len: get_miss_json.len(),
            },
            &mut get_miss_out,
        )
    };
    assert_eq!(get_miss_status, as_status::AS_OK);

    let get_miss_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(get_miss_out.ptr, get_miss_out.len)
    });
    assert_eq!(
        get_miss_events,
        vec![HostEvent::BlobValueRead {
            room_id: "room-blob-json".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:missing".to_string(),
            found: false,
            data_b64: None,
        }]
    );
    unsafe { as_bytes_owned_free(get_miss_out) };

    let get_many_json = serde_json::json!({
        "room_id": "room-blob-json",
        "command": {
            "BlobGetMany": {
                "namespace": "assets",
                "hashes": ["sha256:abc", "sha256:missing"]
            }
        }
    })
    .to_string();

    let mut get_many_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let get_many_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: get_many_json.as_ptr(),
                len: get_many_json.len(),
            },
            &mut get_many_out,
        )
    };
    assert_eq!(get_many_status, as_status::AS_OK);

    let get_many_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(get_many_out.ptr, get_many_out.len)
    });
    assert_eq!(
        get_many_events,
        vec![HostEvent::BlobValuesRead {
            room_id: "room-blob-json".to_string(),
            namespace: "assets".to_string(),
            entries: vec![activesync_host_core::api::BlobEntry {
                hash: "sha256:abc".to_string(),
                data_b64: "QUJD".to_string(),
            }],
            missing: vec!["sha256:missing".to_string()],
        }]
    );
    unsafe { as_bytes_owned_free(get_many_out) };

    let request_upload_json = serde_json::json!({
        "room_id": "room-blob-json",
        "command": {
            "RequestUpload": {
                "namespace": "assets",
                "hash": "sha256:abc",
                "size_bytes": 1024,
                "content_type": "audio/aac"
            }
        }
    })
    .to_string();

    let mut request_upload_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let request_upload_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: request_upload_json.as_ptr(),
                len: request_upload_json.len(),
            },
            &mut request_upload_out,
        )
    };
    assert_eq!(request_upload_status, as_status::AS_OK);

    let request_upload_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(request_upload_out.ptr, request_upload_out.len)
    });
    assert_eq!(
        request_upload_events,
        vec![HostEvent::UploadDenied {
            room_id: "room-blob-json".to_string(),
            namespace: "assets".to_string(),
            hash: "sha256:abc".to_string(),
            reason: "use-ws".to_string(),
        }]
    );
    unsafe { as_bytes_owned_free(request_upload_out) };

    let blob_request_json = serde_json::json!({
        "room_id": "room-blob-json",
        "command": {
            "BlobRequest": {
                "namespace": "assets",
                "hashes": ["sha256:abc", "sha256:missing"]
            }
        }
    })
    .to_string();

    let mut blob_request_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };

    let blob_request_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: blob_request_json.as_ptr(),
                len: blob_request_json.len(),
            },
            &mut blob_request_out,
        )
    };
    assert_eq!(blob_request_status, as_status::AS_OK);

    let blob_request_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(blob_request_out.ptr, blob_request_out.len)
    });
    assert_eq!(
        blob_request_events,
        vec![HostEvent::BlobPackPrepared {
            room_id: "room-blob-json".to_string(),
            namespace: "assets".to_string(),
            blobs: vec![activesync_host_core::api::BlobEntry {
                hash: "sha256:abc".to_string(),
                data_b64: "QUJD".to_string(),
            }],
            requested: vec!["sha256:abc".to_string(), "sha256:missing".to_string()],
        }]
    );
    unsafe { as_bytes_owned_free(blob_request_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_presence_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-presence-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let open_json = serde_json::json!({
        "room_id": "room-presence-json",
        "command": {
            "OpenSession": {
                "session_id": 7,
                "peer_pubkey_hex": "peer-7"
            }
        }
    })
    .to_string();

    let mut open_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let open_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: open_json.as_ptr(),
                len: open_json.len(),
            },
            &mut open_out,
        )
    };
    assert_eq!(open_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(open_out) };

    let set_json = serde_json::json!({
        "room_id": "room-presence-json",
        "command": {
            "PresenceSet": {
                "session_id": 7,
                "data": { "state": "online" },
                "ttl_ms": 5000,
                "now_unix_ms": 100000
            }
        }
    })
    .to_string();

    let mut set_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let set_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: set_json.as_ptr(),
                len: set_json.len(),
            },
            &mut set_out,
        )
    };
    assert_eq!(set_status, as_status::AS_OK);
    let set_events = decode_events_json(unsafe { std::slice::from_raw_parts(set_out.ptr, set_out.len) });
    assert_eq!(
        set_events,
        vec![HostEvent::PresenceValueSet {
            room_id: "room-presence-json".to_string(),
            session_id: 7,
            from_peer_pubkey: "peer-7".to_string(),
            data: serde_json::json!({ "state": "online" }),
            joined: true,
        }]
    );
    unsafe { as_bytes_owned_free(set_out) };

    let get_json = serde_json::json!({
        "room_id": "room-presence-json",
        "command": "PresenceGetAll"
    })
    .to_string();

    let mut get_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let get_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: get_json.as_ptr(),
                len: get_json.len(),
            },
            &mut get_out,
        )
    };
    assert_eq!(get_status, as_status::AS_OK);
    let get_events = decode_events_json(unsafe { std::slice::from_raw_parts(get_out.ptr, get_out.len) });
    assert_eq!(get_events.len(), 1);
    let HostEvent::PresenceValuesListed { entries, .. } = &get_events[0] else {
        panic!("expected PresenceValuesListed");
    };
    assert_eq!(entries.len(), 1);
    unsafe { as_bytes_owned_free(get_out) };

    let sweep_json = serde_json::json!({
        "room_id": "room-presence-json",
        "command": {
            "PresenceSweep": {
                "now_unix_ms": 105000
            }
        }
    })
    .to_string();

    let mut sweep_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let sweep_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: sweep_json.as_ptr(),
                len: sweep_json.len(),
            },
            &mut sweep_out,
        )
    };
    assert_eq!(sweep_status, as_status::AS_OK);
    let sweep_events = decode_events_json(unsafe { std::slice::from_raw_parts(sweep_out.ptr, sweep_out.len) });
    assert_eq!(
        sweep_events,
        vec![HostEvent::PresenceValueRemoved {
            room_id: "room-presence-json".to_string(),
            session_id: 7,
            from_peer_pubkey: "peer-7".to_string(),
            reason: "stale".to_string(),
        }]
    );
    unsafe { as_bytes_owned_free(sweep_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_subscription_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-sub-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let open_json = serde_json::json!({
        "room_id": "room-sub-json",
        "command": {
            "OpenSession": {
                "session_id": 17,
                "peer_pubkey_hex": "peer-17"
            }
        }
    })
    .to_string();

    let mut open_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let open_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: open_json.as_ptr(),
                len: open_json.len(),
            },
            &mut open_out,
        )
    };
    assert_eq!(open_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(open_out) };

    let subscribe_json = serde_json::json!({
        "room_id": "room-sub-json",
        "command": {
            "Subscribe": {
                "session_id": 17,
                "patterns": ["world/**", "chat/*"]
            }
        }
    })
    .to_string();

    let mut subscribe_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let subscribe_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: subscribe_json.as_ptr(),
                len: subscribe_json.len(),
            },
            &mut subscribe_out,
        )
    };
    assert_eq!(subscribe_status, as_status::AS_OK);
    let subscribe_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(subscribe_out.ptr, subscribe_out.len)
    });
    assert_eq!(
        subscribe_events,
        vec![HostEvent::SubscriptionUpdated {
            room_id: "room-sub-json".to_string(),
            session_id: 17,
            patterns: vec!["world/**".to_string(), "chat/*".to_string()],
        }]
    );
    unsafe { as_bytes_owned_free(subscribe_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_set_room_key_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-lock-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let lock_json = serde_json::json!({
        "room_id": "room-lock-json",
        "command": {
            "SetRoomKey": {
                "pubkey_hex": "11".repeat(32)
            }
        }
    })
    .to_string();

    let mut lock_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let lock_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: lock_json.as_ptr(),
                len: lock_json.len(),
            },
            &mut lock_out,
        )
    };
    assert_eq!(lock_status, as_status::AS_OK);
    let lock_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(lock_out.ptr, lock_out.len)
    });
    assert_eq!(
        lock_events,
        vec![HostEvent::RoomLocked {
            room_id: "room-lock-json".to_string(),
            pubkey_hex: "11".repeat(32),
        }]
    );
    unsafe { as_bytes_owned_free(lock_out) };

    let relock_json = serde_json::json!({
        "room_id": "room-lock-json",
        "command": {
            "SetRoomKey": {
                "pubkey_hex": "22".repeat(32)
            }
        }
    })
    .to_string();

    let mut relock_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let relock_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: relock_json.as_ptr(),
                len: relock_json.len(),
            },
            &mut relock_out,
        )
    };
    assert_eq!(relock_status, as_status::AS_OK);
    let relock_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(relock_out.ptr, relock_out.len)
    });
    assert_eq!(
        relock_events,
        vec![HostEvent::SetRoomKeyRejected {
            room_id: "room-lock-json".to_string(),
            msg: "room already locked".to_string(),
        }]
    );
    unsafe { as_bytes_owned_free(relock_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_set_policy_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-policy-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let set_policy_json = serde_json::json!({
        "room_id": "room-policy-json",
        "command": {
            "SetPolicy": {
                "default": "deny",
                "rules": [
                    {
                        "path_glob": "world/**",
                        "can_write": ["11".repeat(32)]
                    }
                ]
            }
        }
    })
    .to_string();

    let mut set_policy_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let set_policy_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: set_policy_json.as_ptr(),
                len: set_policy_json.len(),
            },
            &mut set_policy_out,
        )
    };
    assert_eq!(set_policy_status, as_status::AS_OK);
    let set_policy_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(set_policy_out.ptr, set_policy_out.len)
    });
    assert_eq!(
        set_policy_events,
        vec![HostEvent::PolicySet {
            room_id: "room-policy-json".to_string(),
        }]
    );
    unsafe { as_bytes_owned_free(set_policy_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

fn signed_single_node_pack_b64() -> String {
    let mut graph = StateGraph::new();
    let signing_key = SigningKey::from_bytes(&[9u8; 32]);
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
fn submit_json_supports_sync_pack_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-sync-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let import_json = serde_json::json!({
        "room_id": "room-sync-json",
        "command": {
            "ImportPack": {
                "nodes_b64": signed_single_node_pack_b64()
            }
        }
    })
    .to_string();

    let mut import_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let import_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: import_json.as_ptr(),
                len: import_json.len(),
            },
            &mut import_out,
        )
    };
    assert_eq!(import_status, as_status::AS_OK);
    let import_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(import_out.ptr, import_out.len)
    });
    assert!(matches!(
        &import_events[0],
        HostEvent::PackImported {
            incoming_count: 1,
            accepted_count: 1,
            rejected_count: 0,
            ..
        }
    ));
    unsafe { as_bytes_owned_free(import_out) };

    let request_json = serde_json::json!({
        "room_id": "room-sync-json",
        "command": {
            "RequestServerPack": {
                "known_ids": []
            }
        }
    })
    .to_string();

    let mut request_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let request_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: request_json.as_ptr(),
                len: request_json.len(),
            },
            &mut request_out,
        )
    };
    assert_eq!(request_status, as_status::AS_OK);
    let request_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(request_out.ptr, request_out.len)
    });
    assert!(matches!(
        &request_events[0],
        HostEvent::ServerPackPrepared { nodes_b64, .. } if !nodes_b64.is_empty()
    ));
    unsafe { as_bytes_owned_free(request_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_conflict_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-conflict-json",
        "command": "EnsureRoom"
    })
    .to_string();

    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let first_import_json = serde_json::json!({
        "room_id": "room-conflict-json",
        "command": {
            "ImportPack": {
                "nodes_b64": signed_map_set_pack_b64(31, "world/greeting", b"hello")
            }
        }
    })
    .to_string();
    let second_import_json = serde_json::json!({
        "room_id": "room-conflict-json",
        "command": {
            "ImportPack": {
                "nodes_b64": signed_map_set_pack_b64(32, "world/greeting", b"hola")
            }
        }
    })
    .to_string();

    let mut first_import_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let first_import_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: first_import_json.as_ptr(),
                len: first_import_json.len(),
            },
            &mut first_import_out,
        )
    };
    assert_eq!(first_import_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(first_import_out) };

    let mut second_import_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let second_import_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: second_import_json.as_ptr(),
                len: second_import_json.len(),
            },
            &mut second_import_out,
        )
    };
    assert_eq!(second_import_status, as_status::AS_OK);
    let second_import_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(second_import_out.ptr, second_import_out.len)
    });
    assert!(matches!(
        &second_import_events[0],
        HostEvent::PackImported { .. }
    ));
    assert!(matches!(
        &second_import_events[1],
        HostEvent::ConflictsObserved { entries, .. } if !entries.is_empty()
    ));
    unsafe { as_bytes_owned_free(second_import_out) };

    let recent_conflicts_json = serde_json::json!({
        "room_id": "room-conflict-json",
        "command": {
            "GetRecentConflicts": {
                "since_unix_ms": null
            }
        }
    })
    .to_string();

    let mut recent_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let recent_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: recent_conflicts_json.as_ptr(),
                len: recent_conflicts_json.len(),
            },
            &mut recent_out,
        )
    };
    assert_eq!(recent_status, as_status::AS_OK);
    let recent_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(recent_out.ptr, recent_out.len)
    });
    assert!(matches!(
        &recent_events[0],
        HostEvent::RecentConflictsListed { entries, .. } if !entries.is_empty()
    ));
    unsafe { as_bytes_owned_free(recent_out) };

    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}

#[test]
fn submit_json_supports_peer_relay_commands() {
    let mut engine: *mut as_host_engine = std::ptr::null_mut();
    let create_status = unsafe { as_host_engine_new(&mut engine) };
    assert_eq!(create_status, as_status::AS_OK);

    let ensure_json = serde_json::json!({
        "room_id": "room-relay-json",
        "command": "EnsureRoom"
    })
    .to_string();
    let mut ensure_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let ensure_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: ensure_json.as_ptr(),
                len: ensure_json.len(),
            },
            &mut ensure_out,
        )
    };
    assert_eq!(ensure_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(ensure_out) };

    let open_json = serde_json::json!({
        "room_id": "room-relay-json",
        "command": {
            "OpenSession": {
                "session_id": 21,
                "peer_pubkey_hex": "peer-a"
            }
        }
    })
    .to_string();
    let mut open_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let open_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: open_json.as_ptr(),
                len: open_json.len(),
            },
            &mut open_out,
        )
    };
    assert_eq!(open_status, as_status::AS_OK);
    unsafe { as_bytes_owned_free(open_out) };

    let relay_json = serde_json::json!({
        "room_id": "room-relay-json",
        "command": {
            "RelayPeerSignal": {
                "session_id": 21,
                "msg_type": "webrtc-ice",
                "to_peer_pubkey": "peer-b",
                "payload": {
                    "candidate": {
                        "candidate": "candidate:1 1 udp 1 127.0.0.1 9999 typ host"
                    }
                }
            }
        }
    })
    .to_string();
    let mut relay_out = as_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let relay_status = unsafe {
        as_host_submit_command_json(
            engine,
            as_bytes_view {
                ptr: relay_json.as_ptr(),
                len: relay_json.len(),
            },
            &mut relay_out,
        )
    };
    assert_eq!(relay_status, as_status::AS_OK);
    let relay_events = decode_events_json(unsafe {
        std::slice::from_raw_parts(relay_out.ptr, relay_out.len)
    });

    assert_eq!(
        relay_events,
        vec![HostEvent::PeerSignalRelayed {
            room_id: "room-relay-json".to_string(),
            from_peer_pubkey: "peer-a".to_string(),
            msg_type: "webrtc-ice".to_string(),
            to_peer_pubkey: "peer-b".to_string(),
            payload: serde_json::json!({
                "candidate": {
                    "candidate": "candidate:1 1 udp 1 127.0.0.1 9999 typ host"
                }
            }),
        }]
    );

    unsafe { as_bytes_owned_free(relay_out) };
    let free_status = unsafe { as_host_engine_free(engine) };
    assert_eq!(free_status, as_status::AS_OK);
}
