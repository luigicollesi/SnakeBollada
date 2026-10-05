use serde::{Deserialize, Serialize};

use crate::direction::Direction;
use crate::navigation::{reachable_after_move, NavigationMap};
use crate::GameState;

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DecisionReason {
    SurvivalCritical,
    FutureMobility,
    FoodStrategic,
    HuntingTactical,
    BeamUtility,
    ReservedEscape,
    DeterministicTieBreak,
    BaselineFallback,
    OnlyLegalMove,
    NoSafeMove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct BeamShadowMetadata {
    pub(crate) enabled: bool,
    pub(crate) completed: bool,
    pub(crate) direction: Option<Direction>,
    pub(crate) agreed_with_legacy: bool,
    pub(crate) completed_depth: u8,
    pub(crate) selected_depth: u8,
    pub(crate) attempted_depth: u8,
    pub(crate) line_count: u8,
    pub(crate) best_value: i64,
    pub(crate) elapsed_us: u64,
    pub(crate) action_batches: u32,
    pub(crate) parallel_action_batches: u32,
    pub(crate) resolved_actions: u32,
    pub(crate) new_nodes_built: u32,
    pub(crate) resolve_us: u64,
    pub(crate) node_build_us: u64,
    pub(crate) merge_us: u64,
    pub(crate) edge_score_us: u64,
    pub(crate) our_food_utility: i64,
    pub(crate) our_hunting_utility: i64,
    pub(crate) our_survival_utility: i64,
    pub(crate) our_terminal_utility: i64,
    pub(crate) opponent_food_utility: i64,
    pub(crate) opponent_hunting_utility: i64,
    pub(crate) opponent_survival_utility: i64,
    pub(crate) opponent_terminal_utility: i64,
    pub(crate) forecast_provisional: bool,
    pub(crate) terminal_confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SearchMetadata {
    pub(crate) completed_depth: u8,
    pub(crate) analyzed_depth: u8,
    pub(crate) nodes: u32,
    pub(crate) edges: u32,
    pub(crate) transposition_hits: u32,
    pub(crate) elapsed_us: u64,
    pub(crate) safety_reserve_us: u64,
    pub(crate) runtime_jitter_reserve_us: u64,
    pub(crate) beam_shadow: BeamShadowMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) direction: Direction,
    pub(crate) reason: DecisionReason,
    pub(crate) reachable_cells: u32,
    pub(crate) search: SearchMetadata,
}

pub(crate) fn choose_move(state: &GameState) -> Decision {
    crate::decision::DecisionEngine::stateless().decide(state)
}

pub(crate) fn direction_stays_in_bounds(state: &GameState, direction: Direction) -> bool {
    let target = direction.apply(state.you.head);
    target.x >= 0
        && target.y >= 0
        && target.x < state.board.width as i32
        && target.y < state.board.height as i32
}

pub(crate) fn choose_move_baseline(state: &GameState) -> Decision {
    let map = NavigationMap::from_state(state);
    let legal_moves = Direction::ALL
        .into_iter()
        .filter(|direction| !map.is_blocked(direction.apply(state.you.head)))
        .collect::<Vec<_>>();

    if legal_moves.is_empty() {
        let direction = Direction::ALL
            .into_iter()
            .filter(|direction| direction_stays_in_bounds(state, *direction))
            .min_by_key(|direction| {
                let destination = direction.apply(state.you.head);
                let food_distance = state
                    .board
                    .food
                    .iter()
                    .map(|food| destination.x.abs_diff(food.x) + destination.y.abs_diff(food.y))
                    .min()
                    .unwrap_or(u32::MAX);
                (food_distance, direction.rank())
            })
            .unwrap_or(Direction::Up);

        return Decision {
            direction,
            reason: DecisionReason::NoSafeMove,
            reachable_cells: 0,
            search: SearchMetadata::default(),
        };
    }

    if legal_moves.len() == 1 {
        let direction = legal_moves[0];
        return Decision {
            direction,
            reason: DecisionReason::OnlyLegalMove,
            reachable_cells: reachable_after_move(&map, state, direction),
            search: SearchMetadata::default(),
        };
    }

    let best = legal_moves
        .iter()
        .copied()
        .map(|direction| {
            let destination = direction.apply(state.you.head);
            let reachable = reachable_after_move(&map, state, direction);
            let hazard = map.is_hazard(destination);
            let cramped = reachable < state.you.length;
            let food_distance = state
                .board
                .food
                .iter()
                .map(|food| destination.x.abs_diff(food.x) + destination.y.abs_diff(food.y))
                .min()
                .unwrap_or(u32::MAX);

            (direction, reachable, hazard, cramped, food_distance)
        })
        .min_by_key(|(direction, reachable, hazard, cramped, food_distance)| {
            (
                *cramped,
                *hazard,
                *food_distance,
                std::cmp::Reverse(*reachable),
                direction.rank(),
            )
        })
        .expect("legal moves are not empty");

    Decision {
        direction: best.0,
        reason: DecisionReason::BaselineFallback,
        reachable_cells: best.1,
        search: SearchMetadata::default(),
    }
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

    fn state(ours: Battlesnake, enemies: Vec<Battlesnake>, food: Vec<Coord>) -> GameState {
        state_on_board(7, 7, ours, enemies, food)
    }

    fn state_on_board(
        width: u32,
        height: u32,
        ours: Battlesnake,
        enemies: Vec<Battlesnake>,
        food: Vec<Coord>,
    ) -> GameState {
        let mut snakes = vec![ours.clone()];
        snakes.extend(enemies);

        GameState {
            game: Game {
                id: "test".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 1,
            board: Board {
                height,
                width,
                food,
                snakes,
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn chooses_shortest_safe_food_path() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 0 },
            ],
        );
        let state = state(ours, vec![], vec![Coord { x: 4, y: 2 }]);

        let decision = choose_move(&state);
        assert_eq!(decision.direction, Direction::Right);
        assert_eq!(decision.reason, DecisionReason::BaselineFallback);
    }

    #[test]
    fn beam_uses_actor_relative_utility_when_food_is_contested() {
        let ours = snake("ours", vec![Coord { x: 2, y: 2 }]);
        let enemy = snake("enemy", vec![Coord { x: 5, y: 5 }]);
        let losing_food = Coord { x: 5, y: 4 };
        let claimable_food = Coord { x: 2, y: 4 };
        let state = state(ours, vec![enemy], vec![losing_food, claimable_food]);

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert!(Direction::ALL.contains(&decision.direction));
        assert!(decision.search.completed_depth >= 1);
    }

    #[test]
    fn hobbs_regression_undersized_snake_takes_adjacent_claimable_food() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 1, y: 2 },
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 3 },
            ],
        );
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 8, y: 7 },
                Coord { x: 8, y: 8 },
                Coord { x: 8, y: 9 },
                Coord { x: 8, y: 10 },
            ],
        );
        let state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![Coord { x: 0, y: 2 }, Coord { x: 5, y: 5 }],
        );

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert_eq!(decision.direction, Direction::Left);
    }

    #[test]
    fn hobbs_regression_food_remains_priority_when_enemy_is_one_longer() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 9 },
                Coord { x: 2, y: 8 },
                Coord { x: 2, y: 7 },
            ],
        );
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 3, y: 6 },
                Coord { x: 4, y: 6 },
                Coord { x: 5, y: 6 },
                Coord { x: 6, y: 6 },
            ],
        );
        let state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![
                Coord { x: 0, y: 2 },
                Coord { x: 5, y: 5 },
                Coord { x: 1, y: 9 },
                Coord { x: 9, y: 4 },
            ],
        );

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert_eq!(decision.direction, Direction::Left);
    }

    #[test]
    fn hobbs_regression_does_not_voluntarily_enter_corner_pin() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 9, y: 10 },
                Coord { x: 9, y: 9 },
                Coord { x: 9, y: 8 },
            ],
        );
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 7, y: 8 },
                Coord { x: 7, y: 7 },
                Coord { x: 8, y: 7 },
                Coord { x: 8, y: 6 },
                Coord { x: 7, y: 6 },
            ],
        );
        let state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![
                Coord { x: 0, y: 2 },
                Coord { x: 5, y: 5 },
                Coord { x: 1, y: 9 },
                Coord { x: 9, y: 4 },
                Coord { x: 0, y: 8 },
            ],
        );

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert_eq!(decision.direction, Direction::Left);
    }

    #[test]
    fn hobbs_cycle_one_turn_34_avoids_fatal_top_edge_corridor() {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 4, y: 10 },
                Coord { x: 4, y: 9 },
                Coord { x: 4, y: 8 },
                Coord { x: 3, y: 8 },
            ],
        );
        ours.health = 87;

        let mut enemy = snake(
            "enemy",
            vec![
                Coord { x: 5, y: 7 },
                Coord { x: 4, y: 7 },
                Coord { x: 3, y: 7 },
                Coord { x: 2, y: 7 },
                Coord { x: 2, y: 6 },
                Coord { x: 1, y: 6 },
            ],
        );
        enemy.health = 93;

        let mut state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![
                Coord { x: 8, y: 10 },
                Coord { x: 9, y: 3 },
                Coord { x: 10, y: 2 },
                Coord { x: 10, y: 8 },
            ],
        );
        state.turn = 34;
        state.game.ruleset.insert(
            "settings".to_string(),
            json!({
                "foodSpawnChance": 15,
                "minimumFood": 1,
                "hazardDamagePerTurn": 14
            }),
        );

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert_eq!(
            decision.direction,
            Direction::Left,
            "turn 34 must stay out of the top-edge corridor: {decision:?}"
        );
        assert!(decision.search.analyzed_depth <= crate::search::beam::MAX_BEAM_DEPTH);
        assert!(decision.search.completed_depth <= crate::search::beam::MAX_BEAM_DEPTH);
    }

    #[test]
    fn hobbs_cycle_two_turn_114_does_not_accept_exact_best_response_trap() {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 6 },
                Coord { x: 2, y: 7 },
                Coord { x: 2, y: 8 },
                Coord { x: 2, y: 9 },
                Coord { x: 3, y: 9 },
                Coord { x: 3, y: 8 },
                Coord { x: 4, y: 8 },
                Coord { x: 4, y: 7 },
                Coord { x: 4, y: 6 },
                Coord { x: 4, y: 5 },
                Coord { x: 5, y: 5 },
                Coord { x: 5, y: 6 },
                Coord { x: 5, y: 7 },
            ],
        );
        ours.health = 98;

        let mut enemy = snake(
            "enemy",
            vec![
                Coord { x: 3, y: 3 },
                Coord { x: 3, y: 2 },
                Coord { x: 2, y: 2 },
                Coord { x: 1, y: 2 },
                Coord { x: 0, y: 2 },
                Coord { x: 0, y: 3 },
                Coord { x: 1, y: 3 },
                Coord { x: 1, y: 4 },
                Coord { x: 1, y: 5 },
                Coord { x: 0, y: 5 },
                Coord { x: 0, y: 6 },
                Coord { x: 0, y: 7 },
                Coord { x: 0, y: 8 },
            ],
        );
        enemy.health = 96;

        let mut state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![Coord { x: 10, y: 6 }, Coord { x: 8, y: 10 }],
        );
        state.turn = 114;
        state.game.ruleset.insert(
            "settings".to_string(),
            json!({
                "foodSpawnChance": 15,
                "minimumFood": 1,
                "hazardDamagePerTurn": 14
            }),
        );

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert_ne!(
            decision.direction,
            Direction::Down,
            "turn 114 must account for Hobbs' near-best trapping response: {decision:?}"
        );
    }

    #[test]
    fn hobbs_cycle_four_turn_253_avoids_food_self_trap() {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 6, y: 3 },
                Coord { x: 6, y: 2 },
                Coord { x: 7, y: 2 },
                Coord { x: 8, y: 2 },
                Coord { x: 8, y: 3 },
                Coord { x: 8, y: 4 },
                Coord { x: 7, y: 4 },
                Coord { x: 7, y: 5 },
                Coord { x: 7, y: 6 },
                Coord { x: 7, y: 7 },
                Coord { x: 7, y: 8 },
                Coord { x: 8, y: 8 },
                Coord { x: 8, y: 9 },
                Coord { x: 7, y: 9 },
                Coord { x: 6, y: 9 },
                Coord { x: 6, y: 8 },
                Coord { x: 6, y: 7 },
            ],
        );
        ours.health = 74;

        let mut enemy = snake(
            "enemy",
            vec![
                Coord { x: 3, y: 6 },
                Coord { x: 3, y: 5 },
                Coord { x: 4, y: 5 },
                Coord { x: 5, y: 5 },
                Coord { x: 5, y: 4 },
                Coord { x: 5, y: 3 },
                Coord { x: 5, y: 2 },
                Coord { x: 4, y: 2 },
                Coord { x: 4, y: 1 },
                Coord { x: 4, y: 0 },
                Coord { x: 3, y: 0 },
                Coord { x: 2, y: 0 },
                Coord { x: 2, y: 1 },
                Coord { x: 1, y: 1 },
                Coord { x: 1, y: 0 },
                Coord { x: 0, y: 0 },
                Coord { x: 0, y: 1 },
                Coord { x: 0, y: 2 },
                Coord { x: 1, y: 2 },
                Coord { x: 1, y: 3 },
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
                Coord { x: 3, y: 4 },
            ],
        );
        enemy.health = 93;

        let mut state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![
                Coord { x: 0, y: 3 },
                Coord { x: 2, y: 10 },
                Coord { x: 7, y: 10 },
                Coord { x: 9, y: 7 },
                Coord { x: 10, y: 7 },
                Coord { x: 1, y: 6 },
                Coord { x: 1, y: 9 },
                Coord { x: 7, y: 3 },
                Coord { x: 8, y: 10 },
                Coord { x: 9, y: 6 },
                Coord { x: 3, y: 9 },
            ],
        );
        state.turn = 253;
        state.game.ruleset.insert(
            "settings".to_string(),
            json!({
                "foodSpawnChance": 15,
                "minimumFood": 1,
                "hazardDamagePerTurn": 14
            }),
        );

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert_eq!(
            decision.direction,
            Direction::Up,
            "turn 253 must reject the adjacent food that leaves zero deterministic exits: {decision:?}"
        );
    }

    #[test]
    fn diagnostic_hobbs_cycle_five_turn_268_root_scores() {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 5, y: 3 },
                Coord { x: 6, y: 3 },
                Coord { x: 7, y: 3 },
                Coord { x: 7, y: 2 },
                Coord { x: 8, y: 2 },
                Coord { x: 9, y: 2 },
                Coord { x: 9, y: 1 },
                Coord { x: 10, y: 1 },
                Coord { x: 10, y: 2 },
                Coord { x: 10, y: 3 },
                Coord { x: 9, y: 3 },
                Coord { x: 9, y: 4 },
                Coord { x: 8, y: 4 },
                Coord { x: 7, y: 4 },
                Coord { x: 6, y: 4 },
                Coord { x: 6, y: 5 },
                Coord { x: 7, y: 5 },
                Coord { x: 7, y: 6 },
                Coord { x: 6, y: 6 },
                Coord { x: 6, y: 7 },
                Coord { x: 6, y: 8 },
                Coord { x: 6, y: 9 },
                Coord { x: 6, y: 10 },
                Coord { x: 5, y: 10 },
                Coord { x: 5, y: 9 },
                Coord { x: 4, y: 9 },
                Coord { x: 3, y: 9 },
                Coord { x: 3, y: 8 },
            ],
        );
        ours.health = 95;

        let mut enemy = snake(
            "enemy",
            vec![
                Coord { x: 4, y: 6 },
                Coord { x: 3, y: 6 },
                Coord { x: 3, y: 5 },
                Coord { x: 2, y: 5 },
                Coord { x: 1, y: 5 },
                Coord { x: 1, y: 4 },
                Coord { x: 2, y: 4 },
                Coord { x: 2, y: 3 },
                Coord { x: 3, y: 3 },
                Coord { x: 3, y: 4 },
                Coord { x: 4, y: 4 },
                Coord { x: 4, y: 3 },
                Coord { x: 4, y: 2 },
                Coord { x: 4, y: 1 },
                Coord { x: 5, y: 1 },
                Coord { x: 6, y: 1 },
                Coord { x: 7, y: 1 },
                Coord { x: 8, y: 1 },
                Coord { x: 8, y: 0 },
                Coord { x: 7, y: 0 },
                Coord { x: 6, y: 0 },
            ],
        );
        enemy.health = 86;

        let mut state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![
                Coord { x: 1, y: 1 },
                Coord { x: 0, y: 1 },
                Coord { x: 0, y: 8 },
                Coord { x: 0, y: 5 },
                Coord { x: 4, y: 5 },
                Coord { x: 2, y: 0 },
            ],
        );
        state.turn = 268;
        state.game.ruleset.insert(
            "settings".to_string(),
            json!({
                "foodSpawnChance": 15,
                "minimumFood": 1,
                "hazardDamagePerTurn": 14
            }),
        );

        let normalized = crate::simulation::state::SimulatedGameState::from(&state);
        let policy = crate::search::forecast::FoodForecastPolicy::from_game_state(&state);
        let mut graph =
            crate::search::graph::FutureGraph::new_beam_with_forecast(normalized, policy);
        let budget = crate::search::budget::SearchBudget::from_state_with_extra_reserve(&state, 0);
        let result = crate::search::beam_search::search_beam(&mut graph, &budget)
            .unwrap()
            .expect("turn 268 beam must return candidates");

        let root = graph.root();
        let ours_actor = graph.node(root).state.actor_index("ours").unwrap();
        let enemy_actor = graph.node(root).state.actor_index("enemy").unwrap();
        let rows = result
            .checkpoint
            .lines
            .iter()
            .map(|line| {
                let first = line.path.first().and_then(|step| {
                    graph.node(step.node).children.iter().find(|edge| {
                        edge.child == step.child && edge.joint_action == step.joint_action
                    })
                });
                let ours_score = first.and_then(|edge| edge.transition.for_actor(ours_actor));
                let enemy_move = line
                    .path
                    .first()
                    .and_then(|step| step.joint_action.direction_for(enemy_actor));
                let child_safe_moves = first
                    .and_then(|edge| graph.node(edge.child).active_analysis())
                    .and_then(|analysis| analysis.actor_snapshot(ours_actor))
                    .map(|snapshot| snapshot.metrics.safe_non_reverse_moves);

                (
                    line.root_direction,
                    enemy_move,
                    line.value,
                    line.our_utility_total,
                    line.opponent_utility_total,
                    line.depth,
                    line.terminal,
                    line.certainty,
                    ours_score.map(|score| score.food_benefit),
                    ours_score.map(|score| score.survival_harm),
                    ours_score.map(|score| score.net),
                    child_safe_moves,
                )
            })
            .collect::<Vec<_>>();

        let actual_profile = crate::enemy::profile::OpponentProfile {
            food_bias_milli: 1286,
            hunting_bias_milli: 968,
            trapping_bias_milli: 1075,
            head_threat_bias_milli: 955,
            ..crate::enemy::profile::OpponentProfile::default()
        };
        let hypotheses = graph
            .node(root)
            .active_analysis()
            .and_then(|analysis| analysis.tracing.for_actor(enemy_actor))
            .map(|set| {
                set.hypotheses
                    .iter()
                    .map(|hypothesis| {
                        (
                            hypothesis.direction,
                            hypothesis.plausibility_milli,
                            actual_profile.adjusted_plausibility(*hypothesis),
                            hypothesis.support.food,
                            hypothesis.support.hunting,
                            hypothesis.support.trapping_milli,
                            hypothesis.support.head_threat,
                            hypothesis.threat,
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let response_rows = graph
            .node(root)
            .children
            .iter()
            .filter(|edge| edge.joint_action.direction_for(ours_actor) == Some(Direction::Up))
            .filter_map(|edge| {
                let enemy_move = edge.joint_action.direction_for(enemy_actor)?;
                let child = graph.node(edge.child);
                let certainty = if child.is_terminal() {
                    crate::search::forecast::ForecastCertainty::Deterministic
                } else {
                    crate::search::forecast::ForecastCertainty::Deterministic
                        .after(edge.forecast_delta)
                };
                let continuation = crate::search::maximin::evaluate_continuations(
                    &graph, edge.child, 2, certainty,
                )
                .into_iter()
                .next();
                let edge_enemy = edge.transition.for_actor(enemy_actor)?;
                let edge_ours = edge.transition.for_actor(ours_actor)?;
                let enemy_future = continuation
                    .as_ref()
                    .and_then(|line| line.actor_utility_totals.get(enemy_actor).copied())
                    .unwrap_or(0);
                let ours_future = continuation
                    .as_ref()
                    .and_then(|line| line.actor_utility_totals.get(ours_actor).copied())
                    .unwrap_or(0);
                Some((
                    enemy_move,
                    edge_enemy.actor_choice_net,
                    enemy_future,
                    edge_enemy.actor_choice_net.saturating_add(enemy_future),
                    edge_ours.actor_choice_net,
                    ours_future,
                    edge_ours.actor_choice_net.saturating_add(ours_future),
                    child
                        .active_analysis()
                        .and_then(|analysis| analysis.actor_snapshot(ours_actor))
                        .map(|snapshot| snapshot.metrics.safe_non_reverse_moves),
                ))
            })
            .collect::<Vec<_>>();

        panic!(
            "CYCLE5_T268_DIAGNOSTIC completed_depth={} rows={rows:?} hypotheses={hypotheses:?} responses={response_rows:?}",
            result.completed_depth()
        );
    }

    #[test]
    fn diagnostic_hobbs_cycle_five_turn_267_and_268_root_scores() {
        fn diagnose(
            state: GameState,
            label: &str,
        ) -> Vec<(
            Direction,
            i64,
            i64,
            i64,
            u8,
            LineTerminal,
            ForecastCertainty,
            Option<u8>,
        )> {
            let normalized = crate::simulation::state::SimulatedGameState::from(&state);
            let policy = crate::search::forecast::FoodForecastPolicy::from_game_state(&state);
            let mut graph =
                crate::search::graph::FutureGraph::new_beam_with_forecast(normalized, policy);
            let budget =
                crate::search::budget::SearchBudget::from_state_with_extra_reserve(&state, 0);
            let result = crate::search::beam_search::search_beam(&mut graph, &budget)
                .unwrap()
                .expect("beam must return candidates");

            let root = graph.root();
            let ours_actor = graph.node(root).state.actor_index("ours").unwrap();
            let rows = result
                .checkpoint
                .lines
                .iter()
                .map(|line| {
                    let child_safe_moves = line
                        .path
                        .first()
                        .and_then(|step| {
                            graph.node(step.node).children.iter().find(|edge| {
                                edge.child == step.child && edge.joint_action == step.joint_action
                            })
                        })
                        .and_then(|edge| graph.node(edge.child).active_analysis())
                        .and_then(|analysis| analysis.actor_snapshot(ours_actor))
                        .map(|snapshot| snapshot.metrics.safe_non_reverse_moves);

                    (
                        line.root_direction,
                        line.value,
                        line.our_utility_total,
                        line.opponent_utility_total,
                        line.depth,
                        line.terminal,
                        line.certainty,
                        child_safe_moves,
                    )
                })
                .collect::<Vec<_>>();

            eprintln!(
                "{label} completed_depth={} rows={rows:?}",
                result.completed_depth()
            );
            rows
        }

        let mut ours_267 = snake(
            "ours",
            vec![
                Coord { x: 6, y: 3 },
                Coord { x: 7, y: 3 },
                Coord { x: 7, y: 2 },
                Coord { x: 8, y: 2 },
                Coord { x: 9, y: 2 },
                Coord { x: 9, y: 1 },
                Coord { x: 10, y: 1 },
                Coord { x: 10, y: 2 },
                Coord { x: 10, y: 3 },
                Coord { x: 9, y: 3 },
                Coord { x: 9, y: 4 },
                Coord { x: 8, y: 4 },
                Coord { x: 7, y: 4 },
                Coord { x: 6, y: 4 },
                Coord { x: 6, y: 5 },
                Coord { x: 7, y: 5 },
                Coord { x: 7, y: 6 },
                Coord { x: 6, y: 6 },
                Coord { x: 6, y: 7 },
                Coord { x: 6, y: 8 },
                Coord { x: 6, y: 9 },
                Coord { x: 6, y: 10 },
                Coord { x: 5, y: 10 },
                Coord { x: 5, y: 9 },
                Coord { x: 4, y: 9 },
                Coord { x: 3, y: 9 },
                Coord { x: 3, y: 8 },
                Coord { x: 4, y: 8 },
            ],
        );
        ours_267.health = 96;

        let mut enemy_267 = snake(
            "enemy",
            vec![
                Coord { x: 3, y: 6 },
                Coord { x: 3, y: 5 },
                Coord { x: 2, y: 5 },
                Coord { x: 1, y: 5 },
                Coord { x: 1, y: 4 },
                Coord { x: 2, y: 4 },
                Coord { x: 2, y: 3 },
                Coord { x: 3, y: 3 },
                Coord { x: 3, y: 4 },
                Coord { x: 4, y: 4 },
                Coord { x: 4, y: 3 },
                Coord { x: 4, y: 2 },
                Coord { x: 4, y: 1 },
                Coord { x: 5, y: 1 },
                Coord { x: 6, y: 1 },
                Coord { x: 7, y: 1 },
                Coord { x: 8, y: 1 },
                Coord { x: 8, y: 0 },
                Coord { x: 7, y: 0 },
                Coord { x: 6, y: 0 },
                Coord { x: 5, y: 0 },
            ],
        );
        enemy_267.health = 96;

        let mut state_267 = state_on_board(
            11,
            11,
            ours_267,
            vec![enemy_267],
            vec![
                Coord { x: 1, y: 1 },
                Coord { x: 0, y: 1 },
                Coord { x: 0, y: 8 },
                Coord { x: 0, y: 5 },
                Coord { x: 4, y: 5 },
                Coord { x: 2, y: 0 },
            ],
        );
        state_267.turn = 267;
        state_267.game.ruleset.insert(
            "settings".to_string(),
            json!({
                "foodSpawnChance": 15,
                "minimumFood": 1,
                "hazardDamagePerTurn": 14
            }),
        );

        let mut ours_268 = snake(
            "ours",
            vec![
                Coord { x: 5, y: 3 },
                Coord { x: 6, y: 3 },
                Coord { x: 7, y: 3 },
                Coord { x: 7, y: 2 },
                Coord { x: 8, y: 2 },
                Coord { x: 9, y: 2 },
                Coord { x: 9, y: 1 },
                Coord { x: 10, y: 1 },
                Coord { x: 10, y: 2 },
                Coord { x: 10, y: 3 },
                Coord { x: 9, y: 3 },
                Coord { x: 9, y: 4 },
                Coord { x: 8, y: 4 },
                Coord { x: 7, y: 4 },
                Coord { x: 6, y: 4 },
                Coord { x: 6, y: 5 },
                Coord { x: 7, y: 5 },
                Coord { x: 7, y: 6 },
                Coord { x: 6, y: 6 },
                Coord { x: 6, y: 7 },
                Coord { x: 6, y: 8 },
                Coord { x: 6, y: 9 },
                Coord { x: 6, y: 10 },
                Coord { x: 5, y: 10 },
                Coord { x: 5, y: 9 },
                Coord { x: 4, y: 9 },
                Coord { x: 3, y: 9 },
                Coord { x: 3, y: 8 },
            ],
        );
        ours_268.health = 95;

        let mut enemy_268 = snake(
            "enemy",
            vec![
                Coord { x: 4, y: 6 },
                Coord { x: 3, y: 6 },
                Coord { x: 3, y: 5 },
                Coord { x: 2, y: 5 },
                Coord { x: 1, y: 5 },
                Coord { x: 1, y: 4 },
                Coord { x: 2, y: 4 },
                Coord { x: 2, y: 3 },
                Coord { x: 3, y: 3 },
                Coord { x: 3, y: 4 },
                Coord { x: 4, y: 4 },
                Coord { x: 4, y: 3 },
                Coord { x: 4, y: 2 },
                Coord { x: 4, y: 1 },
                Coord { x: 5, y: 1 },
                Coord { x: 6, y: 1 },
                Coord { x: 7, y: 1 },
                Coord { x: 8, y: 1 },
                Coord { x: 8, y: 0 },
                Coord { x: 7, y: 0 },
                Coord { x: 6, y: 0 },
            ],
        );
        enemy_268.health = 95;

        let mut state_268 = state_on_board(
            11,
            11,
            ours_268,
            vec![enemy_268],
            vec![
                Coord { x: 1, y: 1 },
                Coord { x: 0, y: 1 },
                Coord { x: 0, y: 8 },
                Coord { x: 0, y: 5 },
                Coord { x: 4, y: 5 },
                Coord { x: 2, y: 0 },
            ],
        );
        state_268.turn = 268;
        state_268.game.ruleset.insert(
            "settings".to_string(),
            json!({
                "foodSpawnChance": 15,
                "minimumFood": 1,
                "hazardDamagePerTurn": 14
            }),
        );

        let rows_267 = diagnose(state_267, "CYCLE5_T267_DIAGNOSTIC");
        let rows_268 = diagnose(state_268, "CYCLE5_T268_DIAGNOSTIC");

        panic!("CYCLE5_DIAGNOSTIC_DONE t267={rows_267:?} t268={rows_268:?}");
    }

    #[test]
    fn beam_avoids_equal_head_to_head_when_enemy_response_is_forced() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                Coord { x: 1, y: 1 },
                Coord { x: 1, y: 0 },
                Coord { x: 0, y: 0 },
                Coord { x: 0, y: 1 },
                Coord { x: 0, y: 2 },
            ],
        );
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 4, y: 2 },
                Coord { x: 4, y: 1 },
                Coord { x: 5, y: 1 },
                Coord { x: 5, y: 2 },
                Coord { x: 5, y: 3 },
                Coord { x: 4, y: 3 },
                Coord { x: 3, y: 3 },
            ],
        );
        let state = state(ours, vec![enemy], vec![Coord { x: 3, y: 2 }]);

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert_ne!(decision.direction, Direction::Right);
    }

    #[test]
    fn strategy_uses_baseline_when_no_enemy_exists() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 3, y: 3 },
                Coord { x: 3, y: 2 },
                Coord { x: 3, y: 1 },
            ],
        );
        let state = state(ours, vec![], vec![]);

        let decision = choose_move(&state);

        assert_eq!(decision.reason, DecisionReason::BaselineFallback);
        assert!(decision.reachable_cells > 0);
    }

    #[test]
    fn no_safe_move_fallback_stays_inside_board() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 0, y: 6 },
                Coord { x: 0, y: 5 },
                Coord { x: 1, y: 5 },
                Coord { x: 1, y: 6 },
                Coord { x: 2, y: 6 },
            ],
        );
        let state = state(ours, vec![], vec![Coord { x: 0, y: 4 }]);

        let decision = choose_move_baseline(&state);

        assert_eq!(decision.reason, DecisionReason::NoSafeMove);
        assert!(direction_stays_in_bounds(&state, decision.direction));
        assert!(matches!(
            decision.direction,
            Direction::Right | Direction::Down
        ));
    }

    #[test]
    fn strategy_prefers_non_hazard_food_candidate_before_hazard_candidate() {
        let ours = snake("ours", vec![Coord { x: 3, y: 3 }]);
        let hazard_food = Coord { x: 4, y: 3 };
        let safe_food = Coord { x: 3, y: 5 };
        let mut state = state(ours, vec![], vec![hazard_food, safe_food]);
        state.board.hazards = vec![hazard_food];

        let decision = choose_move(&state);

        assert_eq!(decision.direction, Direction::Up);
        assert_eq!(decision.reason, DecisionReason::BaselineFallback);
    }
}
