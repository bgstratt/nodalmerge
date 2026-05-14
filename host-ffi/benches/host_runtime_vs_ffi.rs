use activesync_host_core::api::{CommandEnvelope, HostCommand};
use activesync_host_core::engine::HostEngine;
use activesync_host_ffi::{
    as_bytes_owned, as_bytes_owned_free, as_bytes_view, as_host_engine, as_host_engine_free,
    as_host_engine_new, as_host_submit_command_json, as_status,
};
use base64::Engine;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use serde_json::json;
use std::ptr;

const ROOM_ID: &str = "bench-room";
const NAMESPACE: &str = "bench";
const BLOB_SIZE_BYTES: usize = 50 * 1024;

fn command_json_bytes(command: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "room_id": ROOM_ID,
        "command": command,
    }))
    .expect("command JSON serialization should succeed")
}

fn ensure_room_json_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "room_id": ROOM_ID,
        "command": "EnsureRoom",
    }))
    .expect("ensure room JSON serialization should succeed")
}

fn noop_json_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "room_id": ROOM_ID,
        "command": "Noop",
    }))
    .expect("noop JSON serialization should succeed")
}

fn map_set_json_bytes() -> Vec<u8> {
    command_json_bytes(json!({
        "MapSet": {
            "namespace": NAMESPACE,
            "key": "fixed-map-key",
            "value": { "n": 42 }
        }
    }))
}

fn request_server_pack_json_bytes() -> Vec<u8> {
    command_json_bytes(json!({
        "RequestServerPack": {
            "known_ids": []
        }
    }))
}

fn blob_set_50kb_json_bytes() -> Vec<u8> {
    let data = vec![7_u8; BLOB_SIZE_BYTES];
    let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
    command_json_bytes(json!({
        "BlobSet": {
            "namespace": NAMESPACE,
            "hash": "blob-50kb-fixed",
            "data_b64": data_b64
        }
    }))
}

fn seed_host_engine(engine: &mut HostEngine) {
    engine
        .apply(CommandEnvelope::new(ROOM_ID, HostCommand::EnsureRoom))
        .expect("ensure room should succeed");

    for i in 0..1_000_u64 {
        engine
            .apply(CommandEnvelope::new(
                ROOM_ID,
                HostCommand::MapSet {
                    namespace: NAMESPACE.to_string(),
                    key: format!("seed-{i}"),
                    value: json!(i),
                },
            ))
            .expect("seed map set should succeed");
    }
}

fn submit_json(engine: *mut as_host_engine, payload: &[u8]) -> (as_status, usize) {
    let view = as_bytes_view {
        ptr: payload.as_ptr(),
        len: payload.len(),
    };
    let mut out = as_bytes_owned {
        ptr: ptr::null_mut(),
        len: 0,
    };

    let status = unsafe {
        // SAFETY: engine pointer is created by as_host_engine_new and remains valid
        // for the life of the benchmark state. payload points to immutable bytes
        // valid for this call, and out points to stack storage owned by this frame.
        as_host_submit_command_json(engine, view, &mut out)
    };

    let out_len = out.len;

    if !out.ptr.is_null() && out_len > 0 {
        unsafe {
            // SAFETY: ownership of out is returned by as_host_submit_command_json.
            as_bytes_owned_free(out)
        };
    }

    (status, out_len)
}

struct FfiBenchState {
    engine: *mut as_host_engine,
    noop: Vec<u8>,
    map_set: Vec<u8>,
    request_server_pack: Vec<u8>,
    blob_set_50kb: Vec<u8>,
}

impl FfiBenchState {
    fn new() -> Self {
        let mut engine = ptr::null_mut();
        let status = unsafe {
            // SAFETY: passing a valid pointer to receive the engine handle.
            as_host_engine_new(&mut engine)
        };
        assert_eq!(status, as_status::AS_OK, "as_host_engine_new must succeed");
        assert!(!engine.is_null(), "engine handle must be non-null");

        let ensure_room = ensure_room_json_bytes();
        let (ensure_status, _) = submit_json(engine, &ensure_room);
        assert_eq!(
            ensure_status,
            as_status::AS_OK,
            "ensure room must succeed for ffi benchmark state"
        );

        for i in 0..1_000_u64 {
            let seed_cmd = command_json_bytes(json!({
                "MapSet": {
                    "namespace": NAMESPACE,
                    "key": format!("seed-{i}"),
                    "value": i
                }
            }));
            let (seed_status, _) = submit_json(engine, &seed_cmd);
            assert_eq!(seed_status, as_status::AS_OK, "seed map set must succeed");
        }

        Self {
            engine,
            noop: noop_json_bytes(),
            map_set: map_set_json_bytes(),
            request_server_pack: request_server_pack_json_bytes(),
            blob_set_50kb: blob_set_50kb_json_bytes(),
        }
    }
}

impl Drop for FfiBenchState {
    fn drop(&mut self) {
        if self.engine.is_null() {
            return;
        }

        let _ = unsafe {
            // SAFETY: engine was allocated by as_host_engine_new and is freed once here.
            as_host_engine_free(self.engine)
        };
        self.engine = ptr::null_mut();
    }
}

fn bench_engine_direct(c: &mut Criterion) {
    let mut engine = HostEngine::new();
    seed_host_engine(&mut engine);

    c.bench_function("engine_direct_noop", |b| {
        b.iter(|| {
            let result = engine
                .apply(CommandEnvelope::new(ROOM_ID, HostCommand::Noop))
                .expect("noop should succeed");
            black_box(result.events.len())
        })
    });

    c.bench_function("engine_direct_map_set", |b| {
        b.iter(|| {
            let result = engine
                .apply(CommandEnvelope::new(
                    ROOM_ID,
                    HostCommand::MapSet {
                        namespace: NAMESPACE.to_string(),
                        key: "fixed-map-key".to_string(),
                        value: json!({ "n": 42 }),
                    },
                ))
                .expect("map set should succeed");
            black_box(result.events.len())
        })
    });

    c.bench_function("engine_direct_request_server_pack_1k", |b| {
        b.iter(|| {
            let result = engine
                .apply(CommandEnvelope::new(
                    ROOM_ID,
                    HostCommand::RequestServerPack {
                        known_ids: Vec::new(),
                    },
                ))
                .expect("request server pack should succeed");
            black_box(result.events.len())
        })
    });

    c.bench_function("engine_direct_blob_set_50kb", |b| {
        let blob_b64 = base64::engine::general_purpose::STANDARD.encode(vec![7_u8; BLOB_SIZE_BYTES]);
        b.iter(|| {
            let result = engine
                .apply(CommandEnvelope::new(
                    ROOM_ID,
                    HostCommand::BlobSet {
                        namespace: NAMESPACE.to_string(),
                        hash: "blob-50kb-fixed".to_string(),
                        data_b64: blob_b64.clone(),
                    },
                ))
                .expect("blob set should succeed");
            black_box(result.events.len())
        })
    });
}

fn bench_ffi_json(c: &mut Criterion) {
    let state = FfiBenchState::new();

    c.bench_function("ffi_submit_json_noop", |b| {
        b.iter(|| {
            let (status, out_len) = submit_json(state.engine, &state.noop);
            assert_eq!(status, as_status::AS_OK, "noop ffi submit should succeed");
            black_box(out_len)
        })
    });

    c.bench_function("ffi_submit_json_map_set", |b| {
        b.iter(|| {
            let (status, out_len) = submit_json(state.engine, &state.map_set);
            assert_eq!(status, as_status::AS_OK, "map set ffi submit should succeed");
            black_box(out_len)
        })
    });

    c.bench_function("ffi_submit_json_request_server_pack_1k", |b| {
        b.iter(|| {
            let (status, out_len) = submit_json(state.engine, &state.request_server_pack);
            assert_eq!(
                status,
                as_status::AS_OK,
                "request server pack ffi submit should succeed"
            );
            black_box(out_len)
        })
    });

    c.bench_function("ffi_submit_json_blob_set_50kb", |b| {
        b.iter(|| {
            let (status, out_len) = submit_json(state.engine, &state.blob_set_50kb);
            assert_eq!(status, as_status::AS_OK, "blob set ffi submit should succeed");
            black_box(out_len)
        })
    });
}

criterion_group!(benches, bench_engine_direct, bench_ffi_json);
criterion_main!(benches);
