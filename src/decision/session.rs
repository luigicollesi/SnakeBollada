//! Per-game FutureGraph reuse. No learned Food/Hunting intention profiles.
use std::collections::VecDeque;

use crate::decision::state_key::StateKey;
use crate::search::forecast::FoodForecastPolicy;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{choose_move_baseline, Decision, DecisionReason};
use crate::GameState;

use super::DecisionEngine;

const RUNTIME_HISTORY_LIMIT: usize = 8;
const MAX_RUNTIME_JITTER_RESERVE_MS: u64 = 100;

#[derive(Debug, Default)]
struct RuntimeHistory {
    recent_compute_us: VecDeque<u64>,
    recent_latency_ms: VecDeque<u64>,
}

impl RuntimeHistory {
    fn record(&mut self, compute_us: u64, latency_ms: u64) {
        push_bounded(&mut self.recent_compute_us, compute_us);
        push_bounded(&mut self.recent_latency_ms, latency_ms);
    }

    fn jitter_reserve_ms(&self) -> u64 {
        let compute_jitter_us = range(&self.recent_compute_us);
        let latency_jitter_ms = range(&self.recent_latency_ms);
        compute_jitter_us
            .saturating_add(999)
            .saturating_div(1000)
            .saturating_add(latency_jitter_ms)
            .min(MAX_RUNTIME_JITTER_RESERVE_MS)
    }
}

#[derive(Debug, Default)]
pub(crate) struct DecisionState {
    graph: Option<FutureGraph>,
    previous_observed_food: Option<Vec<crate::Coord>>,
    runtime_history: RuntimeHistory,
}

impl DecisionState {
    pub(crate) fn decide(&mut self, state: &GameState) -> Decision {
        let runtime_jitter_reserve_ms = self.runtime_history.jitter_reserve_ms();
        let forecast_policy = FoodForecastPolicy::from_game_state(state);
        let normalized = SimulatedGameState::from(state);

        if normalized.rules.simulation_support() != SimulationSupport::StandardLike {
            self.graph = None;
            self.previous_observed_food = Some(normalized_food(&normalized.food));
            return baseline_fallback(state);
        }

        let actual_key = StateKey::from_beam_state(&normalized);
        let observed_food = normalized_food(&normalized.food);
        let food_spawned = self
            .previous_observed_food
            .as_ref()
            .is_some_and(|previous| contains_new_food(previous, &observed_food));

        let mut graph = match self.graph.take() {
            Some(mut graph)
                if graph.node(graph.root()).state.turn < normalized.turn
                    && !food_spawned
                    && graph.root_children_match_food(&actual_key) =>
            {
                if let Some(node_id) = graph.find_node_by_key(&actual_key) {
                    graph.reroot(node_id);
                    graph
                } else {
                    FutureGraph::new_beam_with_forecast(normalized, forecast_policy)
                }
            }
            _ => FutureGraph::new_beam_with_forecast(normalized, forecast_policy),
        };

        let decision = DecisionEngine::stateless()
            .try_decide(state, &mut graph, runtime_jitter_reserve_ms)
            .unwrap_or_else(|| baseline_fallback(state));

        graph.retain_chosen_direction(decision.direction);
        self.graph = Some(graph);
        self.previous_observed_food = Some(observed_food);
        self.runtime_history.record(
            decision.search.elapsed_us,
            state.you.latency.parse::<u64>().unwrap_or(0),
        );

        decision
    }
}

fn baseline_fallback(state: &GameState) -> Decision {
    let mut decision = choose_move_baseline(state);
    if !matches!(
        decision.reason,
        DecisionReason::OnlyLegalMove | DecisionReason::NoSafeMove
    ) {
        decision.reason = DecisionReason::BaselineFallback;
    }
    decision
}

fn push_bounded(values: &mut VecDeque<u64>, value: u64) {
    if values.len() == RUNTIME_HISTORY_LIMIT {
        values.pop_front();
    }
    values.push_back(value);
}

fn range(values: &VecDeque<u64>) -> u64 {
    let Some(minimum) = values.iter().min().copied() else {
        return 0;
    };
    let Some(maximum) = values.iter().max().copied() else {
        return 0;
    };
    maximum.saturating_sub(minimum)
}

fn normalized_food(food: &[crate::Coord]) -> Vec<crate::Coord> {
    let mut normalized = food.to_vec();
    normalized.sort_unstable();
    normalized.dedup();
    normalized
}

fn contains_new_food(previous: &[crate::Coord], actual: &[crate::Coord]) -> bool {
    actual
        .iter()
        .any(|food| previous.binary_search(food).is_err())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Coord;

    #[test]
    fn runtime_history_has_bounded_jitter_window() {
        let mut history = RuntimeHistory::default();
        for index in 0..20_u64 {
            history.record(index * 1000, index);
        }
        assert_eq!(history.recent_compute_us.len(), RUNTIME_HISTORY_LIMIT);
        assert_eq!(history.recent_latency_ms.len(), RUNTIME_HISTORY_LIMIT);
        assert!(history.jitter_reserve_ms() > 0);
        assert!(history.jitter_reserve_ms() <= MAX_RUNTIME_JITTER_RESERVE_MS);
    }

    #[test]
    fn new_food_invalidates_cached_prediction() {
        let before = vec![Coord { x: 1, y: 1 }];
        let after = vec![Coord { x: 1, y: 1 }, Coord { x: 4, y: 4 }];
        assert!(contains_new_food(&before, &after));
        assert!(!contains_new_food(&after, &before));
    }

    #[test]
    fn food_order_and_duplicates_do_not_change_comparison() {
        let a = vec![Coord { x: 2, y: 2 }, Coord { x: 1, y: 1 }];
        let b = vec![
            Coord { x: 1, y: 1 },
            Coord { x: 2, y: 2 },
            Coord { x: 2, y: 2 },
        ];
        assert_eq!(normalized_food(&a), normalized_food(&b));
    }
}
