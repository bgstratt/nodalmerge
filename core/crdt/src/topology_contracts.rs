//! WebSocket topology control-plane response envelopes (Wave 3 host-core parity).

use serde::{Deserialize, Serialize};

use crate::room_lineage::{
    ChildRoomCreated, ChildrenListed, PromotionApplied, PromotionProposed, PromotionValidated,
    RoomLineageDescribed,
};

/// Canonical topology command responses shared by server ingress and host-core serializers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TopologyWsResponse {
    #[serde(rename = "topology.create-child.completed")]
    CreateChildCompleted(ChildRoomCreated),
    #[serde(rename = "topology.describe-lineage.result")]
    DescribeLineageResult(RoomLineageDescribed),
    #[serde(rename = "topology.list-children.result")]
    ListChildrenResult(ChildrenListed),
    #[serde(rename = "topology.propose-promotion.completed")]
    ProposePromotionCompleted(PromotionProposed),
    #[serde(rename = "topology.validate-promotion.completed")]
    ValidatePromotionCompleted(PromotionValidated),
    #[serde(rename = "topology.apply-promotion.completed")]
    ApplyPromotionCompleted(PromotionApplied),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room_lineage::{
        ChildRoomCreated, ParentCheckpoint, PromotionProposed, RoomLineage,
    };

    #[test]
    fn topology_ws_response_uses_externally_tagged_type_field() {
        let response = TopologyWsResponse::ProposePromotionCompleted(PromotionProposed {
            proposal_id: "p1".to_string(),
            parent_room_id: "parent".to_string(),
            child_room_id: "child".to_string(),
            child_checkpoint_hash: "aa".repeat(32),
            payload_ref: "artifact://x".to_string(),
            proposal_digest: "bb".repeat(32),
        });
        let json = serde_json::to_string(&response).expect("serialize");
        assert!(json.starts_with("{\"type\":\"topology.propose-promotion.completed\""));
    }

    #[test]
    fn wire_type_matches_serde_tag() {
        let response = TopologyWsResponse::CreateChildCompleted(ChildRoomCreated {
            child_room_id: "c".to_string(),
            lineage: RoomLineage {
                parent_room_id: "p".to_string(),
                parent_checkpoint: ParentCheckpoint {
                    frontier: vec![],
                    canonical_hash: "aa".repeat(32),
                    policy_timeline_hash: None,
                },
                child_purpose: "t".to_string(),
                created_by: "u".to_string(),
                created_at_hlc: 0,
                promotion_policy_id: "promotion-based".to_string(),
            },
        });
        assert_eq!(
            response.wire_type(),
            "topology.create-child.completed"
        );
    }
}

impl TopologyWsResponse {
    pub fn wire_type(&self) -> &'static str {
        match self {
            Self::CreateChildCompleted(_) => "topology.create-child.completed",
            Self::DescribeLineageResult(_) => "topology.describe-lineage.result",
            Self::ListChildrenResult(_) => "topology.list-children.result",
            Self::ProposePromotionCompleted(_) => "topology.propose-promotion.completed",
            Self::ValidatePromotionCompleted(_) => "topology.validate-promotion.completed",
            Self::ApplyPromotionCompleted(_) => "topology.apply-promotion.completed",
        }
    }
}
