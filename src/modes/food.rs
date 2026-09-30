#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::analysis::routes::StateAnalysis;
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
