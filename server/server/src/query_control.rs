use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use metrics::{counter, gauge, histogram};
use serde_json::{json, Value};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::room::{ProjectionState, QuerySpecState, Room};

#[derive(Debug, Clone, Copy)]
struct QueryBuildLimits {
    max_rows: usize,
    max_inflight: usize,
    max_queue: usize,
}

impl QueryBuildLimits {
    fn from_env() -> Self {
        Self {
            max_rows: parse_limit("NODALMERGE_QUERY_BUILD_MAX_ROWS", 50_000),
            max_inflight: parse_limit("NODALMERGE_QUERY_BUILD_MAX_INFLIGHT", 8),
            max_queue: parse_limit("NODALMERGE_QUERY_BUILD_MAX_QUEUE", 32),
        }
    }
}

struct ProjectionBuildInflightGuard {
    room: Arc<Room>,
    limits: QueryBuildLimits,
    active: bool,
    _permit: Option<OwnedSemaphorePermit>,
}

impl Drop for ProjectionBuildInflightGuard {
    fn drop(&mut self) {
        if self.active {
            record_query_build_inflight(&self.room, self.limits.max_inflight);
        }
    }
}

fn room_build_semaphore(room: &Room, capacity: usize) -> Arc<Semaphore> {
    let mut guard = room
        .projection_build_semaphore
        .lock()
        .expect("projection_build_semaphore poisoned");
    let refresh = guard
        .as_ref()
        .map(|(cap, _)| *cap != capacity)
        .unwrap_or(true);
    if refresh {
        *guard = Some((capacity, Arc::new(Semaphore::new(capacity))));
    }
    Arc::clone(&guard.as_ref().expect("semaphore initialized").1)
}

fn record_query_build_inflight(room: &Room, capacity: usize) {
    let inflight = if capacity == 0 {
        0
    } else {
        let sem = room_build_semaphore(room, capacity);
        capacity.saturating_sub(sem.available_permits())
    };
    gauge!("nodalmerge_query_build_inflight", "room" => room.room_id.clone())
        .set(inflight as f64);
}

fn record_query_build_queue_depth(room: &Room) {
    let depth = room
        .projection_build_waiting
        .load(Ordering::Acquire) as f64;
    gauge!("nodalmerge_query_build_queue_depth", "room" => room.room_id.clone()).set(depth);
}

async fn acquire_projection_build_slot(
    room: Arc<Room>,
    limits: QueryBuildLimits,
    projection_id: &str,
) -> Result<ProjectionBuildInflightGuard, Value> {
    if limits.max_inflight == 0 {
        return Ok(ProjectionBuildInflightGuard {
            room,
            limits,
            active: false,
            _permit: None,
        });
    }

    let sem = room_build_semaphore(&room, limits.max_inflight);
    if let Ok(permit) = sem.clone().try_acquire_owned() {
        record_query_build_inflight(&room, limits.max_inflight);
        return Ok(ProjectionBuildInflightGuard {
            room,
            limits,
            active: true,
            _permit: Some(permit),
        });
    }

    if limits.max_queue == 0 {
        return Err(json!({
            "type": "projection.build.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.query_backpressure",
            "reason_message": "projection build wait queue is disabled (max_queue=0)"
        }));
    }

    let prev_waiting = room
        .projection_build_waiting
        .fetch_add(1, Ordering::AcqRel);
    record_query_build_queue_depth(&room);
    if prev_waiting >= limits.max_queue {
        room.projection_build_waiting
            .fetch_sub(1, Ordering::AcqRel);
        record_query_build_queue_depth(&room);
        return Err(json!({
            "type": "projection.build.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.query_backpressure",
            "reason_message": "projection build wait queue is full"
        }));
    }

    let queue_wait_started = Instant::now();
    let permit = sem.acquire_owned().await.map_err(|_| {
        room.projection_build_waiting
            .fetch_sub(1, Ordering::AcqRel);
        record_query_build_queue_depth(&room);
        json!({
            "type": "projection.build.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.query_backpressure",
            "reason_message": "projection build semaphore closed"
        })
    })?;
    let queue_wait = queue_wait_started.elapsed().as_secs_f64();
    if queue_wait > 0.0 {
        histogram!("nodalmerge_query_build_queue_wait_seconds").record(queue_wait);
        counter!("nodalmerge_query_build_queued_total", "outcome" => "admitted")
            .increment(1);
    }
    room.projection_build_waiting
        .fetch_sub(1, Ordering::AcqRel);
    record_query_build_queue_depth(&room);
    record_query_build_inflight(&room, limits.max_inflight);

    Ok(ProjectionBuildInflightGuard {
        room,
        limits,
        active: true,
        _permit: Some(permit),
    })
}

pub async fn process_query_register(room: &Room, msg: &Value) -> Value {
    let query_spec_id = msg
        .get("query_spec_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let version = msg
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let descriptor = msg.get("descriptor").cloned().unwrap_or_else(|| json!({}));

    if query_spec_id.is_empty() || version.is_empty() || !descriptor.is_object() {
        return json!({
            "type": "query.register.rejected",
            "query_spec_id": query_spec_id,
            "version": version,
            "reason_class": "reject.invalid_payload",
            "reason_message": "query.register requires query_spec_id, version, and object descriptor"
        });
    }

    let mut specs = room.query_specs.write().await;
    specs.insert(
        query_spec_id.clone(),
        QuerySpecState {
            query_spec_id: query_spec_id.clone(),
            version: version.clone(),
            descriptor,
        },
    );
    json!({
        "type": "query.registered",
        "query_spec_id": query_spec_id,
        "version": version,
        "accepted": true
    })
}

pub async fn process_projection_build(room: Arc<Room>, msg: &Value) -> Value {
    process_projection_build_with_limits(room, msg, QueryBuildLimits::from_env()).await
}

async fn process_projection_build_with_limits(
    room: Arc<Room>,
    msg: &Value,
    limits: QueryBuildLimits,
) -> Value {
    let started = Instant::now();
    let projection_id = msg
        .get("projection_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let query_spec_id = msg
        .get("query_spec_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if projection_id.is_empty() || query_spec_id.is_empty() {
        let response = json!({
            "type": "projection.build.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.invalid_payload",
            "reason_message": "projection.build requires projection_id and query_spec_id"
        });
        record_query_build_result(started, "rejected", None);
        return response;
    }

    let _inflight_guard = match acquire_projection_build_slot(
        Arc::clone(&room),
        limits,
        &projection_id,
    )
    .await
    {
        Ok(guard) => guard,
        Err(rejected) => {
            record_query_build_result(started, "rejected", Some("queue_full"));
            return rejected;
        }
    };

    let spec = {
        let specs = room.query_specs.read().await;
        match specs.get(&query_spec_id) {
            Some(spec) => spec.clone(),
            None => {
                let response = json!({
                    "type": "projection.build.rejected",
                    "projection_id": projection_id,
                    "reason_class": "reject.query_spec_not_found",
                    "reason_message": "projection.build.query_spec_id is not registered"
                });
                record_query_build_result(started, "rejected", None);
                return response;
            }
        }
    };

    let graph = room.graph.read().await;
    let canonical = graph.resolve();
    let canonical_seq = graph.node_count() as u64;
    drop(graph);

    let selector = msg
        .get("target_checkpoint")
        .and_then(|v| v.get("selector"))
        .and_then(Value::as_str)
        .unwrap_or("latest");
    if selector != "latest" && selector != "seq" {
        let response = json!({
            "type": "projection.build.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.checkpoint_selector_invalid",
            "reason_message": "selector must be latest or seq"
        });
        record_query_build_result(started, "rejected", None);
        return response;
    }
    if selector == "seq" {
        let requested = msg
            .get("target_checkpoint")
            .and_then(|v| v.get("canonical_seq"))
            .and_then(Value::as_u64);
        if requested.is_none() || requested.unwrap_or(0) != canonical_seq {
            let response = json!({
                "type": "projection.build.rejected",
                "projection_id": projection_id,
                "reason_class": "reject.checkpoint_not_found",
                "reason_message": "target checkpoint does not resolve to current canonical sequence"
            });
            record_query_build_result(started, "rejected", None);
            return response;
        }
    }

    let prefix = spec
        .descriptor
        .get("prefix")
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut ordered: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (k, v) in canonical {
        if let Some(pfx) = prefix.as_deref() {
            if !k.starts_with(pfx) {
                continue;
            }
        }
        ordered.insert(k, v);
    }

    if limits.max_rows > 0 && ordered.len() > limits.max_rows {
        let response = json!({
            "type": "projection.build.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.query_backpressure",
            "reason_message": format!(
                "projection.build row count {} exceeds configured limit {}",
                ordered.len(),
                limits.max_rows
            )
        });
        record_query_build_result(started, "rejected", Some("row_limit"));
        return response;
    }

    let mut rows = Vec::with_capacity(ordered.len());
    let mut digest_seed = String::new();
    for (k, v) in ordered {
        let value = String::from_utf8_lossy(&v).to_string();
        digest_seed.push_str(&k);
        digest_seed.push('=');
        digest_seed.push_str(&value);
        digest_seed.push(';');
        rows.push(json!({ "k": k, "v": value }));
    }
    let digest = nodalmerge_core::Hash::of(digest_seed.as_bytes()).to_hex();
    let canonical_hash = nodalmerge_core::Hash::of(digest_seed.as_bytes()).to_hex();
    let checkpoint = json!({
        "selector": "seq",
        "canonical_seq": canonical_seq,
        "canonical_hash": canonical_hash,
        "frontier": [format!("seq:{canonical_seq}")]
    });

    let mut projections = room.projections.write().await;
    projections.insert(
        projection_id.clone(),
        ProjectionState {
            projection_id: projection_id.clone(),
            query_spec_id,
            checkpoint: checkpoint.clone(),
            rows: rows.clone(),
            digest: digest.clone(),
            invalidated: false,
            invalidation_reason: None,
        },
    );

    let response = json!({
        "type": "projection.build.completed",
        "projection_id": projection_id,
        "checkpoint": checkpoint,
        "digest": digest
    });
    record_query_build_result(started, "ok", None);
    response
}

pub async fn process_projection_read(room: &Room, msg: &Value) -> Value {
    let projection_id = msg
        .get("projection_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if projection_id.is_empty() {
        return json!({
            "type": "projection.read.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.invalid_payload",
            "reason_message": "projection.read requires projection_id"
        });
    }
    let limit = msg.get("limit").and_then(Value::as_u64).unwrap_or(100).max(1) as usize;
    let offset = parse_offset(msg.get("page_token").and_then(Value::as_str));

    let projections = room.projections.read().await;
    let Some(proj) = projections.get(&projection_id) else {
        return json!({
            "type": "projection.read.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.projection_not_found",
            "reason_message": "projection is not registered"
        });
    };
    let end = offset.saturating_add(limit).min(proj.rows.len());
    let rows = if offset < proj.rows.len() {
        proj.rows[offset..end].to_vec()
    } else {
        Vec::new()
    };
    let next_page_token = (end < proj.rows.len()).then(|| format!("offset:{end}"));
    json!({
        "type": "projection.read.result",
        "projection_id": proj.projection_id,
        "checkpoint": proj.checkpoint,
        "rows": rows,
        "digest": proj.digest,
        "next_page_token": next_page_token
    })
}

pub async fn process_projection_invalidate(room: &Room, msg: &Value) -> Value {
    let projection_id = msg
        .get("projection_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let reason = msg
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("manual")
        .to_string();
    if projection_id.is_empty() {
        return json!({
            "type": "projection.invalidate.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.invalid_payload",
            "reason_message": "projection.invalidate requires projection_id"
        });
    }
    let mut projections = room.projections.write().await;
    let Some(proj) = projections.get_mut(&projection_id) else {
        return json!({
            "type": "projection.invalidate.rejected",
            "projection_id": projection_id,
            "reason_class": "reject.projection_not_found",
            "reason_message": "projection is not registered"
        });
    };
    proj.invalidated = true;
    proj.invalidation_reason = Some(reason.clone());
    json!({
        "type": "projection.invalidated",
        "projection_id": projection_id,
        "reason": reason,
        "invalidated_at_hlc": 0
    })
}

pub async fn process_projection_list(room: &Room, msg: &Value) -> Value {
    let query_spec_id = msg
        .get("query_spec_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let state_filter = msg
        .get("state_filter")
        .and_then(Value::as_str)
        .map(str::to_string);

    if let Some(query_spec_id) = query_spec_id.as_deref() {
        let specs = room.query_specs.read().await;
        if !specs.contains_key(query_spec_id) {
            return json!({
                "type": "projection.list.rejected",
                "query_spec_id": query_spec_id,
                "reason_class": "reject.query_spec_not_found",
                "reason_message": "query spec is not registered"
            });
        }
    }

    let projections = room.projections.read().await;
    let mut keys: Vec<String> = projections.keys().cloned().collect();
    keys.sort();
    let mut items = Vec::new();
    for key in keys {
        let Some(proj) = projections.get(&key) else {
            continue;
        };
        if let Some(query_spec_id) = query_spec_id.as_deref() {
            if proj.query_spec_id != query_spec_id {
                continue;
            }
        }
        if let Some(filter) = state_filter.as_deref() {
            if filter.eq_ignore_ascii_case("active") && proj.invalidated {
                continue;
            }
            if filter.eq_ignore_ascii_case("invalidated") && !proj.invalidated {
                continue;
            }
        }
        items.push(json!({
            "projection_id": proj.projection_id,
            "query_spec_id": proj.query_spec_id,
            "state": if proj.invalidated { "invalidated" } else { "active" },
            "digest": proj.digest,
            "checkpoint": proj.checkpoint,
            "invalidation_reason": proj.invalidation_reason
        }));
    }
    json!({
        "type": "projection.list.result",
        "query_spec_id": query_spec_id,
        "items": items,
        "cursor": Value::Null
    })
}

pub async fn process_replay_read_range(room: &Room, msg: &Value) -> Value {
    let key_prefix = msg
        .get("key_prefix")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if key_prefix.is_empty() {
        return json!({
            "type": "replay.read-range.rejected",
            "reason_class": "reject.invalid_payload",
            "reason_message": "replay.read-range requires non-empty key_prefix"
        });
    }

    let from_lamport = msg
        .get("from_lamport")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let limit = msg
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .max(1) as usize;
    let offset = parse_offset(msg.get("cursor").and_then(Value::as_str));

    let graph = room.graph.read().await;
    let mut events = graph
        .all_nodes()
        .into_iter()
        .filter(|node| node.transaction.lamport >= from_lamport)
        .filter_map(|node| {
            let touched: Vec<String> = node
                .transaction
                .ops
                .iter()
                .filter_map(|op| op.key().map(str::to_string))
                .filter(|key| key.starts_with(&key_prefix))
                .collect();
            if touched.is_empty() {
                return None;
            }
            Some((node.transaction.lamport, node.id.to_hex(), touched))
        })
        .collect::<Vec<(u64, String, Vec<String>)>>();

    events.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let end = offset.saturating_add(limit).min(events.len());
    let page = if offset < events.len() {
        events[offset..end].to_vec()
    } else {
        Vec::new()
    };
    let next_cursor = (end < events.len()).then(|| format!("offset:{end}"));

    let items = page
        .into_iter()
        .map(|(lamport, node_id, touched_keys)| {
            json!({
                "lamport": lamport,
                "node_id": node_id,
                "touched_keys": touched_keys
            })
        })
        .collect::<Vec<Value>>();

    json!({
        "type": "replay.read-range.result",
        "key_prefix": key_prefix,
        "from_lamport": from_lamport,
        "items": items,
        "next_cursor": next_cursor
    })
}

fn parse_offset(page_token: Option<&str>) -> usize {
    let Some(token) = page_token else {
        return 0;
    };
    let Some(raw) = token.strip_prefix("offset:") else {
        return 0;
    };
    raw.parse::<usize>().unwrap_or(0)
}

fn parse_limit(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn record_query_build_result(started: Instant, outcome: &'static str, reason: Option<&'static str>) {
    histogram!("nodalmerge_query_build_seconds", "outcome" => outcome)
        .record(started.elapsed().as_secs_f64());
    if let Some(reason) = reason {
        counter!(
            "nodalmerge_query_build_total",
            "outcome" => outcome,
            "reason" => reason
        )
        .increment(1);
    } else {
        counter!("nodalmerge_query_build_total", "outcome" => outcome).increment(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NoPersistence, SharedPersistence};
    use std::time::Duration;
    use tokio::sync::oneshot;

    fn register_descriptor() -> Value {
        json!({ "prefix": "world/" })
    }

    fn build_msg(projection_id: &str, query_spec_id: &str) -> Value {
        json!({
            "type": "projection.build",
            "projection_id": projection_id,
            "query_spec_id": query_spec_id,
            "target_checkpoint": { "selector": "latest" }
        })
    }

    #[tokio::test]
    async fn projection_build_rejects_when_row_limit_exceeded() {
        use ed25519_dalek::SigningKey;
        use nodalmerge_core::{MapOp, Op};

        let persistence: SharedPersistence = Arc::new(NoPersistence);
        let room = Room::new("query-backpressure-rows".to_string(), persistence, 32);
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        {
            let mut graph = room.graph.write().await;
            for i in 0..2 {
                graph
                    .apply_local(
                        &sk,
                        0,
                        vec![Op::Map(MapOp::Set {
                            key: format!("world/k{i}"),
                            value: b"v".to_vec(),
                        })],
                    )
                    .expect("seed graph");
            }
        }
        process_query_register(
            room.as_ref(),
            &json!({
                "query_spec_id": "q.rooms",
                "version": "v1",
                "descriptor": register_descriptor(),
            }),
        )
        .await;
        let limits = QueryBuildLimits {
            max_rows: 1,
            max_inflight: 8,
            max_queue: 32,
        };
        let response = process_projection_build_with_limits(
            Arc::clone(&room),
            &build_msg("p.rows", "q.rooms"),
            limits,
        )
        .await;
        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("projection.build.rejected")
        );
        assert_eq!(
            response.get("reason_class").and_then(Value::as_str),
            Some("reject.query_backpressure")
        );
    }

    #[tokio::test]
    async fn projection_build_rejects_when_wait_queue_is_full() {
        let persistence: SharedPersistence = Arc::new(NoPersistence);
        let room = Room::new("query-backpressure-queue".to_string(), persistence, 32);
        process_query_register(
            room.as_ref(),
            &json!({
                "query_spec_id": "q.rooms",
                "version": "v1",
                "descriptor": register_descriptor(),
            }),
        )
        .await;
        let limits = QueryBuildLimits {
            max_rows: 50_000,
            max_inflight: 1,
            max_queue: 1,
        };

        let (release_tx, release_rx) = oneshot::channel::<()>();
        let holder_room = Arc::clone(&room);
        let holder = tokio::spawn(async move {
            let _slot = acquire_projection_build_slot(holder_room, limits, "p.hold")
                .await
                .expect("holder acquires slot");
            release_rx.await.ok();
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let waiter_room = Arc::clone(&room);
        let waiter = tokio::spawn(async move {
            acquire_projection_build_slot(waiter_room, limits, "p.waiter")
                .await
                .expect("waiter queued")
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let rejected = process_projection_build_with_limits(
            Arc::clone(&room),
            &build_msg("p.queue-full", "q.rooms"),
            limits,
        )
        .await;
        assert_eq!(
            rejected.get("type").and_then(Value::as_str),
            Some("projection.build.rejected")
        );
        assert_eq!(
            rejected.get("reason_class").and_then(Value::as_str),
            Some("reject.query_backpressure")
        );

        release_tx.send(()).ok();
        let _ = holder.await;
        let _ = waiter.await;
    }

    #[tokio::test]
    async fn projection_build_waits_in_fifo_order_then_succeeds() {
        let persistence: SharedPersistence = Arc::new(NoPersistence);
        let room = Room::new("query-backpressure-contention".to_string(), persistence, 32);
        process_query_register(
            room.as_ref(),
            &json!({
                "query_spec_id": "q.rooms",
                "version": "v1",
                "descriptor": register_descriptor(),
            }),
        )
        .await;
        let limits = QueryBuildLimits {
            max_rows: 50_000,
            max_inflight: 1,
            max_queue: 8,
        };

        let (release_tx, release_rx) = oneshot::channel::<()>();
        let holder_room = Arc::clone(&room);
        let holder = tokio::spawn(async move {
            let _slot = acquire_projection_build_slot(holder_room, limits, "p.hold")
                .await
                .expect("holder acquires slot");
            release_rx.await.ok();
        });
        tokio::time::sleep(Duration::from_millis(20)).await;

        let build_room = Arc::clone(&room);
        let build = tokio::spawn(async move {
            process_projection_build_with_limits(
                build_room,
                &build_msg("p.rooms", "q.rooms"),
                limits,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(
            !build.is_finished(),
            "second build should wait for fair queue slot"
        );

        release_tx.send(()).ok();
        let _ = holder.await;
        let response = build.await.expect("build task finished");
        assert_eq!(
            response.get("type").and_then(Value::as_str),
            Some("projection.build.completed")
        );
    }

    #[tokio::test]
    async fn replay_read_range_returns_sorted_cursor_paged_events() {
        use ed25519_dalek::SigningKey;
        use nodalmerge_core::{MapOp, Op};

        let persistence: SharedPersistence = Arc::new(NoPersistence);
        let room = Room::new("query-replay-range".to_string(), persistence, 32);
        let sk = SigningKey::from_bytes(&[0x11u8; 32]);
        {
            let mut graph = room.graph.write().await;
            graph
                .apply_local(
                    &sk,
                    0,
                    vec![Op::Map(MapOp::Set {
                        key: "world/a".to_string(),
                        value: b"1".to_vec(),
                    })],
                )
                .expect("seed graph");
            graph
                .apply_local(
                    &sk,
                    0,
                    vec![Op::Map(MapOp::Set {
                        key: "world/b".to_string(),
                        value: b"2".to_vec(),
                    })],
                )
                .expect("seed graph");
        }

        let first = process_replay_read_range(
            room.as_ref(),
            &json!({
                "type": "replay.read-range",
                "key_prefix": "world/",
                "from_lamport": 0,
                "limit": 1
            }),
        )
        .await;
        assert_eq!(
            first.get("type").and_then(Value::as_str),
            Some("replay.read-range.result")
        );
        let first_items = first
            .get("items")
            .and_then(Value::as_array)
            .expect("items array");
        assert_eq!(first_items.len(), 1);
        let cursor = first
            .get("next_cursor")
            .and_then(Value::as_str)
            .expect("next cursor");
        assert!(cursor.starts_with("offset:"));

        let second = process_replay_read_range(
            room.as_ref(),
            &json!({
                "type": "replay.read-range",
                "key_prefix": "world/",
                "from_lamport": 0,
                "limit": 10,
                "cursor": cursor
            }),
        )
        .await;
        let second_items = second
            .get("items")
            .and_then(Value::as_array)
            .expect("items array");
        assert_eq!(second_items.len(), 1);
        assert!(second.get("next_cursor").is_some());
    }
}
