use std::collections::VecDeque;

use crate::decision::state_key::StateKey;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{
    AggressionState, SimulatedGameState, SimulationSupport, OPENING_FOOD_TARGET_FRUITS,
};
use crate::strategy::{choose_move_baseline, Decision};
use crate::{Coord, GameState};

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
    aggression: AggressionState,
    previous_our_length: Option<usize>,
    previous_observed_food: Option<Vec<crate::Coord>>,
    committed_food: Option<Coord>,
    runtime_history: RuntimeHistory,
}

impl DecisionState {
    pub(crate) fn decide(&mut self, state: &GameState) -> Decision {
        let runtime_jitter_reserve_ms = self.runtime_history.jitter_reserve_ms();
        self.observe_aggression(state);

        if self
            .committed_food
            .is_some_and(|target| !state.board.food.contains(&target))
        {
            self.committed_food = None;
        }

        let prioritize_food = self.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS;

        let mut normalized = SimulatedGameState::from(state);
        normalized.aggression = self.aggression;

        if normalized.rules.simulation_support() != SimulationSupport::StandardLike {
            self.graph = None;
            self.committed_food = None;
            self.previous_our_length = Some(state.you.body.len());
            return choose_move_baseline(state);
        }

        let actual_key = StateKey::from_state(&normalized);
        let food_spawned = self
            .previous_observed_food
            .as_ref()
            .is_some_and(|previous| contains_new_food(previous, &normalized.food));

        let mut graph = match self.graph.take() {
            Some(mut graph) => {
                if graph.node(graph.root()).state.turn >= normalized.turn
                    || food_spawned
                    || !graph.root_children_match_food(&normalized.food)
                {
                    FutureGraph::new(normalized)
                } else if let Some(node_id) = graph.find_node_by_key(&actual_key) {
                    graph.reroot(node_id);
                    graph
                } else {
                    FutureGraph::new(normalized)
                }
            }
            None => FutureGraph::new(normalized),
        };

        let decision = DecisionEngine::stateless()
            .decide_with_graph_with_reserve_and_food_preference(
                state,
                &mut graph,
                runtime_jitter_reserve_ms,
                self.committed_food,
                prioritize_food,
            );

        if prioritize_food {
            if let Some(target) = decision.target_food {
                self.committed_food = Some(target);
            }
        } else {
            self.committed_food = None;
        }

        graph.retain_chosen_direction(decision.direction);

        self.graph = Some(graph);
        self.previous_our_length = Some(state.you.body.len());
        self.previous_observed_food = Some(normalized_food(&state.board.food));
        self.runtime_history.record(
            decision.search.elapsed_us,
            state.you.latency.parse::<u64>().unwrap_or(0),
        );

        decision
    }

    fn observe_aggression(&mut self, state: &GameState) {
        let current_length = state.you.body.len();
        let Some(previous_length) = self.previous_our_length else {
            return;
        };

        let foods_eaten = current_length.saturating_sub(previous_length);
        for _ in 0..foods_eaten {
            self.aggression.record_food();
        }
    }
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
    let previous = normalized_food(previous);
    normalized_food(actual)
        .into_iter()
        .any(|food| previous.binary_search(&food).is_err())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};

    fn snake(body: Vec<Coord>) -> Battlesnake {
        Battlesnake {
            id: "ours".to_string(),
            name: "ours".to_string(),
            health: 100,
            head: body[0],
            length: body.len() as u32,
            body,
            latency: String::new(),
            shout: None,
        }
    }

    fn state(turn: i32, body: Vec<Coord>) -> GameState {
        let ours = snake(body);
        GameState {
            game: Game {
                id: "session".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn,
            board: Board {
                width: 7,
                height: 7,
                food: vec![],
                snakes: vec![ours.clone()],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn observed_growth_updates_persistent_aggression() {
        let mut decision = DecisionState::default();
        let first = state(1, vec![Coord { x: 2, y: 2 }, Coord { x: 2, y: 1 }]);
        decision.previous_our_length = Some(first.you.body.len());

        let grown = state(
            2,
            vec![
                Coord { x: 3, y: 2 },
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
            ],
        );
        decision.observe_aggression(&grown);

        assert_eq!(decision.aggression.fruits_eaten, 1);
    }

    #[test]
    fn food_opening_starts_non_aggressive_and_locks_until_two_fruits() {
        let mut decision = DecisionState::default();

        assert_eq!(decision.aggression.value, 0.0);
        assert_eq!(decision.aggression.fruits_eaten, 0);
        assert!(decision.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS);

        decision.aggression.record_food();
        assert!((decision.aggression.value - 0.10).abs() < f32::EPSILON);
        assert!(decision.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS);

        decision.aggression.record_food();
        assert_eq!(decision.aggression.fruits_eaten, OPENING_FOOD_TARGET_FRUITS);
        assert!(!(decision.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS));
    }

    #[test]
    fn runtime_history_uses_bounded_jitter_window() {
        let mut history = RuntimeHistory::default();
        for index in 0..20_u64 {
            history.record(index * 1_000, index);
        }

        assert_eq!(history.recent_compute_us.len(), RUNTIME_HISTORY_LIMIT);
        assert_eq!(history.recent_latency_ms.len(), RUNTIME_HISTORY_LIMIT);
        assert!(history.jitter_reserve_ms() > 0);
        assert!(history.jitter_reserve_ms() <= MAX_RUNTIME_JITTER_RESERVE_MS);
    }

    #[test]
    fn detects_new_food_before_cache_reconciliation() {
        let previous = vec![Coord { x: 1, y: 1 }];
        let actual = vec![Coord { x: 1, y: 1 }, Coord { x: 4, y: 4 }];

        assert!(contains_new_food(&previous, &actual));
        assert!(!contains_new_food(&actual, &previous));
    }

    #[test]
    fn food_normalization_ignores_order_and_duplicates() {
        let left = vec![Coord { x: 2, y: 2 }, Coord { x: 1, y: 1 }];
        let right = vec![
            Coord { x: 1, y: 1 },
            Coord { x: 2, y: 2 },
            Coord { x: 2, y: 2 },
        ];

        assert_eq!(normalized_food(&left), normalized_food(&right));
    }

    #[test]
    fn no_growth_keeps_aggression_stable() {
        let mut decision = DecisionState {
            previous_our_length: Some(2),
            ..DecisionState::default()
        };
        let next = state(2, vec![Coord { x: 3, y: 2 }, Coord { x: 2, y: 2 }]);
        let before = decision.aggression;

        decision.observe_aggression(&next);

        assert_eq!(decision.aggression, before);
    }
}
