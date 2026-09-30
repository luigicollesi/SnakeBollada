use serde::{Deserialize, Serialize};

use crate::analysis::StateAnalysis;
use crate::direction::Direction;
use crate::forecast::ForecastCertainty;
use crate::modes::food;
use crate::navigation::{reachable_after_move, NavigationMap};
use crate::simulation::state::SimulatedGameState;
use crate::{Coord, GameState};

pub(crate) const STRATEGY_VERSION: &str = "future-search-v1";

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DecisionReason {
    NearestSafeFood,
    SurvivalFallback,
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
    pub(crate) min_future_mobility: u8,
    pub(crate) min_reachable_space: u32,
    pub(crate) worst_utility_milli: i32,
    pub(crate) average_utility_milli: i32,
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
            min_future_mobility: 0,
            min_reachable_space: 0,
            worst_utility_milli: 0,
            average_utility_milli: 0,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchMetadata {
    pub(crate) completed_depth: u8,
    pub(crate) nodes: u32,
    pub(crate) edges: u32,
    pub(crate) transposition_hits: u32,
    pub(crate) cache_reused: bool,
    pub(crate) cache_invalidation: CacheInvalidationReason,
    pub(crate) elapsed_us: u64,
    pub(crate) safety_reserve_us: u64,
    pub(crate) aggression_milli: u16,
    pub(crate) enemy_moves_observed: u16,
    pub(crate) enemy_moves_legal_covered: u16,
    pub(crate) enemy_moves_plausible_covered: u16,
    pub(crate) food_spawn_invalidations: u32,
    pub(crate) food_mutation_invalidations: u32,
    pub(crate) depth_stats: [DepthSearchStats; 6],
    pub(crate) direction_outcomes: [DirectionOutcomeSummary; 4],
}

impl Default for SearchMetadata {
    fn default() -> Self {
        Self {
            completed_depth: 0,
            nodes: 0,
            edges: 0,
            transposition_hits: 0,
            cache_reused: false,
            cache_invalidation: CacheInvalidationReason::None,
            elapsed_us: 0,
            safety_reserve_us: 0,
            aggression_milli: 0,
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) direction: Direction,
    pub(crate) reason: DecisionReason,
    pub(crate) target_food: Option<Coord>,
    pub(crate) path_distance: Option<u16>,
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
            target_food: None,
            path_distance: None,
            reachable_cells: 0,
            search: SearchMetadata::default(),
        };
    }

    if legal_moves.len() == 1 {
        let direction = legal_moves[0];
        return Decision {
            direction,
            reason: DecisionReason::OnlyLegalMove,
            target_food: None,
            path_distance: None,
            reachable_cells: reachable_after_move(&map, state, direction),
            search: SearchMetadata::default(),
        };
    }

    let simulated = SimulatedGameState::from(state);
    let analysis = StateAnalysis::from_simulated(&simulated);
    let food_output = food::candidates(&simulated, &analysis, ForecastCertainty::Deterministic);

    if let Some(decision) = choose_food_candidate(&map, state, &legal_moves, &food_output, false) {
        return decision;
    }

    if let Some(decision) = choose_food_candidate(&map, state, &legal_moves, &food_output, true) {
        return decision;
    }

    survival_fallback(&map, state, &legal_moves)
}

fn choose_food_candidate(
    map: &NavigationMap,
    state: &GameState,
    legal_moves: &[Direction],
    output: &food::FoodModeOutput,
    allow_hazards: bool,
) -> Option<Decision> {
    for candidate in &output.candidates {
        if !legal_moves.contains(&candidate.first_move) {
            continue;
        }

        let destination = candidate.first_move.apply(state.you.head);
        if !allow_hazards && map.is_hazard(destination) {
            continue;
        }

        let reachable = reachable_after_move(map, state, candidate.first_move);
        if reachable < state.you.length {
            continue;
        }

        return Some(Decision {
            direction: candidate.first_move,
            reason: DecisionReason::NearestSafeFood,
            target_food: Some(candidate.target_food),
            path_distance: Some(candidate.distance),
            reachable_cells: reachable,
            search: SearchMetadata::default(),
        });
    }

    None
}

fn survival_fallback(
    map: &NavigationMap,
    state: &GameState,
    legal_moves: &[Direction],
) -> Decision {
    let evaluations = legal_moves
        .iter()
        .copied()
        .map(|direction| {
            let destination = direction.apply(state.you.head);
            (
                direction,
                reachable_after_move(map, state, direction),
                map.is_hazard(destination),
            )
        })
        .collect::<Vec<_>>();

    let best = evaluations
        .iter()
        .copied()
        .filter(|(_, reachable, hazard)| !hazard && *reachable >= state.you.length)
        .min_by_key(|(direction, reachable, _)| (std::cmp::Reverse(*reachable), direction.rank()))
        .or_else(|| {
            evaluations
                .iter()
                .copied()
                .filter(|(_, reachable, _)| *reachable >= state.you.length)
                .min_by_key(|(direction, reachable, hazard)| {
                    (std::cmp::Reverse(*reachable), *hazard, direction.rank())
                })
        })
        .unwrap_or_else(|| {
            evaluations
                .iter()
                .copied()
                .min_by_key(|(direction, reachable, hazard)| {
                    (std::cmp::Reverse(*reachable), *hazard, direction.rank())
                })
                .expect("legal moves are not empty")
        });

    Decision {
        direction: best.0,
        reason: DecisionReason::SurvivalFallback,
        target_food: None,
        path_distance: None,
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
        assert_eq!(decision.reason, DecisionReason::NearestSafeFood);
    }

    #[test]
    fn strategy_uses_food_mode_claim_competition() {
        let ours = snake("ours", vec![Coord { x: 0, y: 0 }]);
        let enemy = snake("enemy", vec![Coord { x: 3, y: 2 }]);
        let losing_food = Coord { x: 3, y: 0 };
        let claimable_food = Coord { x: 0, y: 4 };
        let state = state(ours, vec![enemy], vec![losing_food, claimable_food]);

        let decision = choose_move(&state);

        assert_eq!(decision.direction, Direction::Up);
        assert_eq!(decision.target_food, Some(claimable_food));
    }

    #[test]
    fn strategy_can_take_second_food_direction_when_first_candidate_is_immediately_unsafe() {
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

        assert_eq!(decision.direction, Direction::Up);
        assert_eq!(decision.target_food, Some(safe_food));
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

        assert_eq!(decision.reason, DecisionReason::SurvivalFallback);
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
