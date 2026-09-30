use crate::decision::state_key::StateKey;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{AggressionState, SimulatedGameState, SimulationSupport};
use crate::strategy::{choose_move_baseline, CacheInvalidationReason, Decision};
use crate::GameState;

use super::DecisionEngine;

#[derive(Debug, Default)]
pub(crate) struct DecisionState {
    graph: Option<FutureGraph>,
    aggression: AggressionState,
    previous_our_length: Option<usize>,
}

impl DecisionState {
    pub(crate) fn decide(&mut self, state: &GameState) -> Decision {
        self.observe_aggression(state);

        let mut normalized = SimulatedGameState::from(state);
        normalized.aggression = self.aggression;

        if normalized.rules.simulation_support() != SimulationSupport::StandardLike {
            self.graph = None;
            self.previous_our_length = Some(state.you.body.len());
            return choose_move_baseline(state);
        }

        let actual_key = StateKey::from_state(&normalized);
        let mut cache_reused = false;
        let mut cache_invalidation = CacheInvalidationReason::None;

        let mut graph = match self.graph.take() {
            Some(mut graph) => {
                if graph.node(graph.root()).state.turn >= normalized.turn {
                    cache_invalidation = CacheInvalidationReason::TurnMismatch;
                    FutureGraph::new(normalized)
                } else if !graph.root_children_match_food(&normalized.food) {
                    cache_invalidation = CacheInvalidationReason::FoodMismatch;
                    FutureGraph::new(normalized)
                } else if let Some(node_id) = graph.find_node_by_key(&actual_key) {
                    graph.reroot(node_id);
                    cache_reused = true;
                    graph
                } else {
                    cache_invalidation = CacheInvalidationReason::StateMismatch;
                    FutureGraph::new(normalized)
                }
            }
            None => FutureGraph::new(normalized),
        };

        let mut decision = DecisionEngine::stateless().decide_with_graph(state, &mut graph);
        decision.search.cache_reused = cache_reused;
        decision.search.cache_invalidation = cache_invalidation;
        graph.retain_chosen_direction(decision.direction);

        self.graph = Some(graph);
        self.previous_our_length = Some(state.you.body.len());

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
    fn no_growth_keeps_aggression_stable() {
        let mut decision = DecisionState::default();
        decision.previous_our_length = Some(2);
        let next = state(2, vec![Coord { x: 3, y: 2 }, Coord { x: 2, y: 2 }]);
        let before = decision.aggression;

        decision.observe_aggression(&next);

        assert_eq!(decision.aggression, before);
    }
}
