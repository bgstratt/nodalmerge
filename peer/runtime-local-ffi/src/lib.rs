//! C ABI for peer-local persistence (`nodalmerge-runtime-local`).
//!
//! Intended for .NET / native hosts that embed durable peer state in-process
//! without spawning `nodalmerge-headless` as a sidecar.

use std::path::PathBuf;
use std::ptr;
use std::slice;

use base64::Engine as _;
use nodalmerge_core::{canonical_hash, replay, unpack_nodes, SyncNode};
use nodalmerge_runtime_local::{
    parse_backend_kind, LocalPersistError, LocalPersistReason,
    PeerLocalPersistence, PersistenceHandle,
};
use serde::Serialize;

const ABI_VERSION: u32 = 1;

#[repr(C)]
pub struct nm_local_store {
    handle: PersistenceHandle,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct nm_bytes_view {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct nm_bytes_owned {
    pub ptr: *mut u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum nm_local_status {
    NM_LOCAL_OK = 0,
    NM_LOCAL_ERR_INVALID_ARG = 1,
    NM_LOCAL_ERR_NOT_FOUND = 2,
    NM_LOCAL_ERR_UNAVAILABLE = 3,
    NM_LOCAL_ERR_CORRUPTION = 4,
    NM_LOCAL_ERR_TAIL_CONFLICT = 5,
    NM_LOCAL_ERR_QUOTA = 6,
    NM_LOCAL_ERR_READONLY = 7,
    NM_LOCAL_ERR_INTERNAL = 255,
}

#[derive(Serialize)]
struct FfiHydrateReport {
    room_id: String,
    tail_seq: u64,
    node_count: usize,
    canonical_hash_hex: String,
}

#[derive(Serialize)]
struct FfiRecoveryReport {
    room_id: String,
    tail_seq: u64,
    node_count: usize,
    canonical_hash_hex: String,
}

#[derive(Serialize)]
struct FfiFlushReport {
    room_id: String,
    tail_seq: u64,
    durable: bool,
}

#[derive(Serialize)]
struct FfiAppendReport {
    room_id: String,
    appended: usize,
    tail_seq: u64,
    canonical_hash_hex: String,
}

fn view_to_str(view: nm_bytes_view) -> Result<String, nm_local_status> {
    if view.len == 0 {
        return Ok(String::new());
    }
    if view.ptr.is_null() {
        return Err(nm_local_status::NM_LOCAL_ERR_INVALID_ARG);
    }
    let bytes = unsafe { slice::from_raw_parts(view.ptr, view.len) };
    std::str::from_utf8(bytes)
        .map(|s| s.to_string())
        .map_err(|_| nm_local_status::NM_LOCAL_ERR_INVALID_ARG)
}

fn view_to_opt_path(view: nm_bytes_view) -> Result<Option<PathBuf>, nm_local_status> {
    let s = view_to_str(view)?;
    if s.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(s)))
    }
}

fn make_owned_bytes(bytes: Vec<u8>) -> nm_bytes_owned {
    if bytes.is_empty() {
        return nm_bytes_owned {
            ptr: ptr::null_mut(),
            len: 0,
        };
    }
    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *mut u8;
    nm_bytes_owned { ptr, len }
}

fn map_persist_error(err: LocalPersistError) -> nm_local_status {
    match err.reason {
        LocalPersistReason::Unavailable => nm_local_status::NM_LOCAL_ERR_UNAVAILABLE,
        LocalPersistReason::VersionSkew => nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
        LocalPersistReason::Corruption => nm_local_status::NM_LOCAL_ERR_CORRUPTION,
        LocalPersistReason::TailConflict => nm_local_status::NM_LOCAL_ERR_TAIL_CONFLICT,
        LocalPersistReason::Quota => nm_local_status::NM_LOCAL_ERR_QUOTA,
        LocalPersistReason::ReadOnly => nm_local_status::NM_LOCAL_ERR_READONLY,
    }
}

fn canonical_hash_hex_from_nodes(nodes: &[SyncNode]) -> Result<String, nm_local_status> {
    let state = replay(nodes, None).map_err(|_| nm_local_status::NM_LOCAL_ERR_INTERNAL)?;
    Ok(canonical_hash(&state.map).to_hex())
}

fn with_store<F>(store: *mut nm_local_store, f: F) -> nm_local_status
where
    F: FnOnce(&PersistenceHandle) -> nm_local_status,
{
    if store.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    f(unsafe { &(*store).handle })
}

fn with_adapter<F>(store: *mut nm_local_store, f: F) -> nm_local_status
where
    F: FnOnce(&dyn PeerLocalPersistence) -> nm_local_status,
{
    with_store(store, |handle| f(handle.as_dyn()))
}

#[no_mangle]
pub extern "C" fn nm_local_abi_version() -> u32 {
    ABI_VERSION
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_open(
    backend: nm_bytes_view,
    data_dir: nm_bytes_view,
    out_store: *mut *mut nm_local_store,
) -> nm_local_status {
    if out_store.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }

    let backend_name = match view_to_str(backend) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let data_dir = match view_to_opt_path(data_dir) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let kind = match parse_backend_kind(&backend_name, data_dir) {
        Ok(k) => k,
        Err(_) => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };
    let handle = match PersistenceHandle::open(kind) {
        Ok(h) => h,
        Err(e) => return map_persist_error(e),
    };

    let boxed = Box::new(nm_local_store { handle });
    *out_store = Box::into_raw(boxed);
    nm_local_status::NM_LOCAL_OK
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_free(store: *mut nm_local_store) -> nm_local_status {
    if store.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    drop(Box::from_raw(store));
    nm_local_status::NM_LOCAL_OK
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_is_durable(
    store: *mut nm_local_store,
    out_durable: *mut u8,
) -> nm_local_status {
    if out_durable.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    with_adapter(store, |adapter| {
        *out_durable = u8::from(adapter.is_durable());
        nm_local_status::NM_LOCAL_OK
    })
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_hydrate_json(
    store: *mut nm_local_store,
    room_id: nm_bytes_view,
    out_json: *mut nm_bytes_owned,
) -> nm_local_status {
    if out_json.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    let room = match view_to_str(room_id) {
        Ok(s) if !s.is_empty() => s,
        _ => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };

    with_adapter(store, |adapter| {
        let report = match adapter.hydrate(&room) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let hash = match canonical_hash_hex_from_nodes(&report.nodes) {
            Ok(h) => h,
            Err(e) => return e,
        };
        let ffi = FfiHydrateReport {
            room_id: report.room_id,
            tail_seq: report.tail.seq,
            node_count: report.nodes.len(),
            canonical_hash_hex: hash,
        };
        let json = match serde_json::to_vec(&ffi) {
            Ok(v) => v,
            Err(_) => return nm_local_status::NM_LOCAL_ERR_INTERNAL,
        };
        *out_json = make_owned_bytes(json);
        nm_local_status::NM_LOCAL_OK
    })
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_recover_json(
    store: *mut nm_local_store,
    room_id: nm_bytes_view,
    out_json: *mut nm_bytes_owned,
) -> nm_local_status {
    if out_json.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    let room = match view_to_str(room_id) {
        Ok(s) if !s.is_empty() => s,
        _ => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };

    with_adapter(store, |adapter| {
        let report = match adapter.recover(&room) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let hash = match canonical_hash_hex_from_nodes(&report.nodes) {
            Ok(h) => h,
            Err(e) => return e,
        };
        let ffi = FfiRecoveryReport {
            room_id: report.room_id,
            tail_seq: report.tail.seq,
            node_count: report.nodes.len(),
            canonical_hash_hex: hash,
        };
        let json = match serde_json::to_vec(&ffi) {
            Ok(v) => v,
            Err(_) => return nm_local_status::NM_LOCAL_ERR_INTERNAL,
        };
        *out_json = make_owned_bytes(json);
        nm_local_status::NM_LOCAL_OK
    })
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_flush_json(
    store: *mut nm_local_store,
    room_id: nm_bytes_view,
    out_json: *mut nm_bytes_owned,
) -> nm_local_status {
    if out_json.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    let room = match view_to_str(room_id) {
        Ok(s) if !s.is_empty() => s,
        _ => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };

    with_adapter(store, |adapter| {
        let report = match adapter.flush(&room) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let ffi = FfiFlushReport {
            room_id: report.room_id,
            tail_seq: report.tail.seq,
            durable: report.durable,
        };
        let json = match serde_json::to_vec(&ffi) {
            Ok(v) => v,
            Err(_) => return nm_local_status::NM_LOCAL_ERR_INTERNAL,
        };
        *out_json = make_owned_bytes(json);
        nm_local_status::NM_LOCAL_OK
    })
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_append_nodes_json(
    store: *mut nm_local_store,
    room_id: nm_bytes_view,
    nodes_json: nm_bytes_view,
    out_json: *mut nm_bytes_owned,
) -> nm_local_status {
    if out_json.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    let room = match view_to_str(room_id) {
        Ok(s) if !s.is_empty() => s,
        _ => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };
    let nodes_raw = match view_to_str(nodes_json) {
        Ok(s) => s,
        Err(e) => return e,
    };
    let nodes: Vec<SyncNode> = match serde_json::from_str(&nodes_raw) {
        Ok(n) => n,
        Err(_) => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };

    with_adapter(store, |adapter| {
        let report = match adapter.append_nodes(&room, &nodes, None) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let recovery = match adapter.recover(&room) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let hash = match canonical_hash_hex_from_nodes(&recovery.nodes) {
            Ok(h) => h,
            Err(e) => return e,
        };
        let ffi = FfiAppendReport {
            room_id: report.room_id,
            appended: report.appended,
            tail_seq: report.tail.seq,
            canonical_hash_hex: hash,
        };
        let json = match serde_json::to_vec(&ffi) {
            Ok(v) => v,
            Err(_) => return nm_local_status::NM_LOCAL_ERR_INTERNAL,
        };
        *out_json = make_owned_bytes(json);
        nm_local_status::NM_LOCAL_OK
    })
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_append_pack_b64(
    store: *mut nm_local_store,
    room_id: nm_bytes_view,
    pack_nodes_b64: nm_bytes_view,
    out_json: *mut nm_bytes_owned,
) -> nm_local_status {
    if out_json.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    let room = match view_to_str(room_id) {
        Ok(s) if !s.is_empty() => s,
        _ => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };
    let pack_b64 = match view_to_str(pack_nodes_b64) {
        Ok(s) if !s.is_empty() => s,
        _ => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };

    let raw = match base64::engine::general_purpose::STANDARD.decode(pack_b64.as_bytes()) {
        Ok(bytes) => bytes,
        Err(_) => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };
    let nodes = if raw.is_empty() {
        Vec::new()
    } else {
        match unpack_nodes(&raw) {
            Ok(n) => n,
            Err(_) => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
        }
    };

    with_adapter(store, |adapter| {
        let report = match adapter.append_nodes(&room, &nodes, None) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let recovery = match adapter.recover(&room) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let hash = match canonical_hash_hex_from_nodes(&recovery.nodes) {
            Ok(h) => h,
            Err(e) => return e,
        };
        let ffi = FfiAppendReport {
            room_id: report.room_id,
            appended: report.appended,
            tail_seq: report.tail.seq,
            canonical_hash_hex: hash,
        };
        let json = match serde_json::to_vec(&ffi) {
            Ok(v) => v,
            Err(_) => return nm_local_status::NM_LOCAL_ERR_INTERNAL,
        };
        *out_json = make_owned_bytes(json);
        nm_local_status::NM_LOCAL_OK
    })
}

#[no_mangle]
pub unsafe extern "C" fn nm_local_store_canonical_hash_hex(
    store: *mut nm_local_store,
    room_id: nm_bytes_view,
    out_hex: *mut nm_bytes_owned,
) -> nm_local_status {
    if out_hex.is_null() {
        return nm_local_status::NM_LOCAL_ERR_INVALID_ARG;
    }
    let room = match view_to_str(room_id) {
        Ok(s) if !s.is_empty() => s,
        _ => return nm_local_status::NM_LOCAL_ERR_INVALID_ARG,
    };

    with_adapter(store, |adapter| {
        let report = match adapter.recover(&room) {
            Ok(r) => r,
            Err(e) => return map_persist_error(e),
        };
        let hash = match canonical_hash_hex_from_nodes(&report.nodes) {
            Ok(h) => h,
            Err(e) => return e,
        };
        *out_hex = make_owned_bytes(hash.into_bytes());
        nm_local_status::NM_LOCAL_OK
    })
}

#[no_mangle]
pub unsafe extern "C" fn nm_bytes_owned_free(bytes: nm_bytes_owned) {
    if bytes.ptr.is_null() || bytes.len == 0 {
        return;
    }
    let _ = Box::from_raw(slice::from_raw_parts_mut(bytes.ptr, bytes.len));
}
