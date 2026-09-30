use serde::{Deserialize, Serialize};

use crate::navigation::{food_candidates, reachable_after_move, NavigationMap};
use crate::{Coord, GameState};

pub(crate) const STRATEGY_VERSION: &str = "basic-v1";

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Direction {
    Up,
    Right,
    Down,
    Left,
}

impl Direction {
    pub(crate) const ALL: [Direction; 4] = [
        Direction::Up,
        Direction::Right,
        Direction::Down,
        Direction::Left,
    ];

    pub(crate) const fn apply(self, coord: Coord) -> Coord {
        match self {
            Direction::Up => Coord {
                x: coord.x,
                y: coord.y + 1,
            },
            Direction::Right => Coord {
                x: coord.x + 1,
                y: coord.y,
            },
            Direction::Down => Coord {
                x: coord.x,
                y: coord.y - 1,
            },
            Direction::Left => Coord {
                x: coord.x - 1,
                y: coord.y,
            },
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Right => "right",
            Direction::Down => "down",
            Direction::Left => "left",
        }
    }

    pub(crate) const fn rank(self) -> u8 {
        match self {
            Direction::Up => 0,
            Direction::Right => 1,
            Direction::Down => 2,
            Direction::Left => 3,
        }
    }

    pub(crate) fn from_heads(previous: Coord, current: Coord) -> Option<Self> {
        match (current.x - previous.x, current.y - previous.y) {
            (0, 1) => Some(Direction::Up),
            (1, 0) => Some(Direction::Right),
            (0, -1) => Some(Direction::Down),
            (-1, 0) => Some(Direction::Left),
            _ => None,
        }
    }
}

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

    if let Some(decision) = choose_food(&map, state, &legal_moves, false) {
        return decision;
    }

    if let Some(decision) = choose_food(&map, state, &legal_moves, true) {
        return decision;
    }

    survival_fallback(&map, state, &legal_moves)
}

fn choose_food(
    map: &NavigationMap,
    state: &GameState,
    legal_moves: &[Direction],
    allow_hazards: bool,
) -> Option<Decision> {
    for candidate in food_candidates(map, state.you.head, allow_hazards) {
        if !legal_moves.contains(&candidate.first_move) {
            continue;
        }

        let reachable = reachable_after_move(map, state, candidate.first_move);
        if reachable < state.you.length {
            continue;
        }

        return Some(Decision {
            direction: candidate.first_move,
            reason: DecisionReason::NearestSafeFood,
            target_food: Some(candidate.food),
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
