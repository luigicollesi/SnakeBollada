#![cfg_attr(not(test), allow(dead_code))]

use std::cmp::Reverse;

use crate::analysis::{FoodClaimInfo, FoodRouteInfo, StateAnalysis};
use crate::direction::Direction;
use crate::forecast::ForecastCertainty;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

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

pub(crate) fn candidates(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    certainty: ForecastCertainty,
) -> FoodModeOutput {
    candidates_for_actor(state, analysis, &state.our_snake_id, certainty)
}

pub(crate) fn candidates_for_actor(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    actor_id: &str,
    certainty: ForecastCertainty,
) -> FoodModeOutput {
    let mut expanded = Vec::new();

    for food in state.food.iter().copied() {
        let Some(route) = analysis.route_for(actor_id, food) else {
            continue;
        };
        if !route.reachable {
            continue;
        }

        let Some(distance) = route.distance else {
            continue;
        };
        let claim = analysis.claim_for_actor(actor_id, food);

        for first_move in route.first_moves.iter() {
            expanded.push(candidate_from_route(
                route,
                claim.as_ref(),
                first_move,
                distance,
                certainty,
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

pub(crate) fn candidates_for_target(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    target_food: Coord,
    certainty: ForecastCertainty,
) -> Vec<FoodCandidate> {
    candidates_for_target_actor(state, analysis, &state.our_snake_id, target_food, certainty)
}

pub(crate) fn candidates_for_target_actor(
    _state: &SimulatedGameState,
    analysis: &StateAnalysis,
    actor_id: &str,
    target_food: Coord,
    certainty: ForecastCertainty,
) -> Vec<FoodCandidate> {
    let Some(route) = analysis.route_for(actor_id, target_food) else {
        return Vec::new();
    };
    if !route.reachable {
        return Vec::new();
    }

    let Some(distance) = route.distance else {
        return Vec::new();
    };
    let claim = analysis.claim_for_actor(actor_id, target_food);

    let mut candidates = route
        .first_moves
        .iter()
        .map(|first_move| {
            candidate_from_route(route, claim.as_ref(), first_move, distance, certainty)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(candidate_rank);
    candidates
}

fn candidate_from_route(
    route: &FoodRouteInfo,
    claim: Option<&FoodClaimInfo>,
    first_move: Direction,
    distance: u16,
    certainty: ForecastCertainty,
) -> FoodCandidate {
    FoodCandidate {
        target_food: route.food,
        first_move,
        distance,
        claim_margin: claim.and_then(|value| value.claim_margin),
        contested: claim.is_some_and(|value| value.contested),
        certainty,
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
    use crate::simulation::state::SimulatedGameState;
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
        StateAnalysis::from_state(state)
    }

    fn candidates_for(state: &GameState) -> FoodModeOutput {
        let simulated = SimulatedGameState::from(state);
        candidates(
            &simulated,
            &analyze(state),
            ForecastCertainty::Deterministic,
        )
    }

    #[test]
    fn returns_single_reachable_food_candidate() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        let food = Coord { x: 3, y: 1 };
        let state = state(ours, vec![], vec![food]);

        let output = candidates_for(&state);

        assert_eq!(output.candidates.len(), 1);
        assert_eq!(output.candidates[0].target_food, food);
        assert_eq!(output.candidates[0].first_move, Direction::Right);
        assert_eq!(output.candidates[0].distance, 2);
    }

    #[test]
    fn target_candidates_keep_all_shortest_first_moves_for_committed_food() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }]);
        let target = Coord { x: 2, y: 2 };
        let state = state(ours, vec![], vec![target]);
        let simulated = SimulatedGameState::from(&state);
        let analysis = analyze(&state);

        let candidates = candidates_for_target(
            &simulated,
            &analysis,
            target,
            ForecastCertainty::Deterministic,
        );

        assert_eq!(candidates.len(), 2);
        assert!(candidates
            .iter()
            .all(|candidate| candidate.target_food == target));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.first_move == Direction::Up));
        assert!(candidates
            .iter()
            .any(|candidate| candidate.first_move == Direction::Right));
    }

    #[test]
    fn actor_candidates_use_the_requested_snake_perspective() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }]);
        let enemy = snake("enemy", vec![Coord { x: 5, y: 5 }]);
        let food = Coord { x: 5, y: 3 };
        let state = state(ours, vec![enemy], vec![food]);
        let simulated = SimulatedGameState::from(&state);
        let analysis = analyze(&state);

        let output = candidates_for_actor(
            &simulated,
            &analysis,
            "enemy",
            ForecastCertainty::Deterministic,
        );

        assert!(output
            .candidates
            .iter()
            .any(|candidate| candidate.first_move == Direction::Down));
        assert!(output
            .candidates
            .iter()
            .all(|candidate| candidate.target_food == food));
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

        let output = candidates_for(&state);

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

        let output = candidates_for(&state);
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

        let output = candidates_for(&state);

        assert_eq!(output.candidates[0].target_food, claimable_food);
        assert_eq!(output.candidates[0].first_move, Direction::Up);
        assert!(output.candidates[0].claim_margin.unwrap() > 0);
    }

    #[test]
    fn same_food_can_offer_multiple_shortest_first_moves_but_output_is_direction_deduplicated() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }]);
        let food = Coord { x: 2, y: 2 };
        let state = state(ours, vec![], vec![food]);

        let output = candidates_for(&state);
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

        let output = candidates_for(&state);

        assert!(output.candidates.is_empty());
    }

    #[test]
    fn deterministic_tie_break_is_stable() {
        let ours = snake("ours", vec![Coord { x: 3, y: 3 }]);
        let foods = vec![Coord { x: 3, y: 5 }, Coord { x: 5, y: 3 }];
        let state = state(ours, vec![], foods);

        let first = candidates_for(&state);
        let second = candidates_for(&state);

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
