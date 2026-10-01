#![allow(dead_code)]

mod context;
mod metrics;
mod score;
mod transition_score;
mod weights;

pub(crate) use context::ActorContext;
pub(crate) use metrics::ActorUtilityMetrics;
pub(crate) use transition_score::TransitionScore;
pub(crate) use weights::{NeedWeights, StrategicWeights};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorSnapshot {
    pub(crate) context: ActorContext,
    pub(crate) metrics: ActorUtilityMetrics,
    pub(crate) weights: StrategicWeights,
}

impl ActorSnapshot {
    pub(crate) fn new(context: ActorContext, metrics: ActorUtilityMetrics) -> Self {
        let weights =
            StrategicWeights::from_territory_share(&context, metrics.territory_share_milli);
        Self {
            context,
            metrics,
            weights,
        }
    }
}
