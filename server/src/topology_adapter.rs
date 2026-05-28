//! Topology WS envelope builders (Wave 3 host-core parity).

use nodalmerge_core::{
    ChildRoomCreated, ChildrenListed, PromotionApplied, PromotionProposed, PromotionValidated,
    RoomLineageDescribed, TopologyWsResponse,
};

pub fn create_child_completed(created: ChildRoomCreated) -> TopologyWsResponse {
    TopologyWsResponse::CreateChildCompleted(created)
}

pub fn describe_lineage_result(described: RoomLineageDescribed) -> TopologyWsResponse {
    TopologyWsResponse::DescribeLineageResult(described)
}

pub fn list_children_result(listed: ChildrenListed) -> TopologyWsResponse {
    TopologyWsResponse::ListChildrenResult(listed)
}

pub fn propose_promotion_completed(proposed: PromotionProposed) -> TopologyWsResponse {
    TopologyWsResponse::ProposePromotionCompleted(proposed)
}

pub fn validate_promotion_completed(validated: PromotionValidated) -> TopologyWsResponse {
    TopologyWsResponse::ValidatePromotionCompleted(validated)
}

pub fn apply_promotion_completed(applied: PromotionApplied) -> TopologyWsResponse {
    TopologyWsResponse::ApplyPromotionCompleted(applied)
}
