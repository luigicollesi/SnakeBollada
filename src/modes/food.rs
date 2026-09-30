#![cfg_attr(not(test), allow(dead_code))]

use std::cmp::Reverse;

use crate::analysis::{FoodClaimInfo, FoodRouteInfo, StateAnalysis};
use crate::direction::Direction;
use crate::forecast::ForecastCertainty;
use crate::{Coord, GameState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoodCandidate {
    pub(crate) target_food: Coord,
    pub(crate) first_move: Direction,
    pub(crate) distance: u16,
    pub(crate) claim_margin: Option<i16>,
    pub(crate) contested: bool,
    pub(crate) certainty: ForecastCertainty,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FoodModeOutput {
    pub(crate) candidates: Vec<FoodCandidate>,
}

pub(crate) fn candidates(state: &GameState, analysis: &StateAnalysis) -> FoodModeOutput {
    let mut expanded = Vec::new();

    for food in state.board.food.iter().copied() {
        let Some(route) = analysis.route_for(&state.you.id, food) else {
            continue;
        };
        if !route.reachable {
            continue;
        }

        let Some(distance) = route.distance else {
            continue;
        };
        let claim = analysis.claim_for(food);

        for first_move in route.first_moves.iter() {
            expanded.push(candidate_from_route(
                analysis, route, claim, first_move, distance,
            ));
        }
    }

    expanded.sort_by_key(candidate_rank);

    let mut selected = Vec::with_capacity(2);
    for candidate in expanded {
        if selected
            .iter()
            .any(|existing: &FoodCandidate| existing.first_move == candidate.first_move)
        {
            continue;
        }

        selected.push(candidate);
        if selected.len() == 2 {
            break;
        }
    }

    FoodModeOutput {
        candidates: selected,
    }
}

fn candidate_from_route(
    analysis: &StateAnalysis,
    route: &FoodRouteInfo,
    claim: Option<&FoodClaimInfo>,
    first_move: Direction,
    distance: u16,
) -> FoodCandidate {
    FoodCandidate {
        target_food: route.food,
        first_move,
        distance,
        claim_margin: claim.and_then(|value| value.claim_margin),
        contested: claim.is_some_and(|value| value.contested),
        certainty: analysis.certainty,
    }
}

fn candidate_rank(candidate: &FoodCandidate) -> (u8, u16, Reverse<i16>, u8, Coord) {
    let claim_class = match candidate.claim_margin {
        Some(margin) if margin < 0 => 2,
        Some(0) => 1,
        _ => 0,
    };
    let margin = candidate.claim_margin.unwrap_or(i16::MAX);

    (
        claim_class,
        candidate.distance,
        Reverse(margin),
        candidate.first_move.rank(),
        candidate.target_food,
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::analysis::StateAnalysis;
    use crate::direction::Direction;
    use crate::forecast::ForecastCertainty;
    use crate::{Battlesnake, Board, Coord, Game, GameState};

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
                id: "food-mode".to_string(),
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

    fn analyze(state: &GameState) -> StateAnalysis {
        StateAnalysis::from_state(state, ForecastCertainty::Deterministic)
    }

    #[test]
    fn returns_single_reachable_food_candidate() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        let food = Coord { x: 3, y: 1 };
        let state = state(ours, vec![], vec![food]);

        let output = candidates(&state, &analyze(&state));

        assert_eq!(output.candidates.len(), 1);
        assert_eq!(output.candidates[0].target_food, food);
        assert_eq!(output.candidates[0].first_move, Direction::Right);
        assert_eq!(output.candidates[0].distance, 2);
    }

    #[test]
    fn returns_at_most_two_candidates() {
        let ours = snake("ours", vec![Coord { x: 3, y: 3 }]);
        let foods = vec![
            Coord { x: 3, y: 5 },
            Coord { x: 5, y: 3 },
            Coord { x: 3, y: 1 },
        ];
        let state = state(ours, vec![], foods);

        let output = candidates(&state, &analyze(&state));

        assert!(output.candidates.len() <= 2);
    }

    #[test]
    fn prefers_distinct_first_moves() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }]);
        let foods = vec![
            Coord { x: 3, y: 1 },
            Coord { x: 4, y: 1 },
            Coord { x: 1, y: 3 },
        ];
        let state = state(ours, vec![], foods);

        let output = candidates(&state, &analyze(&state));
        let directions = output
            .candidates
            .iter()
            .map(|candidate| candidate.first_move)
            .collect::<Vec<_>>();

        assert_eq!(directions.len(), 2);
        assert_ne!(directions[0], directions[1]);
        assert!(directions.contains(&Direction::Up));
        assert!(directions.contains(&Direction::Right));
    }

    #[test]
    fn farther_claimable_food_can_beat_nearer_losing_food() {
        let ours = snake("ours", vec![Coord { x: 0, y: 0 }]);
        let enemy = snake("enemy", vec![Coord { x: 3, y: 0 }]);
        let losing_food = Coord { x: 2, y: 0 };
        let claimable_food = Coord { x: 0, y: 3 };
        let state = state(ours, vec![enemy], vec![losing_food, claimable_food]);

        let output = candidates(&state, &analyze(&state));

        assert_eq!(output.candidates[0].target_food, claimable_food);
        assert_eq!(output.candidates[0].first_move, Direction::Up);
        assert!(output.candidates[0].claim_margin.unwrap() > 0);
    }

    #[test]
    fn same_food_can_offer_multiple_shortest_first_moves_but_output_is_direction_deduplicated() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }]);
        let food = Coord { x: 2, y: 2 };
        let state = state(ours, vec![], vec![food]);

        let output = candidates(&state, &analyze(&state));
        let directions = output
            .candidates
            .iter()
            .map(|candidate| candidate.first_move)
            .collect::<Vec<_>>();

        assert_eq!(directions.len(), 2);
        assert_ne!(directions[0], directions[1]);
    }

    #[test]
    fn no_reachable_food_returns_empty_output() {
        let ours = snake("ours", vec![Coord { x: 0, y: 1 }]);
        let wall = snake(
            "wall",
            vec![
                Coord { x: 2, y: 0 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
                Coord { x: 2, y: 5 },
                Coord { x: 2, y: 6 },
            ],
        );
        let food = Coord { x: 4, y: 1 };
        let state = state(ours, vec![wall], vec![food]);

        let output = candidates(&state, &analyze(&state));

        assert!(output.candidates.is_empty());
    }

    #[test]
    fn deterministic_tie_break_is_stable() {
        let ours = snake("ours", vec![Coord { x: 3, y: 3 }]);
        let foods = vec![Coord { x: 3, y: 5 }, Coord { x: 5, y: 3 }];
        let state = state(ours, vec![], foods);

        let first = candidates(&state, &analyze(&state));
        let second = candidates(&state, &analyze(&state));

        assert_eq!(first.candidates, second.candidates);
    }

    #[test]
    fn food_candidate_contains_no_reward_or_utility_field() {
        let candidate = FoodCandidate {
            target_food: Coord { x: 1, y: 1 },
            first_move: Direction::Up,
            distance: 1,
            claim_margin: Some(2),
            contested: false,
            certainty: ForecastCertainty::Deterministic,
        };

        assert_eq!(candidate.distance, 1);
        assert_eq!(candidate.first_move, Direction::Up);
    }
}
