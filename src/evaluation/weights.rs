use crate::simulation::state::SimulatedGameState;

const STRATEGIC_BUDGET: u16 = 1000;
const MIN_CATEGORY_WEIGHT: u16 = 100;
const MAX_CATEGORY_WEIGHT: u16 = 900;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StrategicWeights {
    pub(crate) food: u16,
    pub(crate) hunting: u16,
    pub(crate) survival: u16,
}

impl StrategicWeights {
    pub(crate) fn for_actor(
        state: &SimulatedGameState,
        actor_id: &str,
        territory_share_milli: u16,
    ) -> Option<Self> {
        let actor = state.snake(actor_id).filter(|snake| snake.alive)?;
        let actor_length = u32::try_from(actor.length()).unwrap_or(u32::MAX);

        let mut enemy_count = 0_u32;
        let mut enemy_length_sum = 0_u32;
        let mut largest_enemy = 0_u32;
        for enemy in state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != actor_id)
        {
            let length = u32::try_from(enemy.length()).unwrap_or(u32::MAX);
            enemy_count = enemy_count.saturating_add(1);
            enemy_length_sum = enemy_length_sum.saturating_add(length);
            largest_enemy = largest_enemy.max(length);
        }

        let reference_enemy = if enemy_count == 0 {
            0
        } else {
            let mean = enemy_length_sum.saturating_div(enemy_count);
            largest_enemy
                .saturating_mul(7)
                .saturating_add(mean.saturating_mul(3))
                .saturating_div(10)
        };
        let (size_advantage_milli, size_disadvantage_milli) =
            relative_size_pressure(actor_length, reference_enemy);

        let survival = STRATEGIC_BUDGET
            .saturating_sub(territory_share_milli)
            .clamp(MIN_CATEGORY_WEIGHT, MAX_CATEGORY_WEIGHT);
        let offensive_budget = STRATEGIC_BUDGET.saturating_sub(survival);

        let food_share = 500_i32
            .saturating_add(i32::from(size_disadvantage_milli) / 2)
            .saturating_sub(i32::from(size_advantage_milli) / 2)
            .clamp(
                i32::from(MIN_CATEGORY_WEIGHT),
                i32::from(MAX_CATEGORY_WEIGHT),
            ) as u16;

        let food = u32::from(offensive_budget)
            .saturating_mul(u32::from(food_share))
            .saturating_div(u32::from(STRATEGIC_BUDGET))
            .try_into()
            .unwrap_or(offensive_budget);
        let hunting = offensive_budget.saturating_sub(food);

        Some(Self {
            food,
            hunting,
            survival,
        })
    }

    pub(crate) const fn total(self) -> u16 {
        self.food
            .saturating_add(self.hunting)
            .saturating_add(self.survival)
    }
}

fn relative_size_pressure(actor_length: u32, reference_enemy: u32) -> (u16, u16) {
    if reference_enemy == 0 {
        return (1000, 0);
    }

    if actor_length >= reference_enemy {
        let lead = actor_length.saturating_sub(reference_enemy);
        (ratio_milli(lead, reference_enemy.max(1)), 0)
    } else {
        let deficit = reference_enemy.saturating_sub(actor_length);
        (0, ratio_milli(deficit, actor_length.max(1)))
    }
}

fn ratio_milli(numerator: u32, denominator: u32) -> u16 {
    if denominator == 0 {
        return 0;
    }

    numerator
        .saturating_mul(1000)
        .saturating_div(denominator)
        .min(1000)
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

    fn state(lengths: &[(&str, usize)]) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 11,
            height: 11,
            food: vec![],
            hazards: vec![],
            snakes: lengths
                .iter()
                .map(|(id, length)| snake(id, *length))
                .collect(),
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
    fn strategic_weights_always_consume_the_whole_budget() {
        let state = state(&[("ours", 6), ("enemy", 6)]);
        let weights = StrategicWeights::for_actor(&state, "ours", 400).unwrap();
        assert_eq!(weights.total(), STRATEGIC_BUDGET);
    }

    #[test]
    fn less_territory_increases_survival_and_suppresses_offense() {
        let state = state(&[("ours", 6), ("enemy", 6)]);
        let comfortable = StrategicWeights::for_actor(&state, "ours", 700).unwrap();
        let constrained = StrategicWeights::for_actor(&state, "ours", 150).unwrap();

        assert!(constrained.survival > comfortable.survival);
        assert!(
            constrained.food.saturating_add(constrained.hunting)
                < comfortable.food.saturating_add(comfortable.hunting)
        );
    }

    #[test]
    fn size_disadvantage_shifts_offense_from_hunting_to_food() {
        let smaller_state = state(&[("ours", 5), ("enemy-a", 12), ("enemy-b", 8)]);
        let larger_state = state(&[("ours", 14), ("enemy-a", 7), ("enemy-b", 6)]);

        let smaller = StrategicWeights::for_actor(&smaller_state, "ours", 600).unwrap();
        let larger = StrategicWeights::for_actor(&larger_state, "ours", 600).unwrap();

        assert!(smaller.food > smaller.hunting);
        assert!(larger.hunting > larger.food);
        assert!(smaller.food > larger.food);
        assert!(larger.hunting > smaller.hunting);
    }

    #[test]
    fn largest_threat_matters_more_than_enemy_mean() {
        let even = state(&[("ours", 8), ("enemy-a", 8), ("enemy-b", 8)]);
        let one_large = state(&[("ours", 8), ("enemy-a", 14), ("enemy-b", 2)]);

        let even = StrategicWeights::for_actor(&even, "ours", 600).unwrap();
        let one_large = StrategicWeights::for_actor(&one_large, "ours", 600).unwrap();

        assert!(one_large.food > even.food);
        assert!(one_large.hunting < even.hunting);
    }
}
