#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::direction::{Direction, MoveMask};
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
                id: "analysis".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 1,
            board: Board {
                height: 5,
                width: 5,
                food,
                snakes,
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn one_route_field_serves_multiple_foods() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        let f1 = Coord { x: 3, y: 1 };
        let f2 = Coord { x: 1, y: 3 };
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![], vec![f1, f2]),
            ForecastCertainty::Deterministic,
        );

        assert_eq!(analysis.route_for("ours", f1).unwrap().distance, Some(2));
        assert_eq!(analysis.route_for("ours", f2).unwrap().distance, Some(2));
    }

    #[test]
    fn equal_shortest_paths_preserve_all_first_moves() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        let food = Coord { x: 2, y: 2 };
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![], vec![food]),
            ForecastCertainty::Deterministic,
        );
        let route = analysis.route_for("ours", food).unwrap();

        let mut expected = MoveMask::empty();
        expected.insert(Direction::Up);
        expected.insert(Direction::Right);

        assert_eq!(route.first_moves, expected);
    }

    #[test]
    fn blocked_near_food_is_not_reported_as_reachable() {
        let ours = snake("ours", vec![Coord { x: 0, y: 1 }, Coord { x: 0, y: 0 }]);
        let wall = snake(
            "wall",
            vec![
                Coord { x: 2, y: 0 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
            ],
        );
        let food = Coord { x: 4, y: 1 };
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![wall], vec![food]),
            ForecastCertainty::Deterministic,
        );

        let route = analysis.route_for("ours", food).unwrap();
        assert!(!route.reachable);
        assert_eq!(route.distance, None);
    }

    #[test]
    fn unique_source_tail_can_be_used_but_stacked_tail_cannot() {
        let food = Coord { x: 1, y: 2 };
        let unique = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                Coord { x: 1, y: 1 },
                food,
            ],
        );
        let analysis = StateAnalysis::from_state(
            &state(unique, vec![], vec![food]),
            ForecastCertainty::Deterministic,
        );
        assert_eq!(analysis.route_for("ours", food).unwrap().distance, Some(1));

        let stacked_food = Coord { x: 1, y: 1 };
        let stacked = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                stacked_food,
                stacked_food,
            ],
        );
        let analysis = StateAnalysis::from_state(
            &state(stacked, vec![], vec![stacked_food]),
            ForecastCertainty::Deterministic,
        );
        assert!(!analysis.route_for("ours", stacked_food).unwrap().reachable);
    }

    #[test]
    fn opponent_tail_remains_blocked() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        let food = Coord { x: 2, y: 1 };
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 4, y: 4 },
                Coord { x: 4, y: 3 },
                Coord { x: 3, y: 2 },
                food,
            ],
        );
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![enemy], vec![food]),
            ForecastCertainty::Deterministic,
        );

        assert!(!analysis.route_for("ours", food).unwrap().reachable);
    }
}
