use std::ffi::c_void;

use nodalmerge_host_core::api::HostCommand;
use nodalmerge_host_core::engine::HostEngine;
use nodalmerge_host_core::errors::HostCoreError;
use serde::{Deserialize, Serialize};

const ABI_VERSION: u32 = 1;

#[repr(C)]
pub struct as_host_engine {
    inner: HostEngine,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct as_bytes_view {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct as_bytes_owned {
    pub ptr: *mut u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum as_status {
    AS_OK = 0,
    AS_ERR_INVALID_ARG = 1,
    AS_ERR_NOT_FOUND = 2,
    AS_ERR_AUTH = 3,
    AS_ERR_POLICY = 4,
    AS_ERR_PROTOCOL = 5,
    AS_ERR_INTERNAL = 255,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FfiCommandEnvelope {
    room_id: String,
    command: HostCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FfiDenyMetadata {
    reason_class: String,
    command: String,
    required_capability: String,
    deny_message: Option<String>,
}

fn make_owned_bytes(bytes: Vec<u8>) -> as_bytes_owned {
    if bytes.is_empty() {
        return as_bytes_owned {
            ptr: std::ptr::null_mut(),
            len: 0,
        };
    }

    let boxed = bytes.into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed) as *mut u8;
    as_bytes_owned { ptr, len }
}

fn map_host_core_error(err: HostCoreError) -> as_status {
    match err {
        HostCoreError::InvalidCommand => as_status::AS_ERR_INVALID_ARG,
        HostCoreError::RoomNotFound | HostCoreError::SessionNotFound => as_status::AS_ERR_NOT_FOUND,
        HostCoreError::AuthViolation => as_status::AS_ERR_AUTH,
        HostCoreError::PolicyViolation => as_status::AS_ERR_POLICY,
        HostCoreError::ProtocolViolation | HostCoreError::SessionAlreadyOpen => {
            as_status::AS_ERR_PROTOCOL
        }
        HostCoreError::InternalInvariant => as_status::AS_ERR_INTERNAL,
    }
}

fn command_label(command: &HostCommand) -> &'static str {
    match command {
        HostCommand::EnsureRoom => "ensure-room",
        HostCommand::OpenSession { .. } => "open-session",
        HostCommand::CloseSession { .. } => "close-session",
        HostCommand::ClientHello { .. } => "client-hello",
        HostCommand::MapSet { .. } => "map-set",
        HostCommand::MapDelete { .. } => "map-delete",
        HostCommand::MapGet { .. } => "map-get",
        HostCommand::MapAll { .. } => "map-all",
        HostCommand::TextInsert { .. } => "text-insert",
        HostCommand::TextDelete { .. } => "text-delete",
        HostCommand::TextGet { .. } => "text-get",
        HostCommand::TextGetCanonical { .. } => "text-get-canonical",
        HostCommand::ListPush { .. } => "list-push",
        HostCommand::ListInsert { .. } => "list-insert",
        HostCommand::ListDelete { .. } => "list-delete",
        HostCommand::ListMove { .. } => "list-move",
        HostCommand::ListUpdate { .. } => "list-update",
        HostCommand::ListGet { .. } => "list-get",
        HostCommand::BlobSet { .. } => "blob-set",
        HostCommand::BlobGet { .. } => "blob-get",
        HostCommand::BlobGetMany { .. } => "blob-get-many",
        HostCommand::RequestUpload { .. } => "request-upload",
        HostCommand::BlobRequest { .. } => "blob-request",
        HostCommand::PresenceSet { .. } => "presence-set",
        HostCommand::PresenceGetAll => "presence-get",
        HostCommand::PresenceSweep { .. } => "presence-sweep",
        HostCommand::Subscribe { .. } => "subscribe",
        HostCommand::SetPolicy { .. } => "set-policy",
        HostCommand::SetRoomKey { .. } => "set-room-key",
        HostCommand::ImportPack { .. } => "pack",
        HostCommand::RequestServerPack { .. } => "request-server-pack",
        HostCommand::MstRequest { .. } => "mst-request",
        HostCommand::MstDone { .. } => "mst-done",
        HostCommand::GetRecentConflicts { .. } => "recent-conflicts",
        HostCommand::ReplayReadRange { .. } => "replay.read-range",
        HostCommand::PromoteCheckpointToGraph { .. } => "checkpoint.promote",
        HostCommand::GetFrontier => "graph.get-frontier",
        HostCommand::GetCausalParents { .. } => "graph.get-causal-parents",
        HostCommand::GetCanonicalResolution => "graph.get-canonical-resolution",
        HostCommand::ComputeSyncDiff { .. } => "graph.compute-sync-diff",
        HostCommand::InspectPack { .. } => "inspect-pack",
        HostCommand::RelayPeerSignal { .. } => "relay-peer-signal",
        HostCommand::CreateTopologyChild { .. } => "topology.create-child",
        HostCommand::DescribeRoomLineage { .. } => "topology.describe-lineage",
        HostCommand::ListTopologyChildren { .. } => "topology.list-children",
        HostCommand::ProposeTopologyPromotion { .. } => "topology.propose-promotion",
        HostCommand::ValidateTopologyPromotion { .. } => "topology.validate-promotion",
        HostCommand::ApplyTopologyPromotion { .. } => "topology.apply-promotion",
        HostCommand::RegisterQuerySpec { .. } => "query.register",
        HostCommand::BuildProjection { .. } => "projection.build",
        HostCommand::ReadProjection { .. } => "projection.read",
        HostCommand::InvalidateProjection { .. } => "projection.invalidate",
        HostCommand::ListProjections { .. } => "projection.list",
        HostCommand::DescribeArchive { .. } => "archive.describe",
        HostCommand::ValidateArchive { .. } => "archive.validate",
        HostCommand::ImportArchive { .. } => "archive.import",
        HostCommand::Noop => "noop",
    }
}

fn capability_label_for_command(command: &HostCommand) -> &'static str {
    match command {
        HostCommand::SetPolicy { .. } => "policy.admin",
        HostCommand::SetRoomKey { .. } => "room.admin",
        HostCommand::CreateTopologyChild { .. } => "topology.admin",
        HostCommand::DescribeRoomLineage { .. } => "topology.admin",
        HostCommand::ListTopologyChildren { .. } => "topology.admin",
        HostCommand::ProposeTopologyPromotion { .. } => "topology.admin",
        HostCommand::ValidateTopologyPromotion { .. } => "topology.admin",
        HostCommand::ApplyTopologyPromotion { .. } => "topology.admin",
        HostCommand::RegisterQuerySpec { .. } => "query.admin",
        HostCommand::BuildProjection { .. } => "query.admin",
        HostCommand::PromoteCheckpointToGraph { .. } => "query.admin",
        HostCommand::GetFrontier => "query.admin",
        HostCommand::GetCausalParents { .. } => "query.admin",
        HostCommand::GetCanonicalResolution => "query.admin",
        HostCommand::ComputeSyncDiff { .. } => "query.admin",
        HostCommand::InspectPack { .. } => "query.admin",
        HostCommand::ReadProjection { .. } => "query.read",
        HostCommand::ReplayReadRange { .. } => "query.read",
        HostCommand::InvalidateProjection { .. } => "query.admin",
        HostCommand::ListProjections { .. } => "query.read",
        HostCommand::DescribeArchive { .. } => "archive.read",
        HostCommand::ValidateArchive { .. } => "archive.admin",
        HostCommand::ImportArchive { .. } => "archive.admin",
        _ => "unknown",
    }
}

fn deny_reason_class(status: as_status, command: &HostCommand) -> Option<&'static str> {
    if status == as_status::AS_ERR_AUTH {
        return Some("reject.auth_violation");
    }

    if status == as_status::AS_ERR_POLICY {
        return Some(match command {
            HostCommand::SetPolicy { .. } | HostCommand::SetRoomKey { .. } => {
                "reject.control_plane_forbidden"
            }
            _ => "reject.policy_violation",
        });
    }

    if status == as_status::AS_ERR_PROTOCOL {
        return Some("reject.protocol_violation");
    }

    None
}

fn map_host_core_error_with_metadata(
    err: HostCoreError,
    command: &HostCommand,
) -> (as_status, Option<FfiDenyMetadata>) {
    let status = map_host_core_error(err.clone());
    let metadata = deny_reason_class(status, command).map(|reason_class| FfiDenyMetadata {
        reason_class: reason_class.to_string(),
        command: command_label(command).to_string(),
        required_capability: capability_label_for_command(command).to_string(),
        deny_message: Some(err.to_string()),
    });

    (status, metadata)
}

#[unsafe(no_mangle)]
pub extern "C" fn as_host_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_host_engine_new(out_engine: *mut *mut as_host_engine) -> as_status {
    if out_engine.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    let boxed = Box::new(as_host_engine {
        inner: HostEngine::new(),
    });

    // SAFETY: out_engine was validated as non-null above and points to caller-owned storage.
    unsafe {
        *out_engine = Box::into_raw(boxed);
    }

    as_status::AS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_host_engine_free(engine: *mut as_host_engine) -> as_status {
    if engine.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    // SAFETY: pointer was returned from Box::into_raw in as_host_engine_new.
    unsafe {
        drop(Box::from_raw(engine));
    }

    as_status::AS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_host_submit_command(
    engine: *mut as_host_engine,
    command_bin: as_bytes_view,
    out_events_bin: *mut as_bytes_owned,
) -> as_status {
    if engine.is_null() || out_events_bin.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    if command_bin.len > 0 && command_bin.ptr.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    let command_bytes = if command_bin.len == 0 {
        &[][..]
    } else {
        // SAFETY: validated non-null pointer with caller-provided byte length.
        unsafe { std::slice::from_raw_parts(command_bin.ptr, command_bin.len) }
    };

    let envelope: FfiCommandEnvelope = match postcard::from_bytes(command_bytes) {
        Ok(envelope) => envelope,
        Err(_) => return as_status::AS_ERR_INVALID_ARG,
    };

    let engine_ref = {
        // SAFETY: engine pointer validated as non-null above and is valid for this call.
        unsafe { &mut (*engine).inner }
    };

    let result = match engine_ref.apply(nodalmerge_host_core::api::CommandEnvelope::new(
        envelope.room_id,
        envelope.command,
    )) {
        Ok(result) => result,
        Err(err) => return map_host_core_error(err),
    };

    let event_bytes = match postcard::to_allocvec(&result.events) {
        Ok(bytes) => bytes,
        Err(_) => return as_status::AS_ERR_INTERNAL,
    };

    let owned = make_owned_bytes(event_bytes);
    // SAFETY: out_events_bin was validated as non-null above and points to caller-owned storage.
    unsafe {
        *out_events_bin = owned;
    }

    as_status::AS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_host_submit_command_ex(
    engine: *mut as_host_engine,
    command_bin: as_bytes_view,
    out_events_bin: *mut as_bytes_owned,
    out_deny_metadata_json: *mut as_bytes_owned,
) -> as_status {
    if engine.is_null() || out_events_bin.is_null() || out_deny_metadata_json.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    if command_bin.len > 0 && command_bin.ptr.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    // SAFETY: pointer validated above.
    unsafe {
        *out_deny_metadata_json = as_bytes_owned {
            ptr: std::ptr::null_mut(),
            len: 0,
        };
    }

    let command_bytes = if command_bin.len == 0 {
        &[][..]
    } else {
        // SAFETY: validated non-null pointer with caller-provided byte length.
        unsafe { std::slice::from_raw_parts(command_bin.ptr, command_bin.len) }
    };

    let envelope: FfiCommandEnvelope = match postcard::from_bytes(command_bytes) {
        Ok(envelope) => envelope,
        Err(_) => return as_status::AS_ERR_INVALID_ARG,
    };

    let engine_ref = {
        // SAFETY: engine pointer validated as non-null above and is valid for this call.
        unsafe { &mut (*engine).inner }
    };

    let result = match engine_ref.apply(nodalmerge_host_core::api::CommandEnvelope::new(
        envelope.room_id,
        envelope.command.clone(),
    )) {
        Ok(result) => result,
        Err(err) => {
            let (status, metadata) = map_host_core_error_with_metadata(err, &envelope.command);
            if let Some(metadata) = metadata {
                let metadata_json = match serde_json::to_vec(&metadata) {
                    Ok(bytes) => bytes,
                    Err(_) => return as_status::AS_ERR_INTERNAL,
                };

                let owned = make_owned_bytes(metadata_json);
                // SAFETY: out pointer validated as non-null above and points to caller-owned storage.
                unsafe {
                    *out_deny_metadata_json = owned;
                }
            }

            return status;
        }
    };

    let event_bytes = match postcard::to_allocvec(&result.events) {
        Ok(bytes) => bytes,
        Err(_) => return as_status::AS_ERR_INTERNAL,
    };

    let owned = make_owned_bytes(event_bytes);
    // SAFETY: out_events_bin was validated as non-null above and points to caller-owned storage.
    unsafe {
        *out_events_bin = owned;
    }

    as_status::AS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_host_submit_command_json(
    engine: *mut as_host_engine,
    command_json: as_bytes_view,
    out_events_json: *mut as_bytes_owned,
) -> as_status {
    if engine.is_null() || out_events_json.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    if command_json.len > 0 && command_json.ptr.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    let command_bytes = if command_json.len == 0 {
        &[][..]
    } else {
        // SAFETY: validated non-null pointer with caller-provided byte length.
        unsafe { std::slice::from_raw_parts(command_json.ptr, command_json.len) }
    };

    let envelope: FfiCommandEnvelope = match serde_json::from_slice(command_bytes) {
        Ok(envelope) => envelope,
        Err(_) => return as_status::AS_ERR_INVALID_ARG,
    };

    let engine_ref = {
        // SAFETY: engine pointer validated as non-null above and is valid for this call.
        unsafe { &mut (*engine).inner }
    };

    let result = match engine_ref.apply(nodalmerge_host_core::api::CommandEnvelope::new(
        envelope.room_id,
        envelope.command,
    )) {
        Ok(result) => result,
        Err(err) => return map_host_core_error(err),
    };

    let events_json = match serde_json::to_vec(&result.events) {
        Ok(bytes) => bytes,
        Err(_) => return as_status::AS_ERR_INTERNAL,
    };

    let owned = make_owned_bytes(events_json);
    // SAFETY: out_events_json was validated as non-null above and points to caller-owned storage.
    unsafe {
        *out_events_json = owned;
    }

    as_status::AS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_host_submit_command_json_ex(
    engine: *mut as_host_engine,
    command_json: as_bytes_view,
    out_events_json: *mut as_bytes_owned,
    out_deny_metadata_json: *mut as_bytes_owned,
) -> as_status {
    if engine.is_null() || out_events_json.is_null() || out_deny_metadata_json.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    if command_json.len > 0 && command_json.ptr.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }

    // SAFETY: pointer validated above.
    unsafe {
        *out_deny_metadata_json = as_bytes_owned {
            ptr: std::ptr::null_mut(),
            len: 0,
        };
    }

    let command_bytes = if command_json.len == 0 {
        &[][..]
    } else {
        // SAFETY: validated non-null pointer with caller-provided byte length.
        unsafe { std::slice::from_raw_parts(command_json.ptr, command_json.len) }
    };

    let envelope: FfiCommandEnvelope = match serde_json::from_slice(command_bytes) {
        Ok(envelope) => envelope,
        Err(_) => return as_status::AS_ERR_INVALID_ARG,
    };

    let engine_ref = {
        // SAFETY: engine pointer validated as non-null above and is valid for this call.
        unsafe { &mut (*engine).inner }
    };

    let result = match engine_ref.apply(nodalmerge_host_core::api::CommandEnvelope::new(
        envelope.room_id,
        envelope.command.clone(),
    )) {
        Ok(result) => result,
        Err(err) => {
            let (status, metadata) = map_host_core_error_with_metadata(err, &envelope.command);
            if let Some(metadata) = metadata {
                let metadata_json = match serde_json::to_vec(&metadata) {
                    Ok(bytes) => bytes,
                    Err(_) => return as_status::AS_ERR_INTERNAL,
                };

                let owned = make_owned_bytes(metadata_json);
                // SAFETY: out pointer validated as non-null above and points to caller-owned storage.
                unsafe {
                    *out_deny_metadata_json = owned;
                }
            }

            return status;
        }
    };

    let events_json = match serde_json::to_vec(&result.events) {
        Ok(bytes) => bytes,
        Err(_) => return as_status::AS_ERR_INTERNAL,
    };

    let owned = make_owned_bytes(events_json);
    // SAFETY: out_events_json was validated as non-null above and points to caller-owned storage.
    unsafe {
        *out_events_json = owned;
    }

    as_status::AS_OK
}

// ---------------------------------------------------------------------------
// RoomToken mint/validate (S2 — shared ed25519 auth for all hosts)
//
// These expose `nodalmerge_core::RoomToken` sign/verify over the C ABI so
// non-Rust hosts (the .NET embedded provider) mint and validate the *same*
// ed25519 credential the Rust server understands, instead of a host-local
// scheme. JSON in / JSON out, matching the `_json` command functions above.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FfiRoomTokenMintRequest {
    room_id: String,
    /// 64-hex (32-byte) ed25519 signing-key seed for the room.
    room_signing_key_hex: String,
    /// 64-hex (32-byte) ed25519 public key of the peer being granted access.
    peer_pubkey_hex: String,
    expiry_unix_secs: u64,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Serialize)]
struct FfiRoomTokenMintResponse {
    peer_pubkey: String,
    expiry: u64,
    caps: Vec<String>,
    sig: String,
    /// 64-hex room public key derived from the signing key — what the Rust
    /// server's `set-room-key` expects, so hosts can lock the room to match.
    room_pubkey_hex: String,
}

#[derive(Debug, Deserialize)]
struct FfiRoomTokenValidateRequest {
    room_id: String,
    /// Exactly one of these must be provided. Prefer `room_pubkey_hex` so
    /// validators never need to hold the signing key.
    #[serde(default)]
    room_pubkey_hex: Option<String>,
    #[serde(default)]
    room_signing_key_hex: Option<String>,
    peer_pubkey_hex: String,
    expiry_unix_secs: u64,
    #[serde(default)]
    capabilities: Vec<String>,
    sig_hex: String,
    /// Unix seconds to evaluate expiry against; omit for system time.
    #[serde(default)]
    now_unix_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
struct FfiRoomTokenValidateResponse {
    valid: bool,
    reason: Option<String>,
}

fn parse_hex_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = char::from(chunk[0]).to_digit(16)?;
        let lo = char::from(chunk[1]).to_digit(16)?;
        out[i] = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

unsafe fn read_view<'a>(view: as_bytes_view) -> Option<&'a [u8]> {
    if view.len == 0 {
        return Some(&[]);
    }
    if view.ptr.is_null() {
        return None;
    }
    // SAFETY: non-null pointer with caller-provided length.
    Some(unsafe { std::slice::from_raw_parts(view.ptr, view.len) })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_room_token_mint_json(
    request_json: as_bytes_view,
    out_token_json: *mut as_bytes_owned,
) -> as_status {
    if out_token_json.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }
    let Some(request_bytes) = (unsafe { read_view(request_json) }) else {
        return as_status::AS_ERR_INVALID_ARG;
    };
    let request: FfiRoomTokenMintRequest = match serde_json::from_slice(request_bytes) {
        Ok(r) => r,
        Err(_) => return as_status::AS_ERR_INVALID_ARG,
    };
    let Some(seed) = parse_hex_32(&request.room_signing_key_hex) else {
        return as_status::AS_ERR_INVALID_ARG;
    };
    let Some(peer_pubkey) = parse_hex_32(&request.peer_pubkey_hex) else {
        return as_status::AS_ERR_INVALID_ARG;
    };

    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let token = nodalmerge_core::RoomToken::sign(
        &request.room_id,
        &peer_pubkey,
        request.expiry_unix_secs,
        &request.capabilities,
        &signing_key,
    );

    let response = FfiRoomTokenMintResponse {
        peer_pubkey: token.peer_pubkey_hex(),
        expiry: token.expiry_secs,
        caps: token.capabilities.clone(),
        sig: token.sig_hex(),
        room_pubkey_hex: hex_of(&signing_key.verifying_key().to_bytes()),
    };
    let bytes = match serde_json::to_vec(&response) {
        Ok(b) => b,
        Err(_) => return as_status::AS_ERR_INTERNAL,
    };
    // SAFETY: out pointer validated as non-null above.
    unsafe {
        *out_token_json = make_owned_bytes(bytes);
    }
    as_status::AS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_room_token_validate_json(
    request_json: as_bytes_view,
    out_result_json: *mut as_bytes_owned,
) -> as_status {
    if out_result_json.is_null() {
        return as_status::AS_ERR_INVALID_ARG;
    }
    let Some(request_bytes) = (unsafe { read_view(request_json) }) else {
        return as_status::AS_ERR_INVALID_ARG;
    };
    let request: FfiRoomTokenValidateRequest = match serde_json::from_slice(request_bytes) {
        Ok(r) => r,
        Err(_) => return as_status::AS_ERR_INVALID_ARG,
    };

    let verifying_key = match (&request.room_pubkey_hex, &request.room_signing_key_hex) {
        (Some(pubkey_hex), _) => {
            let Some(pk) = parse_hex_32(pubkey_hex) else {
                return as_status::AS_ERR_INVALID_ARG;
            };
            match ed25519_dalek::VerifyingKey::from_bytes(&pk) {
                Ok(vk) => vk,
                Err(_) => return as_status::AS_ERR_INVALID_ARG,
            }
        }
        (None, Some(seed_hex)) => {
            let Some(seed) = parse_hex_32(seed_hex) else {
                return as_status::AS_ERR_INVALID_ARG;
            };
            ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key()
        }
        (None, None) => return as_status::AS_ERR_INVALID_ARG,
    };

    let token = match nodalmerge_core::RoomToken::from_wire(
        &request.peer_pubkey_hex,
        request.expiry_unix_secs,
        request.capabilities.clone(),
        &request.sig_hex,
    ) {
        Ok(t) => t,
        Err(_) => return as_status::AS_ERR_INVALID_ARG,
    };

    let Some(connecting_peer) = parse_hex_32(&request.peer_pubkey_hex) else {
        return as_status::AS_ERR_INVALID_ARG;
    };
    let now = request.now_unix_secs.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    });

    let response = match token.verify(&request.room_id, &verifying_key, &connecting_peer, now) {
        Ok(()) => FfiRoomTokenValidateResponse { valid: true, reason: None },
        Err(e) => FfiRoomTokenValidateResponse {
            valid: false,
            reason: Some(e.to_string()),
        },
    };
    let bytes = match serde_json::to_vec(&response) {
        Ok(b) => b,
        Err(_) => return as_status::AS_ERR_INTERNAL,
    };
    // SAFETY: out pointer validated as non-null above.
    unsafe {
        *out_result_json = make_owned_bytes(bytes);
    }
    as_status::AS_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn as_bytes_owned_free(bytes: as_bytes_owned) {
    if bytes.ptr.is_null() || bytes.len == 0 {
        return;
    }

    // SAFETY: bytes were allocated via Box<[u8]> in make_owned_bytes.
    unsafe {
        let raw_slice = std::ptr::slice_from_raw_parts_mut(bytes.ptr, bytes.len);
        drop(Box::from_raw(raw_slice));
    }
}

#[allow(dead_code)]
fn _assert_ffi_safe_sizes(_: *mut c_void) {
}
