#![allow(dead_code)]

mod context;
mod metrics;
mod score;
mod transition_score;
mod weights;

pub(crate) use context::ActorContext;
pub(crate) use metrics::ActorMetrics;
pub(crate) use score::ActorEvaluation;
pub(crate) use transition_score::TransitionScore;
pub(crate) use weights::{NeedWeights, StrategicWeights};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorSnapshot {
    pub(crate) context: ActorContext,
    pub(crate) metrics: ActorMetrics,
    pub(crate) weights: StrategicWeights,
}

impl ActorSnapshot {
    pub(crate) fn new(context: ActorContext, metrics: ActorMetrics) -> Self {
        let weights = StrategicWeights::from_actor(&context, &metrics);
        Self {
            context,
            metrics,
            weights,
        }
    }
}
