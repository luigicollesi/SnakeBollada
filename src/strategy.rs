use serde::{Deserialize, Serialize};

use crate::analysis::StrategicPhase;
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

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CacheInvalidationReason {
    #[default]
    None,
    TurnMismatch,
    FoodSpawn,
    FoodMutation,
    StateMismatch,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectionOutcomeSummary {
    pub(crate) direction: Direction,
    pub(crate) available: bool,
    pub(crate) total_routes: u32,
    pub(crate) death_routes: u32,
    pub(crate) dead_end_routes: u32,
    pub(crate) forced_routes: u32,
    pub(crate) constrained_routes: u32,
    pub(crate) average_border_ticks_milli: u32,
    pub(crate) average_border_cost_milli: u32,
    pub(crate) min_future_mobility: u8,
    pub(crate) min_reachable_space: u32,
    pub(crate) min_second_order_mobility: u32,
    pub(crate) worst_utility_milli: i32,
    pub(crate) average_utility_milli: i32,
    pub(crate) average_food_milli: i32,
    pub(crate) average_hunting_milli: i32,
    pub(crate) average_leaf_food_milli: i32,
    pub(crate) average_leaf_hunting_milli: i32,
    pub(crate) reserved_override: bool,
}

impl DirectionOutcomeSummary {
    pub(crate) const fn empty(direction: Direction) -> Self {
        Self {
            direction,
            available: false,
            total_routes: 0,
            death_routes: 0,
            dead_end_routes: 0,
            forced_routes: 0,
            constrained_routes: 0,
            average_border_ticks_milli: 0,
            average_border_cost_milli: 0,
            min_future_mobility: 0,
            min_reachable_space: 0,
            min_second_order_mobility: 0,
            worst_utility_milli: 0,
            average_utility_milli: 0,
            average_food_milli: 0,
            average_hunting_milli: 0,
            average_leaf_food_milli: 0,
            average_leaf_hunting_milli: 0,
            reserved_override: false,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DepthSearchStats {
    pub(crate) depth: u8,
    pub(crate) completed: bool,
    pub(crate) frontier_nodes: u32,
    pub(crate) new_nodes: u32,
    pub(crate) edges_generated: u32,
    pub(crate) branching_milli: u32,
    pub(crate) expansion_us: u64,
    pub(crate) evaluation_us: u64,
}

impl DepthSearchStats {
    pub(crate) const fn empty(depth: u8) -> Self {
        Self {
            depth,
            completed: false,
            frontier_nodes: 0,
            new_nodes: 0,
            edges_generated: 0,
            branching_milli: 0,
            expansion_us: 0,
            evaluation_us: 0,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchMetadata {
    pub(crate) completed_depth: u8,
    pub(crate) analyzed_depth: u8,
    pub(crate) nodes: u32,
    pub(crate) edges: u32,
    pub(crate) transposition_hits: u32,
    pub(crate) cache_reused: bool,
    pub(crate) cache_invalidation: CacheInvalidationReason,
    pub(crate) elapsed_us: u64,
    pub(crate) safety_reserve_us: u64,
    pub(crate) runtime_jitter_reserve_us: u64,
    pub(crate) dag_nodes_evaluated: u32,
    pub(crate) dag_memo_hits: u32,
    pub(crate) dag_deterministic_evaluations: u32,
    pub(crate) dag_provisional_evaluations: u32,
    pub(crate) aggression_milli: u16,
    pub(crate) strategic_phase: StrategicPhase,
    pub(crate) growth_aggression_milli: u16,
    pub(crate) size_dominance_milli: u16,
    pub(crate) hunt_drive_milli: u16,
    pub(crate) food_urgency_milli: u16,
    pub(crate) our_length: u16,
    pub(crate) largest_enemy_length: u16,
    pub(crate) control_ratio_milli: u16,
    pub(crate) dominance_frontier_cells: u16,
    pub(crate) border_risk_milli: u16,
    pub(crate) enemy_pin_risk_milli: u16,
    pub(crate) escape_pressure_milli: u16,
    pub(crate) escape_selected: bool,
    pub(crate) enemy_moves_observed: u16,
    pub(crate) enemy_moves_legal_covered: u16,
    pub(crate) enemy_moves_plausible_covered: u16,
    pub(crate) food_spawn_invalidations: u32,
    pub(crate) food_mutation_invalidations: u32,
    pub(crate) depth_stats: [DepthSearchStats; 6],
    pub(crate) direction_outcomes: [DirectionOutcomeSummary; 4],
    pub(crate) beam_shadow: BeamShadowMetadata,
}

impl Default for SearchMetadata {
    fn default() -> Self {
        Self {
            completed_depth: 0,
            analyzed_depth: 0,
            nodes: 0,
            edges: 0,
            transposition_hits: 0,
            cache_reused: false,
            cache_invalidation: CacheInvalidationReason::None,
            elapsed_us: 0,
            safety_reserve_us: 0,
            runtime_jitter_reserve_us: 0,
            dag_nodes_evaluated: 0,
            dag_memo_hits: 0,
            dag_deterministic_evaluations: 0,
            dag_provisional_evaluations: 0,
            aggression_milli: 0,
            strategic_phase: StrategicPhase::default(),
            growth_aggression_milli: 0,
            size_dominance_milli: 0,
            hunt_drive_milli: 0,
            food_urgency_milli: 0,
            our_length: 0,
            largest_enemy_length: 0,
            control_ratio_milli: 0,
            dominance_frontier_cells: 0,
            border_risk_milli: 0,
            enemy_pin_risk_milli: 0,
            escape_pressure_milli: 0,
            escape_selected: false,
            enemy_moves_observed: 0,
            enemy_moves_legal_covered: 0,
            enemy_moves_plausible_covered: 0,
            food_spawn_invalidations: 0,
            food_mutation_invalidations: 0,
            depth_stats: [
                DepthSearchStats::empty(1),
                DepthSearchStats::empty(2),
                DepthSearchStats::empty(3),
                DepthSearchStats::empty(4),
                DepthSearchStats::empty(5),
                DepthSearchStats::empty(6),
            ],
            direction_outcomes: [
                DirectionOutcomeSummary::empty(Direction::Up),
                DirectionOutcomeSummary::empty(Direction::Right),
                DirectionOutcomeSummary::empty(Direction::Down),
                DirectionOutcomeSummary::empty(Direction::Left),
            ],
            beam_shadow: BeamShadowMetadata::default(),
        }
    }
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

            (
                direction,
                reachable,
                hazard,
                cramped,
                food_distance,
            )
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
        assert_eq!(decision.target_food, None);
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
        assert_eq!(decision.target_food, Some(safe_food));
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
