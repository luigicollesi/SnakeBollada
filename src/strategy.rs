use serde::{Deserialize, Serialize};

use crate::direction::Direction;
use crate::navigation::{reachable_after_move, NavigationMap};
use crate::{Coord, GameState};

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

pub(crate) fn choose_move_baseline(state: &GameState) -> Decision {
    let map = NavigationMap::from_state(state);
    let legal_moves = Direction::ALL
        .into_iter()
        .filter(|direction| !map.is_blocked(direction.apply(state.you.head)))
        .collect::<Vec<_>>();

    if legal_moves.is_empty() {
        return Decision {
            direction: Direction::Up,
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
    use crate::{Battlesnake, Board, Game};

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
                height: 7,
                width: 7,
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
        assert_eq!(decision.reason, DecisionReason::FoodStrategic);
    }

    #[test]
    fn strategy_uses_food_mode_claim_competition() {
        let ours = snake("ours", vec![Coord { x: 2, y: 2 }]);
        let enemy = snake("enemy", vec![Coord { x: 5, y: 5 }]);
        let losing_food = Coord { x: 5, y: 4 };
        let claimable_food = Coord { x: 2, y: 4 };
        let state = state(ours, vec![enemy], vec![losing_food, claimable_food]);

        let decision = choose_move(&state);

        assert_eq!(decision.direction, Direction::Up);
        assert_eq!(decision.reason, DecisionReason::BeamUtility);
    }

    #[test]
    fn strategy_rejects_immediately_unsafe_food_under_adversarial_search() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 0 },
            ],
        );
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 4, y: 2 },
                Coord { x: 4, y: 1 },
                Coord { x: 4, y: 0 },
            ],
        );
        let dangerous_food = Coord { x: 3, y: 2 };
        let safe_food = Coord { x: 3, y: 3 };
        let state = state(ours, vec![enemy], vec![dangerous_food, safe_food]);

        let decision = choose_move(&state);

        assert_ne!(decision.direction, Direction::Right);
        let selected = decision.search.direction_outcomes[usize::from(decision.direction.rank())];
        assert_eq!(selected.death_routes, 0);
        assert_eq!(selected.dead_end_routes, 0);
    }

    #[test]
    fn strategy_falls_back_to_survival_when_food_mode_has_no_candidate() {
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

        assert_eq!(decision.reason, DecisionReason::FutureMobility);
        assert_eq!(decision.target_food, None);
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

    #[test]
    fn avoids_equal_length_head_to_head() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 0 },
            ],
        );
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 4, y: 2 },
                Coord { x: 4, y: 1 },
                Coord { x: 4, y: 0 },
            ],
        );

        let state = state(ours, vec![enemy], vec![Coord { x: 3, y: 2 }]);
        assert_ne!(choose_move(&state).direction, Direction::Right);
    }
}
