use serde::{Deserialize, Serialize};

use crate::analysis::StateAnalysis;
use crate::direction::Direction;
use crate::modes::food;
use crate::forecast::ForecastCertainty;
use crate::simulation::state::SimulatedGameState;
use crate::navigation::{reachable_after_move, NavigationMap};
use crate::{Coord, GameState};

pub(crate) const STRATEGY_VERSION: &str = "food-mode-v1";

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DecisionReason {
    NearestSafeFood,
    SurvivalFallback,
    OnlyLegalMove,
    NoSafeMove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Decision {
    pub(crate) direction: Direction,
    pub(crate) reason: DecisionReason,
    pub(crate) target_food: Option<Coord>,
    pub(crate) path_distance: Option<u16>,
    pub(crate) reachable_cells: u32,
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
        };
    }

    let simulated = SimulatedGameState::from(state);
    let analysis = StateAnalysis::from_simulated(&simulated);
    let food_output = food::candidates(
        &simulated,
        &analysis,
        ForecastCertainty::Deterministic,
    );

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
