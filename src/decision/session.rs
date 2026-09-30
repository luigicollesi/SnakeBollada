use crate::decision::state_key::StateKey;
use crate::direction::Direction;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{AggressionState, SimulatedGameState, SimulationSupport};
use crate::strategy::{choose_move_baseline, CacheInvalidationReason, Decision};
use crate::GameState;

use super::DecisionEngine;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EnemyTracingCoverage {
    observed: u16,
    legal_covered: u16,
    plausible_covered: u16,
}

#[derive(Debug, Default)]
pub(crate) struct DecisionState {
    graph: Option<FutureGraph>,
    aggression: AggressionState,
    previous_our_length: Option<usize>,
}

impl DecisionState {
    pub(crate) fn decide(&mut self, state: &GameState) -> Decision {
        let tracing_coverage = self.enemy_tracing_coverage(state);
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
        decision.search.enemy_moves_observed = tracing_coverage.observed;
        decision.search.enemy_moves_legal_covered = tracing_coverage.legal_covered;
        decision.search.enemy_moves_plausible_covered = tracing_coverage.plausible_covered;
        graph.retain_chosen_direction(decision.direction);

        self.graph = Some(graph);
        self.previous_our_length = Some(state.you.body.len());

        decision
    }

    fn enemy_tracing_coverage(&self, state: &GameState) -> EnemyTracingCoverage {
        let Some(graph) = self.graph.as_ref() else {
            return EnemyTracingCoverage::default();
        };
        let root = graph.node(graph.root());

        if root.state.turn.saturating_add(1) != state.turn {
            return EnemyTracingCoverage::default();
        }

        let mut coverage = EnemyTracingCoverage::default();

        for enemy in root
            .state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != root.state.our_snake_id)
        {
            let Some(previous_head) = enemy.head() else {
                continue;
            };
            let Some(current) = state.board.snakes.iter().find(|snake| snake.id == enemy.id) else {
                continue;
            };
            let Some(direction) = Direction::from_heads(previous_head, current.head) else {
                continue;
            };
            let Some(prediction) = root.tracing.for_enemy(&enemy.id) else {
                continue;
            };

            coverage.observed = coverage.observed.saturating_add(1);
            if prediction.legal_moves.contains(direction) {
                coverage.legal_covered = coverage.legal_covered.saturating_add(1);
            }
            if prediction.plausible_moves.contains(direction) {
                coverage.plausible_covered = coverage.plausible_covered.saturating_add(1);
            }
        }

        coverage
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
    fn enemy_tracing_coverage_compares_next_observed_move() {
        let mut first = state(1, vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        first.board.snakes.push(Battlesnake {
            id: "enemy".to_string(),
            name: "enemy".to_string(),
            health: 100,
            head: Coord { x: 5, y: 5 },
            length: 2,
            body: vec![Coord { x: 5, y: 5 }, Coord { x: 5, y: 4 }],
            latency: String::new(),
            shout: None,
        });

        let mut next = first.clone();
        next.turn = 2;
        let current_enemy = next
            .board
            .snakes
            .iter_mut()
            .find(|snake| snake.id == "enemy")
            .unwrap();
        current_enemy.head = Coord { x: 6, y: 5 };
        current_enemy.body = vec![Coord { x: 6, y: 5 }, Coord { x: 5, y: 5 }];

        let decision = DecisionState {
            graph: Some(FutureGraph::new(SimulatedGameState::from(&first))),
            ..DecisionState::default()
        };

        let coverage = decision.enemy_tracing_coverage(&next);

        assert_eq!(coverage.observed, 1);
        assert_eq!(coverage.legal_covered, 1);
        assert_eq!(coverage.plausible_covered, 1);
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
