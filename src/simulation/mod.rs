#![allow(dead_code)]

mod resolver;
mod state;

pub(crate) use resolver::{
    resolve_turn, EliminationCause, ForecastDelta, InstantEvent, JointAction, ResolveError,
    TurnResolution,
};
pub(crate) use state::{
    AggressionState, RulesContext, SimulatedGameState, SimulatedSnake, DEFAULT_MAX_HEALTH,
};
