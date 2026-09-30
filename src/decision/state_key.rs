use crate::simulation::state::{RulesContext, SimulatedGameState};
use crate::Coord;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SnakeStateKey {
    id: String,
    health: i32,
    body: Vec<Coord>,
    alive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StateKey {
    turn: i32,
    width: u32,
    height: u32,
    snakes: Vec<SnakeStateKey>,
    food: Vec<Coord>,
    hazards: Vec<Coord>,
    rules: RulesContext,
    aggression_milli: u16,
}

impl StateKey {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Self {
        let mut snakes = state
            .snakes
            .iter()
            .map(|snake| SnakeStateKey {
                id: snake.id.clone(),
                health: snake.health,
                body: snake.body.clone(),
                alive: snake.alive,
            })
            .collect::<Vec<_>>();
        snakes.sort_by(|left, right| left.id.cmp(&right.id));

        let mut food = state.food.clone();
        food.sort_unstable();

        let mut hazards = state.hazards.clone();
        hazards.sort_unstable();

        Self {
            turn: state.turn,
            width: state.width,
            height: state.height,
            snakes,
            food,
            hazards,
            rules: state.rules.clone(),
            aggression_milli: aggression_bucket(state.aggression.value),
        }
    }
}

fn aggression_bucket(value: f32) -> u16 {
    if !value.is_finite() {
        return 0;
    }

    (value.clamp(0.0, 1.0) * 1000.0).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{AggressionState, SimulatedSnake};

    fn sample_state() -> SimulatedGameState {
        SimulatedGameState {
            turn: 7,
            width: 5,
            height: 5,
            food: vec![Coord { x: 4, y: 4 }, Coord { x: 0, y: 0 }],
            hazards: vec![Coord { x: 3, y: 3 }, Coord { x: 1, y: 1 }],
            snakes: vec![
                SimulatedSnake {
                    id: "b".to_string(),
                    health: 90,
                    body: vec![Coord { x: 4, y: 2 }],
                    alive: true,
                },
                SimulatedSnake {
                    id: "a".to_string(),
                    health: 80,
                    body: vec![Coord { x: 0, y: 2 }],
                    alive: true,
                },
            ],
            our_snake_id: "a".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState {
                fruits_eaten: 2,
                value: 0.375,
            },
        }
    }

    #[test]
    fn key_is_independent_of_collection_order() {
        let left = sample_state();
        let mut right = sample_state();
        right.food.reverse();
        right.hazards.reverse();
        right.snakes.reverse();

        assert_eq!(StateKey::from_state(&left), StateKey::from_state(&right));
    }

    #[test]
    fn key_changes_when_future_relevant_state_changes() {
        let left = sample_state();
        let mut right = sample_state();
        right.snakes[0].health -= 1;

        assert_ne!(StateKey::from_state(&left), StateKey::from_state(&right));
    }

    #[test]
    fn aggression_is_normalized_for_hashing() {
        let mut left = sample_state();
        let mut right = sample_state();
        left.aggression.value = 0.3751;
        right.aggression.value = 0.3754;

        assert_eq!(StateKey::from_state(&left), StateKey::from_state(&right));
    }
}
