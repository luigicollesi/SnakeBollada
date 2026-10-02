use std::collections::VecDeque;

use crate::decision::state_key::StateKey;
use crate::direction::Direction;
use crate::enemy::intent::infer_observed_intent;
use crate::enemy::profile::OpponentProfiles;
use crate::search::forecast::FoodForecastPolicy;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{choose_move_baseline, Decision, DecisionReason};
use crate::GameState;

use super::continuity::DecisionContinuity;
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
    opponent_profiles: OpponentProfiles,
    runtime_history: RuntimeHistory,
    continuity: DecisionContinuity,
}

impl DecisionState {
    pub(crate) fn decide(&mut self, state: &GameState) -> Decision {
        let runtime_jitter_reserve_ms = self.runtime_history.jitter_reserve_ms();
        self.observe_opponents(state);

        let forecast_policy = FoodForecastPolicy::from_game_state(state);
        let normalized = SimulatedGameState::from(state);
        if normalized.rules.simulation_support() != SimulationSupport::StandardLike {
            self.graph = None;
            self.continuity.clear();
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
            Some(mut graph) => {
                if graph.node(graph.root()).state.turn >= normalized.turn
                    || food_spawned
                    || !graph.root_children_match_food(&actual_key)
                {
                    FutureGraph::new_beam_with_opponent_profiles_and_forecast(
                        normalized.clone(),
                        self.opponent_profiles.clone(),
                        forecast_policy,
                    )
                } else if let Some(node_id) = graph.find_node_by_key(&actual_key) {
                    graph.reroot(node_id);
                    graph
                } else {
                    FutureGraph::new_beam_with_opponent_profiles_and_forecast(
                        normalized.clone(),
                        self.opponent_profiles.clone(),
                        forecast_policy,
                    )
                }
            }
            None => FutureGraph::new_beam_with_opponent_profiles_and_forecast(
                normalized,
                self.opponent_profiles.clone(),
                forecast_policy,
            ),
        };
        graph.set_opponent_profiles(self.opponent_profiles.clone());

        let incumbent_direction = self.continuity.incumbent_for(&actual_key);
        let outcome = DecisionEngine::stateless().try_decide_beam_with_continuity(
            state,
            &mut graph,
            runtime_jitter_reserve_ms,
            incumbent_direction,
        );
        let decision = match outcome {
            Some(outcome) => {
                self.continuity
                    .replace_from_line(&graph, &outcome.selected_line);
                outcome.decision
            }
            None => {
                self.continuity.clear();
                baseline_fallback(state)
            }
        };

        graph.retain_chosen_direction(decision.direction);
        self.graph = Some(graph);
        self.previous_observed_food = Some(observed_food);
        self.runtime_history.record(
            decision.search.elapsed_us,
            state.you.latency.parse::<u64>().unwrap_or(0),
        );

        decision
    }

    fn observe_opponents(&mut self, state: &GameState) {
        let Some(graph) = self.graph.as_ref() else {
            return;
        };
        let previous = graph.node(graph.root());
        if previous.state.turn >= state.turn {
            return;
        }
        let Some(analysis) = previous.active_analysis() else {
            return;
        };

        for enemy in state
            .board
            .snakes
            .iter()
            .filter(|snake| snake.id != state.you.id)
        {
            let Some(previous_enemy) = previous.state.snake(&enemy.id) else {
                continue;
            };
            let Some(previous_head) = previous_enemy.head() else {
                continue;
            };
            let Some(direction) = direction_between(previous_head, enemy.head) else {
                continue;
            };
            let Some(actor) = previous.state.actor_index(&enemy.id) else {
                continue;
            };
            let Some(move_set) = analysis.tracing.for_actor(actor) else {
                continue;
            };

            let inference = infer_observed_intent(graph, actor, direction);
            let profile = self.opponent_profiles.entry(enemy.id.clone()).or_default();
            profile.record_intent_inference(direction, inference);
            profile.observe_with_intent(move_set, direction, inference.contrast());
            log::debug!(
                target: "opponent_intent",
                "enemy={} observed={:?} food_bias={} hunting_bias={} trapping_bias={} head_threat_bias={} learned={} forced={} low_contrast={} incomplete_root={} survival_downweighted={} last_food={} last_hunting={} last_trapping={} information={}",
                enemy.id,
                direction,
                profile.food_bias_milli,
                profile.hunting_bias_milli,
                profile.trapping_bias_milli,
                profile.head_threat_bias_milli,
                profile.intent_stats.learned_observations,
                profile.intent_stats.skipped_forced,
                profile.intent_stats.skipped_low_contrast,
                profile.intent_stats.skipped_incomplete_root,
                profile.intent_stats.downweighted_survival_emergency,
                profile.intent_stats.last_food_contrast,
                profile.intent_stats.last_hunting_contrast,
                profile.intent_stats.last_trapping_contrast,
                profile.intent_stats.last_information_milli,
            );
        }
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

fn direction_between(from: crate::Coord, to: crate::Coord) -> Option<Direction> {
    Direction::ALL
        .into_iter()
        .find(|direction| direction.apply(from) == to)
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
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};

    fn snake(id: &str, body: Vec<Coord>) -> Battlesnake {
        Battlesnake {
            id: id.to_string(),
            name: id.to_string(),
            health: 100,
            head: body[0],
            length: body.len() as u32,
            body,
            latency: String::new(),
            shout: None,
        }
    }

    fn state_with_enemy(turn: i32) -> GameState {
        let ours = snake("ours", vec![Coord { x: 2, y: 1 }, Coord { x: 1, y: 1 }]);
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
                Coord { x: 1, y: 4 },
            ],
        );

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
                food: vec![Coord { x: 4, y: 3 }],
                snakes: vec![ours.clone(), enemy],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn direction_between_recovers_observed_enemy_move() {
        let from = Coord { x: 3, y: 3 };

        assert_eq!(
            direction_between(from, Coord { x: 3, y: 4 }),
            Some(Direction::Up)
        );
        assert_eq!(
            direction_between(from, Coord { x: 2, y: 3 }),
            Some(Direction::Left)
        );
        assert_eq!(direction_between(from, Coord { x: 5, y: 3 }), None);
    }

    #[test]
    fn observed_enemy_move_updates_persistent_profile_from_beam_graph() {
        let previous = state_with_enemy(1);
        let mut graph = FutureGraph::new_beam(SimulatedGameState::from(&previous));
        let root = graph.root();
        graph.expand_to_depth(1).unwrap();
        let root_node = graph.node(root);
        let enemy_actor = root_node.state.actor_index("enemy").unwrap();
        assert!(root_node
            .active_analysis()
            .and_then(|analysis| analysis.tracing.for_actor(enemy_actor))
            .is_some());

        let mut current = previous.clone();
        current.turn = 2;
        current.board.snakes[1].head = Coord { x: 2, y: 2 };
        current.board.snakes[1].body = vec![
            Coord { x: 2, y: 2 },
            Coord { x: 2, y: 3 },
            Coord { x: 2, y: 4 },
        ];

        let mut decision = DecisionState {
            graph: Some(graph),
            ..DecisionState::default()
        };
        decision.observe_opponents(&current);

        let profile = decision.opponent_profiles.get("enemy").unwrap();
        assert_eq!(profile.observations, 1);
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
}
