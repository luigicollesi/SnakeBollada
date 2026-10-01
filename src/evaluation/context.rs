use std::collections::HashSet;

use crate::simulation::state::SimulatedGameState;
use crate::Coord;

const MILLI: u32 = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorContext {
    pub(crate) actor_id: String,
    pub(crate) health_milli: u16,
    pub(crate) length: u16,
    pub(crate) enemy_count: u8,
    pub(crate) largest_enemy_length: u16,
    pub(crate) average_enemy_length: u16,
    pub(crate) lead_over_largest: i16,
    pub(crate) stronger_enemies: u8,
    pub(crate) equal_enemies: u8,
    pub(crate) weaker_enemies: u8,
    pub(crate) board_cells: u32,
    pub(crate) board_occupancy_milli: u16,
    pub(crate) free_space_milli: u16,
    pub(crate) body_density_milli: u16,
    pub(crate) food_density_milli: u16,
    pub(crate) food_per_snake_milli: u16,
    pub(crate) hazard_density_milli: u16,
    pub(crate) crowding_milli: u16,
    pub(crate) duel_milli: u16,
    pub(crate) size_advantage_milli: u16,
    pub(crate) size_disadvantage_milli: u16,
}

impl ActorContext {
    pub(crate) fn from_state(state: &SimulatedGameState, actor_id: &str) -> Option<Self> {
        let actor = state.snake(actor_id).filter(|snake| snake.alive)?;
        let living = state
            .snakes
            .iter()
            .filter(|snake| snake.alive)
            .collect::<Vec<_>>();
        let enemies = living
            .iter()
            .copied()
            .filter(|snake| snake.id != actor_id)
            .collect::<Vec<_>>();

        let actor_length = u32::try_from(actor.length()).unwrap_or(u32::MAX);
        let largest_enemy = enemies
            .iter()
            .map(|snake| u32::try_from(snake.length()).unwrap_or(u32::MAX))
            .max()
            .unwrap_or(0);
        let enemy_length_sum = enemies.iter().fold(0_u32, |sum, snake| {
            sum.saturating_add(u32::try_from(snake.length()).unwrap_or(u32::MAX))
        });
        let enemy_count_u32 = u32::try_from(enemies.len()).unwrap_or(u32::MAX);
        let average_enemy = enemy_length_sum.checked_div(enemy_count_u32).unwrap_or(0);

        let stronger_enemies = enemies
            .iter()
            .filter(|snake| snake.length() > actor.length())
            .count()
            .try_into()
            .unwrap_or(u8::MAX);
        let equal_enemies = enemies
            .iter()
            .filter(|snake| snake.length() == actor.length())
            .count()
            .try_into()
            .unwrap_or(u8::MAX);
        let weaker_enemies = enemies
            .iter()
            .filter(|snake| snake.length() < actor.length())
            .count()
            .try_into()
            .unwrap_or(u8::MAX);

        let board_cells = state.width.saturating_mul(state.height).max(1);
        let occupied = living
            .iter()
            .flat_map(|snake| snake.body.iter().copied())
            .collect::<HashSet<Coord>>()
            .len()
            .try_into()
            .unwrap_or(u32::MAX);

        let board_occupancy_milli = ratio_milli(occupied, board_cells);
        let body_density_milli = ratio_milli(actor_length, board_cells);
        let food_density_milli =
            ratio_milli(state.food.len().try_into().unwrap_or(u32::MAX), board_cells);
        let hazard_density_milli = ratio_milli(
            state.hazards.len().try_into().unwrap_or(u32::MAX),
            board_cells,
        );
        let living_count = u32::try_from(living.len()).unwrap_or(u32::MAX).max(1);
        let food_per_snake_milli = ratio_milli(
            state
                .food
                .len()
                .try_into()
                .unwrap_or(u32::MAX)
                .saturating_mul(MILLI),
            living_count.saturating_mul(MILLI),
        );

        let enemy_pressure = enemy_pressure_milli(enemies.len());
        let crowding_milli = weighted_milli(&[
            (board_occupancy_milli, 500),
            (body_density_milli, 300),
            (enemy_pressure, 200),
        ]);
        let duel_milli = duel_milli(enemies.len());

        let lead = i64::from(actor_length).saturating_sub(i64::from(largest_enemy));
        let lead_over_largest = lead
            .clamp(i64::from(i16::MIN), i64::from(i16::MAX))
            .try_into()
            .unwrap_or(if lead.is_negative() {
                i16::MIN
            } else {
                i16::MAX
            });
        let (size_advantage_milli, size_disadvantage_milli) =
            relative_size_pressure(actor_length, largest_enemy);

        let max_health = state.rules.max_health.max(1);
        let health_milli = actor
            .health
            .max(0)
            .saturating_mul(1000)
            .saturating_div(max_health)
            .clamp(0, 1000)
            .try_into()
            .unwrap_or(1000);

        Some(Self {
            actor_id: actor_id.to_string(),
            health_milli,
            length: actor_length.min(u32::from(u16::MAX)) as u16,
            enemy_count: enemies.len().try_into().unwrap_or(u8::MAX),
            largest_enemy_length: largest_enemy.min(u32::from(u16::MAX)) as u16,
            average_enemy_length: average_enemy.min(u32::from(u16::MAX)) as u16,
            lead_over_largest,
            stronger_enemies,
            equal_enemies,
            weaker_enemies,
            board_cells,
            board_occupancy_milli,
            free_space_milli: 1000_u16.saturating_sub(board_occupancy_milli),
            body_density_milli,
            food_density_milli,
            food_per_snake_milli,
            hazard_density_milli,
            crowding_milli,
            duel_milli,
            size_advantage_milli,
            size_disadvantage_milli,
        })
    }
}

pub(crate) fn enemy_pressure_milli(enemy_count: usize) -> u16 {
    match enemy_count {
        0 => 0,
        1 => 250,
        2 => 500,
        3 => 750,
        _ => 1000,
    }
}

fn duel_milli(enemy_count: usize) -> u16 {
    match enemy_count {
        0 => 0,
        1 => 1000,
        2 => 400,
        3 => 150,
        _ => 0,
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

fn weighted_milli(parts: &[(u16, u16)]) -> u16 {
    let total = parts.iter().fold(0_u32, |sum, (value, weight)| {
        sum.saturating_add(
            u32::from(*value)
                .saturating_mul(u32::from(*weight))
                .saturating_div(MILLI),
        )
    });

    total.min(MILLI).try_into().unwrap_or(1000)
}

#[cfg(test)]
mod tests {
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};

    use super::*;

    fn snake(id: &str, length: usize, health: i32) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
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
            food: vec![Coord { x: 5, y: 5 }],
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
            snake("ours", 8, 90),
            vec![snake("enemy-a", 12, 40), snake("enemy-b", 5, 100)],
        );

        let ours = ActorContext::from_state(&state, "ours").unwrap();
        let enemy = ActorContext::from_state(&state, "enemy-a").unwrap();

        assert_eq!(ours.enemy_count, 2);
        assert_eq!(enemy.enemy_count, 2);
        assert!(ours.size_disadvantage_milli > 0);
        assert_eq!(enemy.size_disadvantage_milli, 0);
        assert!(enemy.size_advantage_milli > 0);
        assert_eq!(ours.stronger_enemies, 1);
        assert_eq!(enemy.stronger_enemies, 0);
    }

    #[test]
    fn lower_health_is_reflected_without_phase_thresholds() {
        let healthy_state = state(snake("ours", 6, 90), vec![snake("enemy", 6, 100)]);
        let hungry_state = state(snake("ours", 6, 20), vec![snake("enemy", 6, 100)]);

        let healthy = ActorContext::from_state(&healthy_state, "ours").unwrap();
        let hungry = ActorContext::from_state(&hungry_state, "ours").unwrap();

        assert!(hungry.health_milli < healthy.health_milli);
    }

    #[test]
    fn more_opponents_increase_contextual_crowding() {
        let duel = state(snake("ours", 6, 90), vec![snake("a", 6, 100)]);
        let crowded = state(
            snake("ours", 6, 90),
            vec![snake("a", 6, 100), snake("b", 6, 100), snake("c", 6, 100)],
        );

        let duel = ActorContext::from_state(&duel, "ours").unwrap();
        let crowded = ActorContext::from_state(&crowded, "ours").unwrap();

        assert!(crowded.crowding_milli > duel.crowding_milli);
        assert!(duel.duel_milli > crowded.duel_milli);
    }
}
