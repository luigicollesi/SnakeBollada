#![allow(dead_code)]

mod actor_table;
mod actor_vec;
mod metrics;
mod transition_score;
mod weights;

pub(crate) use actor_table::ActorTable;
pub(crate) use actor_vec::ActorVec;
pub(crate) use metrics::ActorUtilityMetrics;
pub(crate) use transition_score::TransitionScore;
pub(crate) use weights::StrategicWeights;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorSnapshot {
    pub(crate) metrics: ActorUtilityMetrics,
    pub(crate) weights: StrategicWeights,
}

impl ActorSnapshot {
    pub(crate) fn new(metrics: ActorUtilityMetrics, weights: StrategicWeights) -> Self {
        Self { metrics, weights }
    }
}
