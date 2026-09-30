#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{HashMap, VecDeque};

use crate::board_mask::BoardMask;
use crate::direction::{Direction, MoveMask};
use crate::forecast::ForecastCertainty;
use crate::simulation::state::SimulatedGameState;
use crate::{Coord, GameState};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoodRouteInfo {
    pub(crate) food: Coord,
    pub(crate) reachable: bool,
    pub(crate) distance: Option<u16>,
    pub(crate) first_moves: MoveMask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoodClaimInfo {
    pub(crate) food: Coord,
    pub(crate) our_eta: Option<u16>,
    pub(crate) nearest_enemy_eta: Option<u16>,
    pub(crate) nearest_enemy: Option<String>,
    pub(crate) claim_margin: Option<i16>,
    pub(crate) contested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoodCompetitorInfo {
    pub(crate) snake_id: String,
    pub(crate) eta: u16,
}

#[derive(Debug, Clone)]
struct AnalysisSnake {
    id: String,
    head: Coord,
    body: Vec<Coord>,
}

#[derive(Debug, Clone)]
struct AnalysisState {
    width: u16,
    height: u16,
    food: Vec<Coord>,
    snakes: Vec<AnalysisSnake>,
    our_snake_id: String,
}

impl AnalysisState {
    fn from_game_state(state: &GameState) -> Self {
        Self {
            width: state.board.width as u16,
            height: state.board.height as u16,
            food: state.board.food.clone(),
            snakes: state
                .board
                .snakes
                .iter()
                .map(|snake| AnalysisSnake {
                    id: snake.id.clone(),
                    head: snake.head,
                    body: snake.body.clone(),
                })
                .collect(),
            our_snake_id: state.you.id.clone(),
        }
    }

    fn from_simulated(state: &SimulatedGameState) -> Self {
        Self {
            width: state.width as u16,
            height: state.height as u16,
            food: state.food.clone(),
            snakes: state
                .snakes
                .iter()
                .filter(|snake| snake.alive)
                .map(|snake| AnalysisSnake {
                    id: snake.id.clone(),
                    head: snake
                        .head()
                        .expect("alive simulated snake must have a body"),
                    body: snake.body.clone(),
                })
                .collect(),
            our_snake_id: state.our_snake_id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct SnakeRouteField {
    width: u16,
    height: u16,
    distance: Vec<u16>,
    first_moves: Vec<MoveMask>,
}

impl SnakeRouteField {
    fn index(&self, coord: Coord) -> Option<usize> {
        if coord.x < 0
            || coord.y < 0
            || coord.x >= i32::from(self.width)
            || coord.y >= i32::from(self.height)
        {
            return None;
        }

        Some(coord.y as usize * usize::from(self.width) + coord.x as usize)
    }

    fn route_to(&self, food: Coord) -> FoodRouteInfo {
        let Some(index) = self.index(food) else {
            return FoodRouteInfo {
                food,
                reachable: false,
                distance: None,
                first_moves: MoveMask::empty(),
            };
        };

        let distance = self.distance[index];
        let reachable = distance != u16::MAX;

        FoodRouteInfo {
            food,
            reachable,
            distance: reachable.then_some(distance),
            first_moves: if reachable {
                self.first_moves[index]
            } else {
                MoveMask::empty()
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct StateAnalysis {
    pub(crate) certainty: ForecastCertainty,
    routes: HashMap<String, HashMap<Coord, FoodRouteInfo>>,
    claims: HashMap<Coord, FoodClaimInfo>,
}

impl StateAnalysis {
    pub(crate) fn from_state(state: &GameState, certainty: ForecastCertainty) -> Self {
        Self::from_analysis_state(&AnalysisState::from_game_state(state), certainty)
    }

    pub(crate) fn from_simulated(
        state: &SimulatedGameState,
        certainty: ForecastCertainty,
    ) -> Self {
        Self::from_analysis_state(&AnalysisState::from_simulated(state), certainty)
    }

    fn from_analysis_state(state: &AnalysisState, certainty: ForecastCertainty) -> Self {
        let mut routes = HashMap::new();

        for snake in &state.snakes {
            let field = build_route_field(state, &snake.id);
            let food_routes = state
                .food
                .iter()
                .copied()
                .map(|food| (food, field.route_to(food)))
                .collect::<HashMap<_, _>>();
            routes.insert(snake.id.clone(), food_routes);
        }

        let claims = state
            .food
            .iter()
            .copied()
            .map(|food| (food, derive_food_claim(state, &routes, food)))
            .collect();

        Self {
            certainty,
            routes,
            claims,
        }
    }

    pub(crate) fn route_for(&self, snake_id: &str, food: Coord) -> Option<&FoodRouteInfo> {
        self.routes.get(snake_id)?.get(&food)
    }

    pub(crate) fn claim_for(&self, food: Coord) -> Option<&FoodClaimInfo> {
        self.claims.get(&food)
    }

    pub(crate) fn nearest_competitor_for(
        &self,
        snake_id: &str,
        food: Coord,
    ) -> Option<FoodCompetitorInfo> {
        self.routes
            .iter()
            .filter(|(candidate_id, _)| candidate_id.as_str() != snake_id)
            .filter_map(|(candidate_id, foods)| {
                foods
                    .get(&food)
                    .and_then(|route| route.distance)
                    .map(|eta| FoodCompetitorInfo {
                        snake_id: candidate_id.clone(),
                        eta,
                    })
            })
            .min_by(|left, right| {
                left.eta
                    .cmp(&right.eta)
                    .then_with(|| left.snake_id.cmp(&right.snake_id))
            })
    }
}

fn build_route_field(state: &AnalysisState, snake_id: &str) -> SnakeRouteField {
    let width = state.width;
    let height = state.height;
    let cells = usize::from(width) * usize::from(height);
    let mut occupied = BoardMask::new(width, height);

    for snake in &state.snakes {
        for coord in &snake.body {
            occupied.set(*coord, true);
        }
    }

    let source = state
        .snakes
        .iter()
        .find(|snake| snake.id == snake_id)
        .expect("route source snake must exist on the board");

    occupied.set(source.head, false);

    if source.body.len() >= 2 {
        let tail = source.body[source.body.len() - 1];
        let before_tail = source.body[source.body.len() - 2];
        if tail != before_tail {
            occupied.set(tail, false);
        }
    }

    let mut field = SnakeRouteField {
        width,
        height,
        distance: vec![u16::MAX; cells],
        first_moves: vec![MoveMask::empty(); cells],
    };

    let start_index = field
        .index(source.head)
        .expect("snake head must be in bounds");
    field.distance[start_index] = 0;

    let mut queue = VecDeque::new();
    queue.push_back(source.head);

    while let Some(current) = queue.pop_front() {
        let current_index = field
            .index(current)
            .expect("queued route coordinate must be in bounds");
        let next_distance = field.distance[current_index].saturating_add(1);

        for direction in Direction::ALL {
            let next = direction.apply(current);
            let Some(next_index) = field.index(next) else {
                continue;
            };

            if occupied.contains(next) {
                continue;
            }

            let candidate_first_moves = if current == source.head {
                MoveMask::single(direction)
            } else {
                field.first_moves[current_index]
            };

            if next_distance < field.distance[next_index] {
                field.distance[next_index] = next_distance;
                field.first_moves[next_index] = candidate_first_moves;
                queue.push_back(next);
            } else if next_distance == field.distance[next_index]
                && field.first_moves[next_index].union_with(candidate_first_moves)
            {
                queue.push_back(next);
            }
        }
    }

    field
}

fn derive_food_claim(
    state: &AnalysisState,
    routes: &HashMap<String, HashMap<Coord, FoodRouteInfo>>,
    food: Coord,
) -> FoodClaimInfo {
    let our_eta = routes
        .get(&state.our_snake_id)
        .and_then(|foods| foods.get(&food))
        .and_then(|route| route.distance);

    let nearest_enemy = state
        .snakes
        .iter()
        .filter(|snake| snake.id != state.our_snake_id)
        .filter_map(|snake| {
            routes
                .get(&snake.id)
                .and_then(|foods| foods.get(&food))
                .and_then(|route| route.distance)
                .map(|distance| (distance, snake.id.as_str()))
        })
        .min_by(|(left_distance, left_id), (right_distance, right_id)| {
            left_distance
                .cmp(right_distance)
                .then_with(|| left_id.cmp(right_id))
        });

    let (nearest_enemy_eta, nearest_enemy_id) = nearest_enemy
        .map(|(distance, id)| (Some(distance), Some(id.to_string())))
        .unwrap_or((None, None));

    let claim_margin = match (our_eta, nearest_enemy_eta) {
        (Some(ours), Some(enemy)) => {
            let margin = i32::from(enemy) - i32::from(ours);
            Some(margin.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16)
        }
        _ => None,
    };

    FoodClaimInfo {
        food,
        our_eta,
        nearest_enemy_eta,
        nearest_enemy: nearest_enemy_id,
        claim_margin,
        contested: matches!((our_eta, nearest_enemy_eta), (Some(ours), Some(enemy)) if ours == enemy),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::simulation::state::SimulatedGameState;
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
    fn certainty_is_preserved() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }]);
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![], vec![]),
            ForecastCertainty::FoodProvisional,
        );

        assert_eq!(analysis.certainty, ForecastCertainty::FoodProvisional);
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
    fn simulated_state_uses_same_route_engine() {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        let food = Coord { x: 3, y: 1 };
        let api_state = state(ours, vec![], vec![food]);
        let simulated = SimulatedGameState::from(&api_state);

        let api_analysis =
            StateAnalysis::from_state(&api_state, ForecastCertainty::Deterministic);
        let simulated_analysis =
            StateAnalysis::from_simulated(&simulated, ForecastCertainty::Deterministic);

        assert_eq!(
            api_analysis.route_for("ours", food),
            simulated_analysis.route_for("ours", food)
        );
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

    #[test]
    fn claim_uses_nearest_enemy_eta() {
        let ours = snake("ours", vec![Coord { x: 0, y: 0 }]);
        let enemy_a = snake("enemy-a", vec![Coord { x: 4, y: 0 }]);
        let enemy_b = snake("enemy-b", vec![Coord { x: 4, y: 4 }]);
        let food = Coord { x: 2, y: 0 };
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![enemy_a, enemy_b], vec![food]),
            ForecastCertainty::Deterministic,
        );
        let claim = analysis.claim_for(food).unwrap();

        assert_eq!(claim.our_eta, Some(2));
        assert_eq!(claim.nearest_enemy_eta, Some(2));
        assert_eq!(claim.nearest_enemy.as_deref(), Some("enemy-a"));
        assert_eq!(claim.claim_margin, Some(0));
    }

    #[test]
    fn competitor_lookup_is_relative_to_requested_snake() {
        let ours = snake("ours", vec![Coord { x: 0, y: 0 }]);
        let enemy_a = snake("enemy-a", vec![Coord { x: 4, y: 0 }]);
        let enemy_b = snake("enemy-b", vec![Coord { x: 4, y: 4 }]);
        let food = Coord { x: 2, y: 0 };
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![enemy_a, enemy_b], vec![food]),
            ForecastCertainty::Deterministic,
        );

        let competitor = analysis
            .nearest_competitor_for("enemy-a", food)
            .expect("enemy-a should have a competitor");

        assert_eq!(competitor.snake_id, "ours");
        assert_eq!(competitor.eta, 2);
    }

    #[test]
    fn equal_eta_marks_food_contested() {
        let ours = snake("ours", vec![Coord { x: 0, y: 0 }]);
        let enemy = snake("enemy", vec![Coord { x: 4, y: 0 }]);
        let food = Coord { x: 2, y: 0 };
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![enemy], vec![food]),
            ForecastCertainty::Deterministic,
        );

        assert!(analysis.claim_for(food).unwrap().contested);
    }

    #[test]
    fn enemy_unreachable_does_not_mark_food_contested() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 0, y: 1 },
                Coord { x: 2, y: 0 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
                Coord { x: 2, y: 4 },
            ],
        );
        let enemy = snake("enemy", vec![Coord { x: 4, y: 1 }]);
        let food = Coord { x: 1, y: 1 };
        let analysis = StateAnalysis::from_state(
            &state(ours, vec![enemy], vec![food]),
            ForecastCertainty::Deterministic,
        );
        let claim = analysis.claim_for(food).unwrap();

        assert_eq!(claim.our_eta, Some(1));
        assert_eq!(claim.nearest_enemy_eta, None);
        assert_eq!(claim.claim_margin, None);
        assert!(!claim.contested);
    }
}
