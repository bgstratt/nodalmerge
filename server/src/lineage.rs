//! Room family lineage: child creation and describe/list operations (topology Phase B).

use std::collections::{BTreeMap, HashSet};

use nodalmerge_core::{
    canonical_hash, policy_timeline_hash, replay, ChildRoomCreated, ChildRoomSummary,
    ChildrenListed, LineageReasonClass, LineageRejected, ParentCheckpoint, RoomLineage,
    RoomLineageDescribed,
};
use nodalmerge_host_core::engine::shape_welcome_server_frontier_hex;
use serde_json::Value;

use crate::room::{Room, Rooms};

const KNOWN_PROMOTION_POLICIES: &[&str] = &["reference-only", "promotion-based"];

pub fn parse_parent_checkpoint(msg: &Value) -> Result<ParentCheckpoint, LineageRejected> {
    let cp = msg.get("parent_checkpoint").ok_or_else(|| {
        rejected(
            LineageReasonClass::InvalidCheckpoint,
            "parent_checkpoint is required",
        )
    })?;

    let frontier = cp
        .get("frontier")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            rejected(
                LineageReasonClass::InvalidCheckpoint,
                "parent_checkpoint.frontier must be a non-empty array",
            )
        })?;

    let canonical_hash = cp
        .get("canonical_hash")
        .and_then(|v| v.as_str())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| {
            rejected(
                LineageReasonClass::InvalidCheckpoint,
                "parent_checkpoint.canonical_hash must be 64-char hex",
            )
        })?;

    let policy_timeline_hash = cp
        .get("policy_timeline_hash")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(ParentCheckpoint {
        frontier,
        canonical_hash,
        policy_timeline_hash,
    })
}

pub async fn snapshot_room_canonical_hash(room: &Room) -> Result<String, LineageRejected> {
    let nodes: Vec<nodalmerge_core::SyncNode> = {
        let graph = room.graph.read().await;
        let ids = graph.all_node_ids();
        graph.get_nodes(&ids).into_iter().cloned().collect()
    };

    if nodes.is_empty() {
        return Err(rejected(
            LineageReasonClass::InvalidCheckpoint,
            "room has no nodes — cannot snapshot canonical hash",
        ));
    }

    let replayed = replay(&nodes, None).map_err(|e| {
        rejected(
            LineageReasonClass::InvalidCheckpoint,
            format!("replay failed: {e}"),
        )
    })?;

    Ok(canonical_hash(&replayed.map).to_hex())
}

pub async fn snapshot_parent_checkpoint(room: &Room) -> Result<ParentCheckpoint, LineageRejected> {
    let nodes: Vec<nodalmerge_core::SyncNode> = {
        let graph = room.graph.read().await;
        let ids = graph.all_node_ids();
        graph.get_nodes(&ids).into_iter().cloned().collect()
    };

    if nodes.is_empty() {
        return Err(rejected(
            LineageReasonClass::InvalidCheckpoint,
            "parent room has no nodes — cannot snapshot checkpoint",
        ));
    }

    let replayed = replay(&nodes, None).map_err(|e| {
        rejected(
            LineageReasonClass::InvalidCheckpoint,
            format!("parent replay failed: {e}"),
        )
    })?;

    let map: BTreeMap<String, Vec<u8>> = replayed.map.clone();
    let hash = canonical_hash(&map).to_hex();
    let frontier = {
        let graph = room.graph.read().await;
        shape_welcome_server_frontier_hex(&graph.frontier())
    };

    let policy_timeline_hash = {
        let timeline = room.policy_timeline.read().await;
        Some(policy_timeline_hash(&timeline).to_hex())
    };

    Ok(ParentCheckpoint {
        frontier,
        canonical_hash: hash,
        policy_timeline_hash,
    })
}

pub async fn process_topology_create_child(
    rooms: &Rooms,
    parent_room_id: &str,
    msg: &Value,
) -> Result<ChildRoomCreated, LineageRejected> {
    let child_room_id = msg
        .get("child_room_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            rejected(
                LineageReasonClass::InvalidCheckpoint,
                "child_room_id is required",
            )
        })?
        .to_string();

    let child_purpose = msg
        .get("child_purpose")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            rejected(
                LineageReasonClass::InvalidCheckpoint,
                "child_purpose is required",
            )
        })?
        .to_string();

    let created_by = msg
        .get("created_by")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_string();

    let promotion_policy_id = msg
        .get("promotion_policy_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            rejected(
                LineageReasonClass::PolicyUnknown,
                "promotion_policy_id is required",
            )
        })?
        .to_string();

    if !KNOWN_PROMOTION_POLICIES.contains(&promotion_policy_id.as_str()) {
        return Err(rejected(
            LineageReasonClass::PolicyUnknown,
            format!("unknown promotion_policy_id: {promotion_policy_id}"),
        ));
    }

    let parent_checkpoint = parse_parent_checkpoint(msg)?;

    rooms
        .create_child_room(
            parent_room_id,
            &child_room_id,
            parent_checkpoint,
            child_purpose,
            created_by,
            promotion_policy_id,
        )
        .await
}

pub async fn process_topology_describe_lineage(
    rooms: &Rooms,
    room_id: &str,
) -> Result<RoomLineageDescribed, LineageRejected> {
    rooms.describe_lineage(room_id).await
}

pub async fn process_topology_list_children(
    rooms: &Rooms,
    parent_room_id: &str,
) -> Result<ChildrenListed, LineageRejected> {
    rooms.list_children(parent_room_id).await
}

impl Rooms {
    pub async fn create_child_room(
        &self,
        parent_room_id: &str,
        child_room_id: &str,
        expected_checkpoint: ParentCheckpoint,
        child_purpose: String,
        created_by: String,
        promotion_policy_id: String,
    ) -> Result<ChildRoomCreated, LineageRejected> {
        {
            let r = self.rooms.read().await;
            if r.get(parent_room_id).is_none() {
                return Err(rejected(
                    LineageReasonClass::ParentNotFound,
                    format!("parent room not found: {parent_room_id}"),
                ));
            }
            if r.get(child_room_id).is_some() {
                return Err(rejected(
                    LineageReasonClass::ChildAlreadyExists,
                    format!("child room already exists: {child_room_id}"),
                ));
            }
        }

        let parent = self.get_or_create(parent_room_id).await;
        let actual = snapshot_parent_checkpoint(&parent).await?;

        if actual.canonical_hash != expected_checkpoint.canonical_hash {
            return Err(rejected(
                LineageReasonClass::ParentCheckpointMismatch,
                "parent canonical_hash does not match declared parent_checkpoint",
            ));
        }

        if let (Some(expected_tl), Some(actual_tl)) = (
            expected_checkpoint.policy_timeline_hash.as_ref(),
            actual.policy_timeline_hash.as_ref(),
        ) {
            if expected_tl != actual_tl {
                return Err(rejected(
                    LineageReasonClass::ParentCheckpointMismatch,
                    "parent policy_timeline_hash does not match current parent timeline",
                ));
            }
        }

        let created_at_hlc = parent.graph.read().await.lamport();

        let lineage = RoomLineage {
            parent_room_id: parent_room_id.to_string(),
            parent_checkpoint: expected_checkpoint,
            child_purpose,
            created_by,
            created_at_hlc,
            promotion_policy_id,
        };

        let child = self.get_or_create(child_room_id).await;
        {
            let mut slot = child.lineage.write().await;
            if slot.is_some() {
                return Err(rejected(
                    LineageReasonClass::ChildAlreadyExists,
                    "child room already has lineage metadata",
                ));
            }
            *slot = Some(lineage.clone());
        }
        self.lineage_store
            .upsert_lineage(child_room_id, lineage.clone())
            .await;

        {
            let mut index = self.children_index.write().await;
            let children = index
                .entry(parent_room_id.to_string())
                .or_default();
            children.push(child_room_id.to_string());
            if let Some(cap) = self.lineage_children_index_cap {
                if children.len() > cap {
                    let drop_count = children.len() - cap;
                    children.drain(0..drop_count);
                }
            }
        }

        Ok(ChildRoomCreated {
            child_room_id: child_room_id.to_string(),
            lineage,
        })
    }

    pub async fn describe_lineage(
        &self,
        room_id: &str,
    ) -> Result<RoomLineageDescribed, LineageRejected> {
        let room = {
            let r = self.rooms.read().await;
            r.get(room_id)
                .cloned()
                .ok_or_else(|| rejected(LineageReasonClass::RoomNotFound, "room not found"))?
        };

        let lineage = room.lineage.read().await.clone();
        let mut ancestors = Vec::new();
        let mut seen = HashSet::new();
        let mut cursor = lineage.clone();

        while let Some(ref lin) = cursor {
            if !seen.insert(lin.parent_room_id.clone()) {
                break;
            }
            ancestors.push(lin.clone());
            let parent = {
                let r = self.rooms.read().await;
                r.get(&lin.parent_room_id).cloned()
            };
            cursor = match parent {
                Some(p) => p.lineage.read().await.clone(),
                None => None,
            };
        }

        Ok(RoomLineageDescribed {
            room_id: room_id.to_string(),
            lineage,
            ancestors,
        })
    }

    pub async fn list_children(
        &self,
        parent_room_id: &str,
    ) -> Result<ChildrenListed, LineageRejected> {
        {
            let r = self.rooms.read().await;
            if r.get(parent_room_id).is_none() {
                return Err(rejected(
                    LineageReasonClass::ParentNotFound,
                    format!("parent room not found: {parent_room_id}"),
                ));
            }
        }

        let child_ids: Vec<String> = {
            let index = self.children_index.read().await;
            index.get(parent_room_id).cloned().unwrap_or_default()
        };
        let child_ids = if child_ids.is_empty() {
            let from_store = self.lineage_store.list_children(parent_room_id).await;
            if !from_store.is_empty() {
                let mut index = self.children_index.write().await;
                let bounded = if let Some(cap) = self.lineage_children_index_cap {
                    if from_store.len() > cap {
                        from_store[from_store.len() - cap..].to_vec()
                    } else {
                        from_store.clone()
                    }
                } else {
                    from_store.clone()
                };
                index.insert(parent_room_id.to_string(), bounded);
            }
            from_store
        } else {
            child_ids
        };

        let mut children = Vec::new();
        for child_id in child_ids {
            let room = self.get_or_create(&child_id).await;
            let mut lineage = room.lineage.read().await.clone();
            if lineage.is_none() {
                lineage = self.lineage_store.get_lineage(&child_id).await;
                if let Some(lin) = lineage.as_ref() {
                    *room.lineage.write().await = Some(lin.clone());
                }
            }
            if let Some(lin) = lineage {
                children.push(ChildRoomSummary {
                    child_room_id: child_id,
                    child_purpose: lin.child_purpose,
                    promotion_policy_id: lin.promotion_policy_id,
                    created_by: lin.created_by,
                });
            }
        }

        Ok(ChildrenListed {
            parent_room_id: parent_room_id.to_string(),
            children,
        })
    }
}

fn rejected(
    reason_class: LineageReasonClass,
    reason_message: impl Into<String>,
) -> LineageRejected {
    LineageRejected {
        reason_class,
        reason_message: reason_message.into(),
    }
}
