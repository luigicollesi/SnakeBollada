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
    pub(crate) our_frontier_hunting_utility: i64,
    pub(crate) our_frontier_territory: i64,
    pub(crate) our_frontier_length: i64,
    pub(crate) our_frontier_constriction: i64,
    pub(crate) our_frontier_escape: i64,
    pub(crate) our_frontier_survival_availability_milli: u16,
    pub(crate) our_survival_utility: i64,
    pub(crate) our_terminal_utility: i64,
    pub(crate) opponent_food_utility: i64,
    pub(crate) opponent_hunting_utility: i64,
    pub(crate) opponent_frontier_hunting_utility: i64,
    pub(crate) opponent_frontier_territory: i64,
    pub(crate) opponent_frontier_length: i64,
    pub(crate) opponent_frontier_constriction: i64,
    pub(crate) opponent_frontier_escape: i64,
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
    fn hobbs_cycle_six_turn_133_preserves_second_escape_lane() {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 0, y: 1 },
                Coord { x: 0, y: 2 },
                Coord { x: 1, y: 2 },
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 3 },
                Coord { x: 3, y: 3 },
                Coord { x: 3, y: 4 },
                Coord { x: 3, y: 5 },
                Coord { x: 3, y: 6 },
                Coord { x: 3, y: 7 },
                Coord { x: 2, y: 7 },
                Coord { x: 2, y: 8 },
                Coord { x: 3, y: 8 },
                Coord { x: 4, y: 8 },
            ],
        );
        ours.health = 98;

        let mut enemy = snake(
            "enemy",
            vec![
                Coord { x: 6, y: 3 },
                Coord { x: 6, y: 4 },
                Coord { x: 7, y: 4 },
                Coord { x: 8, y: 4 },
                Coord { x: 8, y: 5 },
                Coord { x: 9, y: 5 },
                Coord { x: 9, y: 4 },
                Coord { x: 9, y: 3 },
                Coord { x: 8, y: 3 },
                Coord { x: 7, y: 3 },
                Coord { x: 7, y: 2 },
                Coord { x: 8, y: 2 },
                Coord { x: 9, y: 2 },
                Coord { x: 9, y: 1 },
            ],
        );
        enemy.health = 97;

        let mut state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![Coord { x: 3, y: 1 }, Coord { x: 4, y: 10 }],
        );
        state.turn = 133;
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
            Direction::Right,
            "turn 133 must preserve two deterministic exits instead of entering a forced corridor: {decision:?}"
        );
    }

    #[test]
    fn hobbs_cycle_ten_turn_81_keeps_full_depth_better_line() {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 3, y: 0 },
                Coord { x: 2, y: 0 },
                Coord { x: 1, y: 0 },
                Coord { x: 1, y: 1 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 2 },
                Coord { x: 3, y: 2 },
                Coord { x: 3, y: 3 },
                Coord { x: 2, y: 3 },
            ],
        );
        ours.health = 98;

        let mut enemy = snake(
            "enemy",
            vec![
                Coord { x: 7, y: 6 },
                Coord { x: 8, y: 6 },
                Coord { x: 9, y: 6 },
                Coord { x: 10, y: 6 },
                Coord { x: 10, y: 5 },
                Coord { x: 9, y: 5 },
                Coord { x: 8, y: 5 },
                Coord { x: 7, y: 5 },
            ],
        );
        enemy.health = 96;

        let mut state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![
                Coord { x: 9, y: 10 },
                Coord { x: 0, y: 1 },
                Coord { x: 4, y: 10 },
            ],
        );
        state.turn = 81;
        state.game.timeout = 500;
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
            "turn 81 must trust the much stronger full-depth line instead of an unconditional immediate-mobility override: {decision:?}"
        );
    }

    #[test]
    fn hobbs_cycle_eight_turn_67_preserves_immediate_escape_flexibility() {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 9, y: 4 },
                Coord { x: 9, y: 3 },
                Coord { x: 10, y: 3 },
                Coord { x: 10, y: 2 },
                Coord { x: 9, y: 2 },
                Coord { x: 8, y: 2 },
                Coord { x: 8, y: 1 },
                Coord { x: 7, y: 1 },
            ],
        );
        ours.health = 76;

        let mut enemy = snake(
            "enemy",
            vec![
                Coord { x: 6, y: 5 },
                Coord { x: 6, y: 4 },
                Coord { x: 6, y: 3 },
                Coord { x: 7, y: 3 },
                Coord { x: 7, y: 4 },
            ],
        );
        enemy.health = 63;

        let mut state = state_on_board(
            11,
            11,
            ours,
            vec![enemy],
            vec![
                Coord { x: 0, y: 7 },
                Coord { x: 10, y: 5 },
                Coord { x: 1, y: 0 },
                Coord { x: 0, y: 2 },
                Coord { x: 0, y: 1 },
                Coord { x: 4, y: 8 },
            ],
        );
        state.turn = 67;
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
            "turn 67 must preserve multiple immediate escape lanes instead of entering a forced corridor: {decision:?}"
        );
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
