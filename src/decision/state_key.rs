use crate::simulation::state::{RulesContext, SimulatedGameState};
use crate::Coord;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SnakeStateKey {
    id: String,
    health: i32,
    body: Vec<Coord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct StateKey {
    width: u32,
    height: u32,
    snakes: Vec<SnakeStateKey>,
    food: Vec<Coord>,
    hazards: Vec<Coord>,
    rules: RulesContext,
}

impl StateKey {
    pub(crate) fn from_beam_state(state: &SimulatedGameState) -> Self {
        let mut snakes = state
            .snakes
            .iter()
            .filter(|snake| snake.alive)
            .map(|snake| SnakeStateKey {
                id: snake.id.clone(),
                health: snake.health,
                body: snake.body.clone(),
            })
            .collect::<Vec<_>>();
        snakes.sort_by(|left, right| left.id.cmp(&right.id));

        let mut food = state.food.clone();
        food.sort_unstable();

        let mut hazards = state.hazards.clone();
        hazards.sort_unstable();

        Self {
            width: state.width,
            height: state.height,
            snakes,
            food,
            hazards,
            rules: state.rules.clone(),
        }
    }

    pub(crate) fn food(&self) -> &[Coord] {
        &self.food
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{SimulatedSnake};

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
        }
    }

    #[test]
    fn key_is_independent_of_collection_order() {
        let left = sample_state();
        let mut right = sample_state();
        right.food.reverse();
        right.hazards.reverse();
        right.snakes.reverse();

        assert_eq!(
            StateKey::from_beam_state(&left),
            StateKey::from_beam_state(&right)
        );
    }

    #[test]
    fn dead_snakes_do_not_change_future_key() {
        let left = sample_state();
        let mut right = sample_state();
        right.snakes.push(SimulatedSnake {
            id: "dead".to_string(),
            health: 0,
            body: vec![Coord { x: 2, y: 2 }],
            alive: false,
        });

        assert_eq!(
            StateKey::from_beam_state(&left),
            StateKey::from_beam_state(&right)
        );
    }

    #[test]
    fn key_changes_when_future_relevant_state_changes() {
        let left = sample_state();
        let mut right = sample_state();
        right.snakes[0].health -= 1;

        assert_ne!(
            StateKey::from_beam_state(&left),
            StateKey::from_beam_state(&right)
        );
    }

    #[test]
    fn beam_key_ignores_turn() {
        let left = sample_state();
        let mut right = sample_state();
        right.turn = right.turn.saturating_add(9);

        assert_eq!(
            StateKey::from_beam_state(&left),
            StateKey::from_beam_state(&right)
        );
    }
}
