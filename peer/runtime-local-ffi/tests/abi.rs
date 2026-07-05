use base64::Engine as _;
use ed25519_dalek::SigningKey;
use nodalmerge_core::{MapOp, Op, StateGraph, SyncNode};
use nodalmerge_core::pack_nodes;
use nodalmerge_runtime_local_ffi::{
    nm_bytes_owned, nm_bytes_owned_free, nm_bytes_view, nm_local_abi_version, nm_local_status,
    nm_local_store, nm_local_store_append_nodes_json, nm_local_store_append_pack_b64,
    nm_local_store_canonical_hash_hex, nm_local_store_free, nm_local_store_flush_json,
    nm_local_store_hydrate_json, nm_local_store_open,
};

fn make_nodes() -> Vec<SyncNode> {
    let sk = SigningKey::from_bytes(&[0x71u8; 32]);
    let mut g = StateGraph::new();
    let id = g
        .apply_local(
            &sk,
            0,
            vec![Op::Map(MapOp::Set {
                key: "world/cli".into(),
                value: b"ffi".to_vec(),
            })],
        )
        .expect("apply");
    g.get_nodes(&[id]).into_iter().cloned().collect()
}

fn view(s: &str) -> nm_bytes_view {
    nm_bytes_view {
        ptr: s.as_ptr(),
        len: s.len(),
    }
}

#[test]
#[ignore = "fixture generator: cargo test dump_pack_b64_fixture -- --ignored --nocapture"]
fn dump_pack_b64_fixture() {
    let nodes = make_nodes();
    let node_refs: Vec<&SyncNode> = nodes.iter().collect();
    let packed = pack_nodes(&node_refs);
    let pack_b64 = base64::engine::general_purpose::STANDARD.encode(&packed);
    eprintln!("PACK_B64_FIXTURE={pack_b64}");
}

#[test]
fn abi_version_is_v1() {
    assert_eq!(nm_local_abi_version(), 1);
}

#[test]
fn memory_store_append_and_hash_stable_in_session() {
    let mut store: *mut nm_local_store = std::ptr::null_mut();
    let status = unsafe { nm_local_store_open(view("memory"), view(""), &mut store) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);

    let nodes = make_nodes();
    let nodes_json = serde_json::to_string(&nodes).expect("serialize nodes");
    let mut out = nm_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let status = unsafe { nm_local_store_hydrate_json(store, view("room-ffi"), &mut out) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);
    unsafe { nm_bytes_owned_free(out) };

    let status = unsafe {
        nm_local_store_append_nodes_json(store, view("room-ffi"), view(&nodes_json), &mut out)
    };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK, "append failed for serialized nodes");
    unsafe { nm_bytes_owned_free(out) };

    let status = unsafe { nm_local_store_flush_json(store, view("room-ffi"), &mut out) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);
    unsafe { nm_bytes_owned_free(out) };

    let status = unsafe { nm_local_store_canonical_hash_hex(store, view("room-ffi"), &mut out) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);
    let hash1 = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(out.ptr, out.len)).unwrap().to_string()
    };
    unsafe { nm_bytes_owned_free(out) };

    let status = unsafe { nm_local_store_canonical_hash_hex(store, view("room-ffi"), &mut out) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);
    let hash2 = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(out.ptr, out.len)).unwrap()
    };
    assert_eq!(hash1, hash2);

    unsafe {
        nm_bytes_owned_free(out);
        nm_local_store_free(store);
    }
}

#[test]
fn append_pack_b64_round_trips_ws_pack_blob() {
    let mut store: *mut nm_local_store = std::ptr::null_mut();
    let status = unsafe { nm_local_store_open(view("memory"), view(""), &mut store) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);

    let nodes = make_nodes();
    let node_refs: Vec<&SyncNode> = nodes.iter().collect();
    let packed = pack_nodes(&node_refs);
    let pack_b64 = base64::engine::general_purpose::STANDARD.encode(&packed);

    let mut out = nm_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let status = unsafe {
        nm_local_store_append_pack_b64(store, view("room-pack"), view(&pack_b64), &mut out)
    };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);
    let json = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(out.ptr, out.len)).unwrap()
    };
    assert!(json.contains("\"appended\":1"));
    unsafe { nm_bytes_owned_free(out) };

    let status = unsafe { nm_local_store_canonical_hash_hex(store, view("room-pack"), &mut out) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);
    unsafe {
        nm_bytes_owned_free(out);
        nm_local_store_free(store);
    }
}

#[test]
fn file_store_survives_reopen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().to_string_lossy();
    let mut store: *mut nm_local_store = std::ptr::null_mut();
    let status = unsafe {
        nm_local_store_open(view("embedded"), view(&data_dir), &mut store)
    };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);

    let nodes = make_nodes();
    let nodes_json = serde_json::to_string(&nodes).unwrap();
    let mut out = nm_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    unsafe {
        nm_local_store_append_nodes_json(store, view("room-file"), view(&nodes_json), &mut out);
        nm_bytes_owned_free(out);
        nm_local_store_flush_json(store, view("room-file"), &mut out);
        nm_bytes_owned_free(out);
        nm_local_store_free(store);
    };

    let status = unsafe {
        nm_local_store_open(view("embedded"), view(&data_dir), &mut store)
    };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);

    let status = unsafe { nm_local_store_hydrate_json(store, view("room-file"), &mut out) };
    assert_eq!(status, nm_local_status::NM_LOCAL_OK);
    let json = unsafe {
        std::str::from_utf8(std::slice::from_raw_parts(out.ptr, out.len)).unwrap()
    };
    assert!(json.contains("canonical_hash_hex"));
    assert!(json.contains("\"node_count\":1"));
    unsafe {
        nm_bytes_owned_free(out);
        nm_local_store_free(store);
    }
}

