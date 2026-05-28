//! Prometheus counters/histograms for topology promotion (Wave 3).

use std::time::Instant;

use nodalmerge_core::PromotionReasonClass;

pub struct PromotionTimer {
    stage: &'static str,
    started: Instant,
}

impl PromotionTimer {
    pub fn start(stage: &'static str) -> Self {
        Self {
            stage,
            started: Instant::now(),
        }
    }

    pub fn finish(self, outcome: &'static str, reason: Option<PromotionReasonClass>) {
        record_promotion(self.stage, outcome, reason);
        metrics::histogram!(
            "nodalmerge_topology_promotion_seconds",
            "stage" => self.stage,
            "outcome" => outcome
        )
        .record(self.started.elapsed().as_secs_f64());
    }
}

pub fn record_promotion(
    stage: &'static str,
    outcome: &'static str,
    reason: Option<PromotionReasonClass>,
) {
    if let Some(reason) = reason {
        metrics::counter!(
            "nodalmerge_topology_promotion_total",
            "stage" => stage,
            "outcome" => outcome,
            "reason" => reason.as_str()
        )
        .increment(1);
    } else {
        metrics::counter!(
            "nodalmerge_topology_promotion_total",
            "stage" => stage,
            "outcome" => outcome
        )
        .increment(1);
    }
}
