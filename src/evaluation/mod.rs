#![allow(dead_code)]

mod actor_vec;
mod hunting_transition;
mod metrics;
mod route_utility;
mod transition_score;
mod weights;

pub(crate) use actor_vec::ActorVec;
pub(crate) use hunting_transition::{evaluate_hunting_transition, HuntingTransitionScore};
pub(crate) use metrics::ActorUtilityMetrics;
pub(crate) use route_utility::{CategoryScore, RouteUtilityBreakdown};
pub(crate) use transition_score::{ActorTransitionScore, TransitionScore};
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
