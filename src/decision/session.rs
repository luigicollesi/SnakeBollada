use std::collections::VecDeque;

use crate::decision::intent::{
    committable_hunt_plan, DecisionIntent, EscapeIntent, FoodIntent, HuntIntent,
};
use crate::decision::state_key::StateKey;
use crate::direction::Direction;
use crate::enemy::profile::OpponentProfiles;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{
    AggressionState, SimulatedGameState, SimulationSupport, OPENING_FOOD_TARGET_FRUITS,
};
use crate::strategy::{choose_move_baseline, Decision};
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
    aggression: AggressionState,
    previous_our_length: Option<usize>,
    previous_observed_food: Option<Vec<crate::Coord>>,
    intent: Option<DecisionIntent>,
    opponent_profiles: OpponentProfiles,
    runtime_history: RuntimeHistory,
}

impl DecisionState {
    pub(crate) fn decide(&mut self, state: &GameState) -> Decision {
        let runtime_jitter_reserve_ms = self.runtime_history.jitter_reserve_ms();
        self.observe_aggression(state);
        self.observe_opponents(state);

        let mut normalized = SimulatedGameState::from(state);
        normalized.aggression = self.aggression;

        if normalized.rules.simulation_support() != SimulationSupport::StandardLike {
            self.graph = None;
            self.intent = None;
            self.previous_our_length = Some(state.you.body.len());
            return choose_move_baseline(state);
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
                    FutureGraph::new_beam_with_opponent_profiles(
                        normalized.clone(),
                        self.opponent_profiles.clone(),
                    )
                } else if let Some(node_id) = graph.find_node_by_key(&actual_key) {
                    graph.reroot(node_id);
                    graph
                } else {
                    FutureGraph::new_beam_with_opponent_profiles(
                        normalized.clone(),
                        self.opponent_profiles.clone(),
                    )
                }
            }
            None => FutureGraph::new_beam_with_opponent_profiles(
                normalized.clone(),
                self.opponent_profiles.clone(),
            ),
        };
        graph.set_opponent_profiles(self.opponent_profiles.clone());

        self.intent = None;
        let decision = if let Some(decision) = DecisionEngine::stateless()
            .try_decide_beam_with_graph(state, &mut graph, runtime_jitter_reserve_ms)
        {
            decision
        } else {
            choose_move_baseline(state)
        };

        graph.retain_chosen_direction(decision.direction);

        self.graph = Some(graph);
        self.previous_our_length = Some(state.you.body.len());
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

            self.opponent_profiles
                .entry(enemy.id.clone())
                .or_default()
                .observe(move_set, direction);
        }
    }

    fn reconcile_intent_with_observation(&mut self, state: &GameState) {
        let release = match self.intent.as_ref() {
            Some(DecisionIntent::Food(intent)) => !state.board.food.contains(&intent.target),
            Some(DecisionIntent::Hunt(intent)) => !state
                .board
                .snakes
                .iter()
                .any(|snake| snake.id == intent.target),
            Some(DecisionIntent::Escape(_)) | None => false,
        };

        if release {
            self.intent = None;
        }
    }

    fn refresh_food_intent(&mut self, graph: &FutureGraph) {
        let Some(DecisionIntent::Food(intent)) = self.intent.as_ref() else {
            return;
        };

        let root = graph.node(graph.root());
        let viable = root
            .active_analysis()
            .and_then(|analysis| {
                analysis
                    .state_analysis()
                    .route_for(&root.state.our_snake_id, intent.target)
            })
            .is_some_and(|route| route.reachable && route.distance.is_some());

        let dominant_hunt_available = root.active_analysis().is_some_and(|analysis| {
            analysis.posture().favors_dominant_hunt()
                && analysis
                    .hunting()
                    .plans
                    .iter()
                    .any(|plan| committable_hunt_plan(plan) && plan.score_milli >= 400)
        });

        if !viable || dominant_hunt_available {
            self.intent = None;
        }
    }

    fn refresh_hunt_intent(&mut self, graph: &FutureGraph) {
        let Some(DecisionIntent::Hunt(intent)) = self.intent.as_mut() else {
            return;
        };

        let root = graph.node(graph.root());
        let score = root
            .active_analysis()
            .and_then(|analysis| {
                analysis
                    .hunting()
                    .plans
                    .iter()
                    .find(|plan| plan.target == intent.target && plan.kind == intent.kind)
            })
            .map(|plan| plan.score_milli);
        intent.record_plan(score);

        if intent.should_release(root.state.turn) {
            self.intent = None;
        }
    }

    fn update_intent_after_decision(
        &mut self,
        state: &GameState,
        graph: &FutureGraph,
        decision: &Decision,
    ) {
        if let Some(DecisionIntent::Escape(intent)) = self.intent.as_mut() {
            intent.record_pressure(decision.search.escape_pressure_milli);
            if intent.should_release() {
                self.intent = None;
            }
            return;
        }

        if decision.search.escape_selected {
            self.intent = Some(DecisionIntent::Escape(EscapeIntent::new(
                state.turn,
                decision.search.escape_pressure_milli,
            )));
            return;
        }
        if let Some(DecisionIntent::Food(current)) = self.intent.as_ref() {
            if state.board.food.contains(&current.target)
                && decision.reason != crate::strategy::DecisionReason::HuntingTactical
            {
                return;
            }
            if decision.reason == crate::strategy::DecisionReason::HuntingTactical {
                self.intent = None;
            }
        }

        if matches!(self.intent, Some(DecisionIntent::Hunt(_))) {
            return;
        }

        if let Some(target) = decision.target_food {
            self.intent = Some(DecisionIntent::Food(FoodIntent::new(target, state.turn)));
            return;
        }

        if decision.reason != crate::strategy::DecisionReason::HuntingTactical {
            return;
        }

        let (Some(target), Some(kind)) = (decision.target_enemy.as_deref(), decision.hunt_kind)
        else {
            return;
        };

        let root = graph.node(graph.root());
        let Some(plan) = root.active_analysis().and_then(|analysis| {
            analysis.hunting().plans.iter().find(|plan| {
                plan.target == target && plan.kind == kind && committable_hunt_plan(plan)
            })
        }) else {
            return;
        };

        self.intent = Some(DecisionIntent::Hunt(HuntIntent::new(plan, state.turn)));
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
    fn observed_enemy_head_pressure_updates_persistent_profile() {
        let ours = snake(vec![Coord { x: 2, y: 1 }, Coord { x: 1, y: 1 }]);
        let enemy = Battlesnake {
            id: "enemy".to_string(),
            name: "enemy".to_string(),
            health: 100,
            head: Coord { x: 2, y: 3 },
            length: 3,
            body: vec![
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
                Coord { x: 1, y: 4 },
            ],
            latency: String::new(),
            shout: None,
        };
        let previous = GameState {
            game: Game {
                id: "opponent-profile".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 1,
            board: Board {
                width: 7,
                height: 7,
                food: vec![Coord { x: 4, y: 3 }],
                snakes: vec![ours.clone(), enemy.clone()],
                hazards: vec![],
            },
            you: ours.clone(),
        };
        let graph = FutureGraph::new(SimulatedGameState::from(&previous));
        let root = graph.node(graph.root());
        let enemy_actor = root
            .state
            .actor_index("enemy")
            .expect("enemy actor must exist");
        let attack = root
            .active_analysis()
            .unwrap()
            .tracing
            .for_actor(enemy_actor)
            .unwrap()
            .hypothesis(Direction::Down)
            .expect("head-pressure move must be predicted");
        assert!(attack.support.head_threat);

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
        assert!(profile.head_threat_bias_milli > 1000);
        assert!(profile.food_bias_milli < 1000);
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
    fn food_opening_starts_non_aggressive_and_locks_until_four_fruits() {
        let mut decision = DecisionState::default();

        assert_eq!(decision.aggression.value, 0.0);
        assert_eq!(decision.aggression.fruits_eaten, 0);
        assert!(decision.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS);

        decision.aggression.record_food();
        assert!((decision.aggression.value - 0.05).abs() < f32::EPSILON);
        assert!(decision.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS);

        while decision.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS {
            decision.aggression.record_food();
        }
        assert_eq!(decision.aggression.fruits_eaten, OPENING_FOOD_TARGET_FRUITS);
        assert!((decision.aggression.value - 0.20).abs() < f32::EPSILON);
        assert!(!(decision.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS));
    }

    #[test]
    fn food_intent_is_released_when_target_route_becomes_unreachable() {
        use crate::decision::intent::{DecisionIntent, FoodIntent};

        let target = Coord { x: 6, y: 6 };
        let mut game = state(1, vec![Coord { x: 0, y: 0 }, Coord { x: 0, y: 1 }]);
        game.board.food = vec![target];
        let mut normalized = SimulatedGameState::from(&game);

        normalized
            .snakes
            .push(crate::simulation::state::SimulatedSnake {
                id: "wall".to_string(),
                health: 100,
                body: (0..7).map(|y| Coord { x: 1, y }).collect(),
                alive: true,
            });
        let graph = FutureGraph::new(normalized);

        let mut decision = DecisionState {
            intent: Some(DecisionIntent::Food(FoodIntent::new(target, 1))),
            ..DecisionState::default()
        };
        decision.refresh_food_intent(&graph);

        assert!(decision.intent.is_none());
    }

    #[test]
    fn dominant_hunt_releases_reachable_food_intent() {
        use crate::decision::intent::{DecisionIntent, FoodIntent};

        let ours = snake(vec![
            Coord { x: 2, y: 2 },
            Coord { x: 2, y: 1 },
            Coord { x: 2, y: 0 },
            Coord { x: 1, y: 0 },
            Coord { x: 0, y: 0 },
            Coord { x: 0, y: 1 },
            Coord { x: 0, y: 2 },
            Coord { x: 1, y: 2 },
        ]);
        let enemy = Battlesnake {
            id: "enemy".to_string(),
            name: "enemy".to_string(),
            health: 100,
            head: Coord { x: 6, y: 3 },
            length: 3,
            body: vec![
                Coord { x: 6, y: 3 },
                Coord { x: 6, y: 2 },
                Coord { x: 6, y: 1 },
            ],
            latency: String::new(),
            shout: None,
        };
        let target = Coord { x: 4, y: 4 };
        let game = GameState {
            game: Game {
                id: "dominant-food-release".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 10,
            board: Board {
                width: 7,
                height: 7,
                food: vec![target],
                snakes: vec![ours.clone(), enemy],
                hazards: vec![],
            },
            you: ours,
        };
        let graph = FutureGraph::new(SimulatedGameState::from(&game));
        let root = graph.node(graph.root());
        let analysis = root.active_analysis().unwrap();

        assert!(analysis
            .state_analysis()
            .route_for(&root.state.our_snake_id, target)
            .is_some_and(|route| route.reachable));
        assert!(analysis.posture().favors_dominant_hunt());
        assert!(analysis
            .hunting()
            .plans
            .iter()
            .any(|plan| committable_hunt_plan(plan) && plan.score_milli >= 400));

        let mut decision = DecisionState {
            intent: Some(DecisionIntent::Food(FoodIntent::new(target, 8))),
            ..DecisionState::default()
        };
        decision.refresh_food_intent(&graph);

        assert!(decision.intent.is_none());
    }

    #[test]
    fn generic_hunting_reason_does_not_persist_arbitrary_plan() {
        let game = state(3, vec![Coord { x: 3, y: 3 }, Coord { x: 3, y: 2 }]);
        let graph = FutureGraph::new(SimulatedGameState::from(&game));
        let decision = Decision {
            direction: Direction::Up,
            reason: crate::strategy::DecisionReason::HuntingTactical,
            target_food: None,
            target_enemy: None,
            hunt_kind: None,
            path_distance: None,
            reachable_cells: 10,
            search: crate::strategy::SearchMetadata::default(),
        };
        let mut state = DecisionState::default();

        state.update_intent_after_decision(&game, &graph, &decision);

        assert!(state.intent.is_none());
    }

    #[test]
    fn hunting_decision_persists_exact_selected_plan() {
        let ours = snake(vec![
            Coord { x: 2, y: 2 },
            Coord { x: 2, y: 1 },
            Coord { x: 2, y: 0 },
            Coord { x: 1, y: 0 },
            Coord { x: 0, y: 0 },
            Coord { x: 0, y: 1 },
            Coord { x: 0, y: 2 },
            Coord { x: 1, y: 2 },
        ]);
        let enemy = Battlesnake {
            id: "enemy".to_string(),
            name: "enemy".to_string(),
            health: 100,
            head: Coord { x: 6, y: 3 },
            length: 3,
            body: vec![
                Coord { x: 6, y: 3 },
                Coord { x: 6, y: 2 },
                Coord { x: 6, y: 1 },
            ],
            latency: String::new(),
            shout: None,
        };
        let game = GameState {
            game: Game {
                id: "exact-hunt-plan".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 10,
            board: Board {
                width: 7,
                height: 7,
                food: vec![],
                snakes: vec![ours.clone(), enemy],
                hazards: vec![],
            },
            you: ours,
        };
        let graph = FutureGraph::new(SimulatedGameState::from(&game));
        let plan = graph
            .node(graph.root())
            .active_analysis()
            .unwrap()
            .hunting()
            .plans
            .iter()
            .find(|plan| committable_hunt_plan(plan))
            .expect("dominant state must expose a committable hunt plan")
            .clone();
        let decision = Decision {
            direction: Direction::Right,
            reason: crate::strategy::DecisionReason::HuntingTactical,
            target_food: None,
            target_enemy: Some(plan.target.clone()),
            hunt_kind: Some(plan.kind),
            path_distance: None,
            reachable_cells: 10,
            search: crate::strategy::SearchMetadata::default(),
        };
        let mut state = DecisionState::default();

        state.update_intent_after_decision(&game, &graph, &decision);

        let Some(DecisionIntent::Hunt(intent)) = state.intent else {
            panic!("selected hunt plan must become the persistent intent");
        };
        assert_eq!(intent.target, plan.target);
        assert_eq!(intent.kind, plan.kind);
    }

    #[test]
    fn escape_decision_overrides_existing_food_intent() {
        use crate::decision::intent::{DecisionIntent, FoodIntent};

        let game = state(3, vec![Coord { x: 3, y: 3 }, Coord { x: 3, y: 2 }]);
        let graph = FutureGraph::new(SimulatedGameState::from(&game));
        let mut decision_state = DecisionState {
            intent: Some(DecisionIntent::Food(FoodIntent::new(
                Coord { x: 4, y: 4 },
                2,
            ))),
            ..DecisionState::default()
        };
        let decision = Decision {
            direction: Direction::Up,
            reason: crate::strategy::DecisionReason::SurvivalCritical,
            target_food: None,
            target_enemy: None,
            hunt_kind: None,
            path_distance: None,
            reachable_cells: 10,
            search: crate::strategy::SearchMetadata {
                escape_pressure_milli: 750,
                escape_selected: true,
                ..crate::strategy::SearchMetadata::default()
            },
        };

        decision_state.update_intent_after_decision(&game, &graph, &decision);

        assert!(matches!(
            decision_state.intent,
            Some(DecisionIntent::Escape(_))
        ));
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
