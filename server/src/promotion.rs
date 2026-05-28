//! Promotion propose / validate / apply (topology Phase C).

use ed25519_dalek::SigningKey;
use nodalmerge_core::{
    Hash, MapOp, Op, PromotionApplied, PromotionLifecycle, PromotionProposed, PromotionReasonClass,
    PromotionRejected, PromotionValidated,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::lineage::snapshot_room_canonical_hash;
use crate::promotion_metrics::PromotionTimer;
use crate::room::{Room, Rooms};

const AUDIT_KEY_PREFIX: &str = "_topology/promotion/";

struct PromotionQueueGuard {
    rooms: Rooms,
    active: bool,
    _permit: Option<OwnedSemaphorePermit>,
}

impl Drop for PromotionQueueGuard {
    fn drop(&mut self) {
        if self.active {
            record_promotion_queue_inflight(&self.rooms);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionRecord {
    pub parent_room_id: String,
    pub child_room_id: String,
    pub child_checkpoint_hash: String,
    pub payload_ref: String,
    pub parent_canonical_hash_at_propose: String,
    pub parent_canonical_hash_at_validate: Option<String>,
    pub lifecycle: PromotionLifecycle,
    pub proposal_digest: String,
    pub validation_digest: Option<String>,
    pub parent_new_canonical_hash: Option<String>,
    pub audit_key: Option<String>,
}

pub async fn process_topology_propose_promotion(
    rooms: &Rooms,
    msg: &Value,
) -> Result<PromotionProposed, PromotionRejected> {
    let timer = PromotionTimer::start("propose");
    let _queue = acquire_promotion_queue_slot(rooms)
        .await
        .map_err(queue_rejection)?;
    let result = process_topology_propose_promotion_inner(rooms, msg).await;
    match &result {
        Ok(_) => timer.finish("ok", None),
        Err(rejected) => timer.finish("rejected", Some(rejected.reason_class)),
    }
    result
}

async fn process_topology_propose_promotion_inner(
    rooms: &Rooms,
    msg: &Value,
) -> Result<PromotionProposed, PromotionRejected> {
    let parent_room_id = require_str(msg, "parent_room_id")?;
    let child_room_id = require_str(msg, "child_room_id")?;
    let child_checkpoint_hash = parse_checkpoint_hash(msg, "child_checkpoint_hash")?;
    let payload_ref = require_str(msg, "payload_ref")?;
    let idempotency_key = msg
        .get("idempotency_key")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    if let Some(key) = idempotency_key.as_ref() {
        if let Some(record) = rooms.promotion_store.get(key).await {
            if record.parent_room_id == parent_room_id
                && record.child_room_id == child_room_id
                && record.payload_ref == payload_ref
            {
                return Ok(PromotionProposed {
                    proposal_id: key.clone(),
                    parent_room_id: record.parent_room_id.clone(),
                    child_room_id: record.child_room_id.clone(),
                    child_checkpoint_hash: record.child_checkpoint_hash.clone(),
                    payload_ref: record.payload_ref.clone(),
                    proposal_digest: record.proposal_digest.clone(),
                });
            }
            return Err(rejected(
                PromotionReasonClass::ApplyConflict,
                "idempotency_key already used with different inputs",
            ));
        }
    }

    let lineage = load_child_lineage(rooms, &child_room_id, &parent_room_id).await?;
    if lineage.promotion_policy_id != "promotion-based" {
        return Err(rejected(
            PromotionReasonClass::PolicyDenied,
            "promotion_policy_id is not promotion-based",
        ));
    }

    let child = rooms.get_or_create(&child_room_id).await;
    let actual_child_hash = snapshot_room_canonical_hash(&child)
        .await
        .map_err(lineage_to_promotion)?;
    if actual_child_hash != child_checkpoint_hash {
        return Err(rejected(
            PromotionReasonClass::ChildCheckpointMismatch,
            "child canonical_hash does not match child_checkpoint_hash",
        ));
    }

    let parent = rooms.get_or_create(&parent_room_id).await;
    let parent_hash_at_propose = snapshot_room_canonical_hash(&parent)
        .await
        .map_err(lineage_to_promotion)?;

    let digest_input = format!(
        "{parent_room_id}|{child_room_id}|{child_checkpoint_hash}|{payload_ref}|{}",
        idempotency_key.as_deref().unwrap_or("")
    );
    let proposal_digest = Hash::of(digest_input.as_bytes()).to_hex();
    let proposal_id = idempotency_key.unwrap_or_else(|| proposal_digest.clone());

    let record = PromotionRecord {
        parent_room_id: parent_room_id.clone(),
        child_room_id: child_room_id.clone(),
        child_checkpoint_hash: child_checkpoint_hash.clone(),
        payload_ref: payload_ref.clone(),
        parent_canonical_hash_at_propose: parent_hash_at_propose,
        parent_canonical_hash_at_validate: None,
        lifecycle: PromotionLifecycle::Proposed,
        proposal_digest: proposal_digest.clone(),
        validation_digest: None,
        parent_new_canonical_hash: None,
        audit_key: None,
    };

    if let Some(existing) = rooms.promotion_store.get(&proposal_id).await {
        if existing.lifecycle != PromotionLifecycle::Proposed
            || existing.parent_room_id != parent_room_id
        {
            return Err(rejected(
                PromotionReasonClass::ApplyConflict,
                "proposal_id already used with different inputs",
            ));
        }
        return Ok(PromotionProposed {
            proposal_id,
            parent_room_id,
            child_room_id,
            child_checkpoint_hash,
            payload_ref,
            proposal_digest: existing.proposal_digest.clone(),
        });
    }
    rooms.promotion_store.insert(&proposal_id, record).await;

    Ok(PromotionProposed {
        proposal_id,
        parent_room_id,
        child_room_id,
        child_checkpoint_hash,
        payload_ref,
        proposal_digest,
    })
}

pub async fn process_topology_validate_promotion(
    rooms: &Rooms,
    msg: &Value,
) -> Result<PromotionValidated, PromotionRejected> {
    let timer = PromotionTimer::start("validate");
    let _queue = acquire_promotion_queue_slot(rooms)
        .await
        .map_err(queue_rejection)?;
    let result = process_topology_validate_promotion_inner(rooms, msg).await;
    match &result {
        Ok(_) => timer.finish("ok", None),
        Err(rejected) => timer.finish("rejected", Some(rejected.reason_class)),
    }
    result
}

async fn process_topology_validate_promotion_inner(
    rooms: &Rooms,
    msg: &Value,
) -> Result<PromotionValidated, PromotionRejected> {
    let proposal_id = require_str(msg, "proposal_id")?;
    let mut record = rooms
        .promotion_store
        .get(&proposal_id)
        .await
        .ok_or_else(|| rejected(PromotionReasonClass::NotFound, "proposal not found"))?;

    if record.lifecycle == PromotionLifecycle::Applied {
        return Ok(PromotionValidated {
            proposal_id,
            validation_digest: record
                .validation_digest
                .clone()
                .unwrap_or_else(|| record.proposal_digest.clone()),
        });
    }

    load_child_lineage(rooms, &record.child_room_id, &record.parent_room_id).await?;

    let child = rooms.get_or_create(&record.child_room_id).await;
    let child_hash = snapshot_room_canonical_hash(&child)
        .await
        .map_err(lineage_to_promotion)?;
    if child_hash != record.child_checkpoint_hash {
        return Err(rejected(
            PromotionReasonClass::ChildCheckpointMismatch,
            "child checkpoint changed since proposal",
        ));
    }

    let parent = rooms.get_or_create(&record.parent_room_id).await;
    let parent_hash = snapshot_room_canonical_hash(&parent)
        .await
        .map_err(lineage_to_promotion)?;

    if parent_hash != record.parent_canonical_hash_at_propose {
        return Err(rejected(
            PromotionReasonClass::StaleParent,
            "parent canonical hash moved since proposal",
        ));
    }

    let validation_digest = Hash::of(
        format!(
            "validate|{}|{}|{}",
            proposal_id, record.proposal_digest, parent_hash
        )
        .as_bytes(),
    )
    .to_hex();

    record.lifecycle = PromotionLifecycle::Validated;
    record.parent_canonical_hash_at_validate = Some(parent_hash);
    record.validation_digest = Some(validation_digest.clone());

    rooms.promotion_store.update(&proposal_id, record).await;

    Ok(PromotionValidated {
        proposal_id,
        validation_digest,
    })
}

pub async fn process_topology_apply_promotion(
    rooms: &Rooms,
    server_key: &SigningKey,
    msg: &Value,
) -> Result<PromotionApplied, PromotionRejected> {
    let timer = PromotionTimer::start("apply");
    let _queue = acquire_promotion_queue_slot(rooms)
        .await
        .map_err(queue_rejection)?;
    let result = process_topology_apply_promotion_inner(rooms, server_key, msg).await;
    match &result {
        Ok(_) => timer.finish("ok", None),
        Err(rejected) => timer.finish("rejected", Some(rejected.reason_class)),
    }
    result
}

async fn process_topology_apply_promotion_inner(
    rooms: &Rooms,
    server_key: &SigningKey,
    msg: &Value,
) -> Result<PromotionApplied, PromotionRejected> {
    let proposal_id = require_str(msg, "proposal_id")?;

    let record_snapshot = rooms
        .promotion_store
        .get(&proposal_id)
        .await
        .ok_or_else(|| rejected(PromotionReasonClass::NotFound, "proposal not found"))?;
    let already_applied = record_snapshot.lifecycle == PromotionLifecycle::Applied;

    if already_applied {
        let parent_hash = record_snapshot
            .parent_new_canonical_hash
            .clone()
            .unwrap_or_else(|| record_snapshot.parent_canonical_hash_at_propose.clone());
        let audit_key = record_snapshot
            .audit_key
            .clone()
            .unwrap_or_else(|| format!("{AUDIT_KEY_PREFIX}{proposal_id}"));
        return Ok(PromotionApplied {
            proposal_id,
            parent_room_id: record_snapshot.parent_room_id,
            parent_new_canonical_hash: parent_hash,
            audit_key,
        });
    }

    if record_snapshot.lifecycle != PromotionLifecycle::Validated {
        return Err(rejected(
            PromotionReasonClass::NotValidated,
            "proposal must be validated before apply",
        ));
    }

    let parent = rooms.get_or_create(&record_snapshot.parent_room_id).await;
    let parent_hash_now = snapshot_room_canonical_hash(&parent)
        .await
        .map_err(lineage_to_promotion)?;

    let expected_parent = record_snapshot
        .parent_canonical_hash_at_validate
        .as_ref()
        .unwrap_or(&record_snapshot.parent_canonical_hash_at_propose);

    if &parent_hash_now != expected_parent {
        return Err(rejected(
            PromotionReasonClass::StaleParent,
            "parent canonical hash moved since validation",
        ));
    }

    let audit_key = format!("{AUDIT_KEY_PREFIX}{proposal_id}");
    let audit_payload = serde_json::json!({
        "proposal_id": proposal_id,
        "child_room_id": record_snapshot.child_room_id,
        "child_checkpoint_hash": record_snapshot.child_checkpoint_hash,
        "payload_ref": record_snapshot.payload_ref,
        "proposal_digest": record_snapshot.proposal_digest,
        "validation_digest": record_snapshot.validation_digest,
    });
    commit_audit_node(
        &parent,
        server_key,
        &audit_key,
        audit_payload.to_string().into_bytes(),
    )
    .await?;

    let parent_new_canonical_hash = snapshot_room_canonical_hash(&parent)
        .await
        .map_err(lineage_to_promotion)?;

    let mut applied_record = record_snapshot.clone();
    applied_record.lifecycle = PromotionLifecycle::Applied;
    applied_record.parent_new_canonical_hash = Some(parent_new_canonical_hash.clone());
    applied_record.audit_key = Some(audit_key.clone());
    rooms
        .promotion_store
        .update(&proposal_id, applied_record)
        .await;

    Ok(PromotionApplied {
        proposal_id,
        parent_room_id: record_snapshot.parent_room_id,
        parent_new_canonical_hash,
        audit_key,
    })
}

async fn load_child_lineage(
    rooms: &Rooms,
    child_room_id: &str,
    parent_room_id: &str,
) -> Result<nodalmerge_core::RoomLineage, PromotionRejected> {
    let child = rooms.get_or_create(child_room_id).await;
    let mut lineage = child.lineage.read().await.clone();
    if lineage.is_none() {
        lineage = rooms.lineage_store.get_lineage(child_room_id).await;
        if let Some(lin) = lineage.as_ref() {
            *child.lineage.write().await = Some(lin.clone());
        }
    }
    let lineage = lineage.ok_or_else(|| {
        rejected(
            PromotionReasonClass::InvalidLineage,
            "child room has no lineage metadata",
        )
    })?;
    if lineage.parent_room_id != parent_room_id {
        return Err(rejected(
            PromotionReasonClass::InvalidLineage,
            "child lineage parent_room_id does not match parent_room_id",
        ));
    }
    Ok(lineage)
}

async fn commit_audit_node(
    room: &Room,
    server_key: &SigningKey,
    audit_key: &str,
    value: Vec<u8>,
) -> Result<(), PromotionRejected> {
    let mut graph = room.graph.write().await;
    graph
        .apply_local(
            server_key,
            0,
            vec![Op::Map(MapOp::Set {
                key: audit_key.to_string(),
                value,
            })],
        )
        .map_err(|e| {
            rejected(
                PromotionReasonClass::ApplyConflict,
                format!("failed to commit promotion audit node: {e}"),
            )
        })?;
    Ok(())
}

fn require_str(msg: &Value, field: &str) -> Result<String, PromotionRejected> {
    msg.get(field)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            rejected(
                PromotionReasonClass::InvalidLineage,
                format!("{field} is required"),
            )
        })
}

fn parse_checkpoint_hash(msg: &Value, field: &str) -> Result<String, PromotionRejected> {
    let hash = require_str(msg, field)?;
    if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(rejected(
            PromotionReasonClass::InvalidLineage,
            format!("{field} must be 64-char hex"),
        ));
    }
    Ok(hash.to_ascii_lowercase())
}

fn rejected(
    reason_class: PromotionReasonClass,
    reason_message: impl Into<String>,
) -> PromotionRejected {
    PromotionRejected {
        reason_class,
        reason_message: reason_message.into(),
    }
}

fn room_promotion_semaphore(rooms: &Rooms) -> Arc<Semaphore> {
    let mut guard = rooms
        .promotion_semaphore
        .lock()
        .expect("promotion_semaphore poisoned");
    let refresh = guard
        .as_ref()
        .map(|(cap, _)| *cap != rooms.promotion_max_inflight)
        .unwrap_or(true);
    if refresh {
        *guard = Some((
            rooms.promotion_max_inflight,
            Arc::new(Semaphore::new(rooms.promotion_max_inflight)),
        ));
    }
    Arc::clone(&guard.as_ref().expect("promotion semaphore initialized").1)
}

fn record_promotion_queue_inflight(rooms: &Rooms) {
    let inflight = if rooms.promotion_max_inflight == 0 {
        0
    } else {
        let sem = room_promotion_semaphore(rooms);
        rooms
            .promotion_max_inflight
            .saturating_sub(sem.available_permits())
    };
    metrics::gauge!("nodalmerge_topology_promotion_inflight").set(inflight as f64);
}

fn record_promotion_queue_depth(rooms: &Rooms) {
    let depth = rooms.promotion_waiting.load(Ordering::Acquire) as f64;
    metrics::gauge!("nodalmerge_topology_promotion_queue_depth").set(depth);
}

async fn acquire_promotion_queue_slot(rooms: &Rooms) -> Result<PromotionQueueGuard, &'static str> {
    if rooms.promotion_max_inflight == 0 {
        return Ok(PromotionQueueGuard {
            rooms: rooms.clone(),
            active: false,
            _permit: None,
        });
    }
    let sem = room_promotion_semaphore(rooms);
    if let Ok(permit) = sem.clone().try_acquire_owned() {
        record_promotion_queue_inflight(rooms);
        return Ok(PromotionQueueGuard {
            rooms: rooms.clone(),
            active: true,
            _permit: Some(permit),
        });
    }
    if rooms.promotion_max_queue == 0 {
        return Err("topology promotion queue disabled and inflight saturated");
    }

    let prev_waiting = rooms.promotion_waiting.fetch_add(1, Ordering::AcqRel);
    record_promotion_queue_depth(rooms);
    if prev_waiting >= rooms.promotion_max_queue {
        rooms.promotion_waiting.fetch_sub(1, Ordering::AcqRel);
        record_promotion_queue_depth(rooms);
        return Err("topology promotion queue is full");
    }
    let queue_started = Instant::now();
    let permit = sem.acquire_owned().await.map_err(|_| {
        rooms.promotion_waiting.fetch_sub(1, Ordering::AcqRel);
        record_promotion_queue_depth(rooms);
        "topology promotion queue unavailable"
    })?;
    rooms.promotion_waiting.fetch_sub(1, Ordering::AcqRel);
    record_promotion_queue_depth(rooms);
    record_promotion_queue_inflight(rooms);
    metrics::histogram!("nodalmerge_topology_promotion_queue_wait_seconds")
        .record(queue_started.elapsed().as_secs_f64());
    metrics::counter!("nodalmerge_topology_promotion_queued_total", "outcome" => "admitted")
        .increment(1);
    Ok(PromotionQueueGuard {
        rooms: rooms.clone(),
        active: true,
        _permit: Some(permit),
    })
}

fn queue_rejection(message: &'static str) -> PromotionRejected {
    rejected(PromotionReasonClass::ApplyConflict, message)
}

fn lineage_to_promotion(err: nodalmerge_core::LineageRejected) -> PromotionRejected {
    rejected(
        PromotionReasonClass::InvalidLineage,
        format!("{}: {}", err.reason_class.as_str(), err.reason_message),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{NoPersistence, SharedPersistence};

    #[tokio::test]
    async fn promotion_queue_rejects_when_queue_is_full() {
        let persistence: SharedPersistence = Arc::new(NoPersistence);
        let mut rooms = Rooms::new(SigningKey::from_bytes(&[0x11; 32]), persistence, 512, 0, 0);
        rooms.promotion_max_inflight = 1;
        rooms.promotion_max_queue = 1;

        let held = acquire_promotion_queue_slot(&rooms)
            .await
            .expect("first slot acquired");
        let waiter_rooms = rooms.clone();
        let waiter = tokio::spawn(async move {
            acquire_promotion_queue_slot(&waiter_rooms)
                .await
                .expect("waiter queued")
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let rejected = acquire_promotion_queue_slot(&rooms).await;
        assert!(rejected.is_err(), "third waiter should be rejected");

        drop(held);
        let _ = waiter.await.expect("waiter task joined");
    }

    #[tokio::test]
    async fn promotion_queue_rejects_when_waiting_disabled() {
        let persistence: SharedPersistence = Arc::new(NoPersistence);
        let mut rooms = Rooms::new(SigningKey::from_bytes(&[0x12; 32]), persistence, 512, 0, 0);
        rooms.promotion_max_inflight = 1;
        rooms.promotion_max_queue = 0;

        let held = acquire_promotion_queue_slot(&rooms)
            .await
            .expect("first slot acquired");
        let rejected = acquire_promotion_queue_slot(&rooms).await;
        assert!(rejected.is_err(), "waiting disabled should reject immediately");
        drop(held);
    }
}
