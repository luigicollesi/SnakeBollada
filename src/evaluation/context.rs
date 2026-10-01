use crate::simulation::state::SimulatedGameState;

const MILLI: u32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActorContext {
    pub(crate) size_advantage_milli: u16,
    pub(crate) size_disadvantage_milli: u16,
}

impl ActorContext {
    pub(crate) fn from_state(state: &SimulatedGameState, actor_id: &str) -> Option<Self> {
        let actor = state.snake(actor_id).filter(|snake| snake.alive)?;
        let actor_length = u32::try_from(actor.length()).unwrap_or(u32::MAX);
        let largest_enemy = state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != actor_id)
            .map(|snake| u32::try_from(snake.length()).unwrap_or(u32::MAX))
            .max()
            .unwrap_or(0);
        let (size_advantage_milli, size_disadvantage_milli) =
            relative_size_pressure(actor_length, largest_enemy);

        Some(Self {
            size_advantage_milli,
            size_disadvantage_milli,
        })
    }
}

fn relative_size_pressure(actor_length: u32, largest_enemy: u32) -> (u16, u16) {
    if largest_enemy == 0 {
        return (1000, 0);
    }

    if actor_length >= largest_enemy {
        let lead = actor_length.saturating_sub(largest_enemy);
        (ratio_milli(lead, largest_enemy.max(1)), 0)
    } else {
        let deficit = largest_enemy.saturating_sub(actor_length);
        (0, ratio_milli(deficit, actor_length.max(1)))
    }
}

fn ratio_milli(numerator: u32, denominator: u32) -> u16 {
    if denominator == 0 {
        return 0;
    }

    numerator
        .saturating_mul(MILLI)
        .saturating_div(denominator)
        .min(MILLI)
        .try_into()
        .unwrap_or(1000)
}

#[cfg(test)]
mod tests {
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedGameState, SimulatedSnake,
    };
    use crate::Coord;

    use super::*;

    fn snake(id: &str, length: usize) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: (0..length)
                .map(|index| Coord {
                    x: i32::try_from(index % 11).unwrap_or(0),
                    y: i32::try_from(index / 11).unwrap_or(0),
                })
                .collect(),
            alive: true,
        }
    }

    fn state(ours: SimulatedSnake, enemies: Vec<SimulatedSnake>) -> SimulatedGameState {
        let mut snakes = vec![ours];
        snakes.extend(enemies);
        SimulatedGameState {
            turn: 1,
            width: 11,
            height: 11,
            food: vec![],
            hazards: vec![],
            snakes,
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        }
    }

    #[test]
    fn context_is_actor_relative() {
        let state = state(
            snake("ours", 8),
            vec![snake("enemy-a", 12), snake("enemy-b", 5)],
        );

        let ours = ActorContext::from_state(&state, "ours").unwrap();
        let enemy = ActorContext::from_state(&state, "enemy-a").unwrap();

        assert!(ours.size_disadvantage_milli > 0);
        assert_eq!(ours.size_advantage_milli, 0);
        assert_eq!(enemy.size_disadvantage_milli, 0);
        assert!(enemy.size_advantage_milli > 0);
    }

    #[test]
    fn no_enemy_is_full_size_advantage() {
        let state = state(snake("ours", 8), vec![]);
        let ours = ActorContext::from_state(&state, "ours").unwrap();

        assert_eq!(ours.size_advantage_milli, 1000);
        assert_eq!(ours.size_disadvantage_milli, 0);
    }
}
