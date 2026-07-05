//! Cross-implementation interop proof for the RoomToken FFI surface (S2):
//! a token minted through the C ABI must verify via `nodalmerge_core`
//! directly (what the Rust server does at hello), and a core-minted token
//! must validate through the C ABI (what the .NET provider does).

use nodalmerge_host_ffi::{
    nm_bytes_owned, nm_bytes_view, nm_room_token_mint_json, nm_room_token_validate_json, nm_host_status,
};

fn call_json(
    f: unsafe extern "C" fn(nm_bytes_view, *mut nm_bytes_owned) -> nm_host_status,
    request: &serde_json::Value,
) -> (nm_host_status, Option<serde_json::Value>) {
    let body = serde_json::to_vec(request).unwrap();
    let view = nm_bytes_view {
        ptr: body.as_ptr(),
        len: body.len(),
    };
    let mut out = nm_bytes_owned {
        ptr: std::ptr::null_mut(),
        len: 0,
    };
    let status = unsafe { f(view, &mut out) };
    let parsed = if out.ptr.is_null() || out.len == 0 {
        None
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(out.ptr, out.len) }.to_vec();
        unsafe { nodalmerge_host_ffi::nm_bytes_owned_free(out) };
        Some(serde_json::from_slice(&bytes).unwrap())
    };
    (status, parsed)
}

fn seed_hex(seed: u8) -> String {
    hex(&[seed; 32])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn peer_pubkey_hex(seed: u8) -> String {
    hex(&ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
        .verifying_key()
        .to_bytes())
}

#[test]
fn ffi_minted_token_verifies_via_core_directly() {
    let (status, token) = call_json(
        nm_room_token_mint_json,
        &serde_json::json!({
            "room_id": "interop-room",
            "room_signing_key_hex": seed_hex(0x11),
            "peer_pubkey_hex": peer_pubkey_hex(0x22),
            "expiry_unix_secs": u64::MAX,
            "capabilities": ["write:intent/**", "read:world/**"]
        }),
    );
    assert_eq!(status, nm_host_status::NM_HOST_OK);
    let token = token.unwrap();

    // Verify exactly the way the Rust server does at hello.
    let core_token = nodalmerge_core::RoomToken::from_wire(
        token["peer_pubkey"].as_str().unwrap(),
        token["expiry"].as_u64().unwrap(),
        token["caps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect(),
        token["sig"].as_str().unwrap(),
    )
    .unwrap();

    let room_key = ed25519_dalek::SigningKey::from_bytes(&[0x11; 32]);
    let peer = ed25519_dalek::SigningKey::from_bytes(&[0x22; 32])
        .verifying_key()
        .to_bytes();
    core_token
        .verify("interop-room", &room_key.verifying_key(), &peer, 0)
        .expect("FFI-minted token must verify via core");

    // And the response's derived room pubkey matches the real one.
    assert_eq!(
        token["room_pubkey_hex"].as_str().unwrap(),
        hex(&room_key.verifying_key().to_bytes())
    );
}

#[test]
fn core_minted_token_validates_via_ffi() {
    let room_key = ed25519_dalek::SigningKey::from_bytes(&[0x33; 32]);
    let peer = ed25519_dalek::SigningKey::from_bytes(&[0x44; 32])
        .verifying_key()
        .to_bytes();
    let caps = vec!["read:world/**".to_string()];
    let token = nodalmerge_core::RoomToken::sign("interop-room", &peer, u64::MAX, &caps, &room_key);

    let (status, result) = call_json(
        nm_room_token_validate_json,
        &serde_json::json!({
            "room_id": "interop-room",
            "room_pubkey_hex": hex(&room_key.verifying_key().to_bytes()),
            "peer_pubkey_hex": token.peer_pubkey_hex(),
            "expiry_unix_secs": token.expiry_secs,
            "capabilities": token.capabilities,
            "sig_hex": token.sig_hex(),
            "now_unix_secs": 0
        }),
    );
    assert_eq!(status, nm_host_status::NM_HOST_OK);
    let result = result.unwrap();
    assert_eq!(result["valid"], true, "reason: {:?}", result["reason"]);
}

#[test]
fn validate_rejects_tamper_expiry_and_wrong_room() {
    let room_key_hex = seed_hex(0x55);
    let peer_hex = peer_pubkey_hex(0x66);
    let (status, token) = call_json(
        nm_room_token_mint_json,
        &serde_json::json!({
            "room_id": "room-a",
            "room_signing_key_hex": room_key_hex,
            "peer_pubkey_hex": peer_hex,
            "expiry_unix_secs": 100,
            "capabilities": ["read:world/**"]
        }),
    );
    assert_eq!(status, nm_host_status::NM_HOST_OK);
    let token = token.unwrap();
    let sig = token["sig"].as_str().unwrap();

    let base = serde_json::json!({
        "room_id": "room-a",
        "room_signing_key_hex": room_key_hex,
        "peer_pubkey_hex": peer_hex,
        "expiry_unix_secs": 100,
        "capabilities": ["read:world/**"],
        "sig_hex": sig,
        "now_unix_secs": 0
    });

    // Valid as-is (validating via signing key instead of pubkey also works).
    let (s, r) = call_json(nm_room_token_validate_json, &base);
    assert_eq!(s, nm_host_status::NM_HOST_OK);
    assert_eq!(r.unwrap()["valid"], true);

    // Capability tampering breaks the signature.
    let mut tampered = base.clone();
    tampered["capabilities"] = serde_json::json!(["write:world/**"]);
    let (s, r) = call_json(nm_room_token_validate_json, &tampered);
    assert_eq!(s, nm_host_status::NM_HOST_OK);
    assert_eq!(r.unwrap()["valid"], false);

    // Expired.
    let mut expired = base.clone();
    expired["now_unix_secs"] = serde_json::json!(100);
    let (s, r) = call_json(nm_room_token_validate_json, &expired);
    assert_eq!(s, nm_host_status::NM_HOST_OK);
    assert_eq!(r.unwrap()["valid"], false);

    // Wrong room.
    let mut wrong_room = base.clone();
    wrong_room["room_id"] = serde_json::json!("room-b");
    let (s, r) = call_json(nm_room_token_validate_json, &wrong_room);
    assert_eq!(s, nm_host_status::NM_HOST_OK);
    assert_eq!(r.unwrap()["valid"], false);
}
