use serde_json::Value;

use crate::simulation::state::SimulatedGameState;
use crate::GameState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum ForecastCertainty {
    #[default]
    Deterministic,
    FoodProvisional,
}

impl ForecastCertainty {
    pub(crate) const fn after(self, delta: ForecastDelta) -> Self {
        match (self, delta) {
            (Self::FoodProvisional, _) | (_, ForecastDelta::FoodUncertainty) => {
                Self::FoodProvisional
            }
            _ => Self::Deterministic,
        }
    }

    pub(crate) const fn is_provisional(self) -> bool {
        matches!(self, Self::FoodProvisional)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum ForecastDelta {
    #[default]
    Deterministic,
    FoodUncertainty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FoodForecastPolicy {
    pub(crate) spawn_chance_percent: u16,
    pub(crate) minimum_food: u16,
}

impl FoodForecastPolicy {
    pub(crate) fn from_game_state(state: &GameState) -> Self {
        let settings = state
            .game
            .ruleset
            .get("settings")
            .and_then(Value::as_object);

        let spawn_chance_percent = settings
            .and_then(|settings| settings.get("foodSpawnChance"))
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0)
            .min(100);

        let minimum_food = settings
            .and_then(|settings| settings.get("minimumFood"))
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0);

        Self {
            spawn_chance_percent,
            minimum_food,
        }
    }

    pub(crate) fn delta_after(&self, child: &SimulatedGameState) -> ForecastDelta {
        let random_spawn_possible = self.spawn_chance_percent > 0;
        let forced_spawn_possible = child.food.len() < usize::from(self.minimum_food);

        if random_spawn_possible || forced_spawn_possible {
            ForecastDelta::FoodUncertainty
        } else {
            ForecastDelta::Deterministic
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};

    fn state(settings: serde_json::Value) -> GameState {
        let ours = Battlesnake {
            id: "ours".to_string(),
            name: "ours".to_string(),
            health: 100,
            head: Coord { x: 1, y: 1 },
            length: 1,
            body: vec![Coord { x: 1, y: 1 }],
            latency: String::new(),
            shout: None,
        };

        GameState {
            game: Game {
                id: "forecast".to_string(),
                ruleset: HashMap::from([
                    ("name".to_string(), json!("standard")),
                    ("settings".to_string(), settings),
                ]),
                timeout: 500,
            },
            turn: 0,
            board: Board {
                width: 7,
                height: 7,
                food: vec![],
                snakes: vec![ours.clone()],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn parses_standard_food_forecast_settings() {
        let state = state(json!({
            "foodSpawnChance": 15,
            "minimumFood": 1
        }));

        let policy = FoodForecastPolicy::from_game_state(&state);

        assert_eq!(policy.spawn_chance_percent, 15);
        assert_eq!(policy.minimum_food, 1);
    }

    #[test]
    fn certainty_is_monotonic_after_food_uncertainty() {
        assert_eq!(
            ForecastCertainty::Deterministic.after(ForecastDelta::FoodUncertainty),
            ForecastCertainty::FoodProvisional
        );
        assert_eq!(
            ForecastCertainty::FoodProvisional.after(ForecastDelta::Deterministic),
            ForecastCertainty::FoodProvisional
        );
    }
}
