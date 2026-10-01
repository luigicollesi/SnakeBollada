use serde_json::Value;

use crate::{Battlesnake, Coord, GameState};

pub(crate) const DEFAULT_MAX_HEALTH: i32 = 100;
pub(crate) const BASE_AGGRESSION: f32 = 0.0;
pub(crate) const AGGRESSION_PER_FOOD: f32 = 0.10;
pub(crate) const MAX_AGGRESSION: f32 = 0.80;
pub(crate) const OPENING_FOOD_TARGET_FRUITS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AggressionState {
    pub(crate) fruits_eaten: u32,
    pub(crate) value: f32,
}

impl AggressionState {
    pub(crate) fn record_food(&mut self) {
        self.fruits_eaten = self.fruits_eaten.saturating_add(1);
        self.value =
            (BASE_AGGRESSION + self.fruits_eaten as f32 * AGGRESSION_PER_FOOD).min(MAX_AGGRESSION);
    }
}

impl Default for AggressionState {
    fn default() -> Self {
        Self {
            fruits_eaten: 0,
            value: BASE_AGGRESSION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SimulationSupport {
    StandardLike,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct RulesContext {
    pub(crate) name: String,
    pub(crate) max_health: i32,
    pub(crate) hazard_damage_per_turn: i32,
}

impl RulesContext {
    pub(crate) fn simulation_support(&self) -> SimulationSupport {
        match self.name.as_str() {
            "standard" => SimulationSupport::StandardLike,
            _ => SimulationSupport::Unsupported,
        }
    }

    fn from_state(state: &GameState) -> Self {
        let name = state
            .game
            .ruleset
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("standard")
            .to_string();

        let settings = state
            .game
            .ruleset
            .get("settings")
            .and_then(Value::as_object);
        let hazard_damage_per_turn = settings
            .and_then(|settings| settings.get("hazardDamagePerTurn"))
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0);

        Self {
            name,
            max_health: DEFAULT_MAX_HEALTH,
            hazard_damage_per_turn,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SimulatedSnake {
    pub(crate) id: String,
    pub(crate) health: i32,
    pub(crate) body: Vec<Coord>,
    pub(crate) alive: bool,
}

impl SimulatedSnake {
    pub(crate) fn head(&self) -> Option<Coord> {
        self.body.first().copied()
    }

    pub(crate) fn length(&self) -> usize {
        self.body.len()
    }
}

impl From<&Battlesnake> for SimulatedSnake {
    fn from(snake: &Battlesnake) -> Self {
        Self {
            id: snake.id.clone(),
            health: snake.health,
            body: snake.body.clone(),
            alive: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SimulatedGameState {
    pub(crate) turn: i32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) food: Vec<Coord>,
    pub(crate) hazards: Vec<Coord>,
    pub(crate) snakes: Vec<SimulatedSnake>,
    pub(crate) our_snake_id: String,
    pub(crate) rules: RulesContext,
    pub(crate) aggression: AggressionState,
}

impl SimulatedGameState {
    pub(crate) fn snake(&self, id: &str) -> Option<&SimulatedSnake> {
        self.snakes.iter().find(|snake| snake.id == id)
    }

    pub(crate) fn snake_mut(&mut self, id: &str) -> Option<&mut SimulatedSnake> {
        self.snakes.iter_mut().find(|snake| snake.id == id)
    }
}

impl From<&GameState> for SimulatedGameState {
    fn from(state: &GameState) -> Self {
        Self {
            turn: state.turn,
            width: state.board.width,
            height: state.board.height,
            food: state.board.food.clone(),
            hazards: state.board.hazards.clone(),
            snakes: state
                .board
                .snakes
                .iter()
                .map(SimulatedSnake::from)
                .collect(),
            our_snake_id: state.you.id.clone(),
            rules: RulesContext::from_state(state),
            aggression: AggressionState::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Board, Game};

    fn snake(id: &str, health: i32, body: Vec<Coord>) -> Battlesnake {
        Battlesnake {
            id: id.to_string(),
            name: id.to_string(),
            health,
            head: body[0],
            length: body.len() as u32,
            body,
            latency: String::new(),
            shout: None,
        }
    }

    #[test]
    fn aggression_increases_per_food_and_clamps() {
        let mut aggression = AggressionState::default();
        assert_eq!(aggression.value, BASE_AGGRESSION);

        aggression.record_food();
        assert_eq!(aggression.fruits_eaten, 1);
        assert!((aggression.value - 0.10).abs() < f32::EPSILON);

        for _ in 0..20 {
            aggression.record_food();
        }
        assert_eq!(aggression.value, MAX_AGGRESSION);
    }

    #[test]
    fn ruleset_support_is_explicit() {
        let standard = RulesContext {
            name: "standard".to_string(),
            max_health: 100,
            hazard_damage_per_turn: 0,
        };
        let constrictor = RulesContext {
            name: "constrictor".to_string(),
            max_health: 100,
            hazard_damage_per_turn: 0,
        };

        assert_eq!(
            standard.simulation_support(),
            SimulationSupport::StandardLike
        );
        assert_eq!(
            constrictor.simulation_support(),
            SimulationSupport::Unsupported
        );
    }

    #[test]
    fn converts_api_state_without_strategy_fields() {
        let ours = snake("ours", 87, vec![Coord { x: 2, y: 2 }, Coord { x: 2, y: 1 }]);
        let state = GameState {
            game: Game {
                id: "game".to_string(),
                ruleset: HashMap::from([
                    ("name".to_string(), json!("royale")),
                    ("settings".to_string(), json!({ "hazardDamagePerTurn": 14 })),
                ]),
                timeout: 500,
            },
            turn: 9,
            board: Board {
                height: 11,
                width: 11,
                food: vec![Coord { x: 4, y: 4 }],
                snakes: vec![ours.clone()],
                hazards: vec![Coord { x: 5, y: 5 }],
            },
            you: ours,
        };

        let simulated = SimulatedGameState::from(&state);

        assert_eq!(simulated.turn, 9);
        assert_eq!(simulated.rules.name, "royale");
        assert_eq!(simulated.rules.hazard_damage_per_turn, 14);
        assert_eq!(simulated.snake("ours").unwrap().health, 87);
        assert_eq!(simulated.aggression, AggressionState::default());
    }
}
