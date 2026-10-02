use crate::simulation::state::SimulatedGameState;

const STRATEGIC_BUDGET: u16 = 1000;
const MIN_CATEGORY_WEIGHT: u16 = 100;
const MAX_CATEGORY_WEIGHT: u16 = 900;
const MIN_SURVIVAL_WEIGHT: u16 = 150;
const MAX_SURVIVAL_WEIGHT: u16 = 900;
const MAX_TERRITORY_ONLY_SURVIVAL_WEIGHT: u16 = 650;

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
        space_capacity_milli: u16,
        territory_control_milli: u16,
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

        let survival = survival_weight(space_capacity_milli, territory_control_milli);
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

fn survival_weight(space_capacity_milli: u16, territory_control_milli: u16) -> u16 {
    let spatial = space_survival_weight(space_capacity_milli);
    let territorial =
        territory_survival_weight(territory_control_milli).min(MAX_TERRITORY_ONLY_SURVIVAL_WEIGHT);
    spatial.max(territorial)
}

fn space_survival_weight(space_capacity_milli: u16) -> u16 {
    let capacity = space_capacity_milli.min(1000);
    match capacity {
        0..=100 => MAX_SURVIVAL_WEIGHT,
        101..=250 => interpolate(capacity, 100, 250, 900, 800),
        251..=400 => interpolate(capacity, 250, 400, 800, 650),
        401..=550 => interpolate(capacity, 400, 550, 650, 500),
        551..=700 => interpolate(capacity, 550, 700, 500, 350),
        701..=850 => interpolate(capacity, 700, 850, 350, 220),
        _ => interpolate(capacity, 850, 1000, 220, MIN_SURVIVAL_WEIGHT),
    }
}

fn territory_survival_weight(territory_control_milli: u16) -> u16 {
    let territory = territory_control_milli.min(1000);
    match territory {
        0..=100 => MAX_SURVIVAL_WEIGHT,
        101..=150 => interpolate(territory, 100, 150, 900, 750),
        151..=200 => interpolate(territory, 150, 200, 750, 600),
        201..=250 => interpolate(territory, 200, 250, 600, 450),
        251..=300 => interpolate(territory, 250, 300, 450, 350),
        301..=350 => interpolate(territory, 300, 350, 350, 250),
        351..=450 => interpolate(territory, 350, 450, 250, 150),
        _ => MIN_SURVIVAL_WEIGHT,
    }
}

fn interpolate(value: u16, x0: u16, x1: u16, y0: u16, y1: u16) -> u16 {
    let span = u32::from(x1.saturating_sub(x0)).max(1);
    let offset = u32::from(value.saturating_sub(x0).min(x1.saturating_sub(x0)));
    let drop = u32::from(y0.saturating_sub(y1))
        .saturating_mul(offset)
        .saturating_div(span);

    u32::from(y0).saturating_sub(drop).try_into().unwrap_or(y1)
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
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
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
        }
    }

    #[test]
    fn strategic_weights_always_consume_the_whole_budget() {
        let state = state(&[("ours", 6), ("enemy", 6)]);
        let weights = StrategicWeights::for_actor(&state, "ours", 1000, 400).unwrap();
        assert_eq!(weights.total(), STRATEGIC_BUDGET);
    }

    #[test]
    fn less_territory_increases_survival_and_suppresses_offense() {
        let state = state(&[("ours", 6), ("enemy", 6)]);
        let comfortable = StrategicWeights::for_actor(&state, "ours", 1000, 700).unwrap();
        let constrained = StrategicWeights::for_actor(&state, "ours", 1000, 150).unwrap();

        assert!(constrained.survival > comfortable.survival);
        assert!(
            constrained.food.saturating_add(constrained.hunting)
                < comfortable.food.saturating_add(comfortable.hunting)
        );
    }

    #[test]
    fn territory_curve_is_moderate_at_normal_four_player_share() {
        assert_eq!(territory_survival_weight(100), 900);
        assert_eq!(territory_survival_weight(150), 750);
        assert_eq!(territory_survival_weight(200), 600);
        assert_eq!(territory_survival_weight(250), 450);
        assert_eq!(territory_survival_weight(300), 350);
        assert_eq!(territory_survival_weight(350), 250);
        assert_eq!(territory_survival_weight(450), 150);
        assert_eq!(territory_survival_weight(700), 150);
    }

    #[test]
    fn territory_curve_decreases_monotonically_with_control() {
        let samples = [50, 100, 150, 200, 250, 300, 350, 450, 700];
        for pair in samples.windows(2) {
            assert!(survival_weight(pair[0]) >= survival_weight(pair[1]));
        }
    }

    #[test]
    fn large_space_with_low_control_is_pressured_but_not_spatially_critical() {
        let state = state(&[("ours", 6), ("enemy", 6)]);
        let weights = StrategicWeights::for_actor(&state, "ours", 1000, 120).unwrap();

        assert_eq!(weights.survival, MAX_TERRITORY_ONLY_SURVIVAL_WEIGHT);
        assert!(weights.survival < MAX_SURVIVAL_WEIGHT);
    }

    #[test]
    fn small_space_dominates_even_when_territory_control_is_high() {
        let state = state(&[("ours", 12), ("enemy", 6)]);
        let weights = StrategicWeights::for_actor(&state, "ours", 50, 800).unwrap();

        assert_eq!(weights.survival, MAX_SURVIVAL_WEIGHT);
        assert!(weights.survival > weights.food.saturating_add(weights.hunting));
    }

    #[test]
    fn spatial_capacity_relaxes_survival_monotonically() {
        let samples = [0, 100, 250, 400, 550, 700, 850, 1000];
        for pair in samples.windows(2) {
            assert!(space_survival_weight(pair[0]) >= space_survival_weight(pair[1]));
        }
    }

    #[test]
    fn size_disadvantage_shifts_offense_from_hunting_to_food() {
        let smaller_state = state(&[("ours", 5), ("enemy-a", 12), ("enemy-b", 8)]);
        let larger_state = state(&[("ours", 14), ("enemy-a", 7), ("enemy-b", 6)]);

        let smaller = StrategicWeights::for_actor(&smaller_state, "ours", 1000, 600).unwrap();
        let larger = StrategicWeights::for_actor(&larger_state, "ours", 1000, 600).unwrap();

        assert!(smaller.food > smaller.hunting);
        assert!(larger.hunting > larger.food);
        assert!(smaller.food > larger.food);
        assert!(larger.hunting > smaller.hunting);
    }

    #[test]
    fn largest_threat_matters_more_than_enemy_mean() {
        let even = state(&[("ours", 8), ("enemy-a", 8), ("enemy-b", 8)]);
        let one_large = state(&[("ours", 8), ("enemy-a", 14), ("enemy-b", 2)]);

        let even = StrategicWeights::for_actor(&even, "ours", 1000, 600).unwrap();
        let one_large = StrategicWeights::for_actor(&one_large, "ours", 1000, 600).unwrap();

        assert!(one_large.food > even.food);
        assert!(one_large.hunting < even.hunting);
    }

    #[test]
    fn constrained_territory_keeps_survival_dominant_even_for_largest_snake() {
        let state = state(&[("ours", 16), ("enemy-a", 7), ("enemy-b", 6)]);
        let weights = StrategicWeights::for_actor(&state, "ours", 1000, 120).unwrap();

        assert!(weights.survival > weights.food);
        assert!(weights.survival > weights.hunting);
        assert!(weights.survival > weights.food.saturating_add(weights.hunting));
    }

    #[test]
    fn comfortable_territory_lets_relative_size_choose_growth_or_hunting() {
        let smaller_state = state(&[("ours", 5), ("enemy-a", 10), ("enemy-b", 9)]);
        let larger_state = state(&[("ours", 15), ("enemy-a", 7), ("enemy-b", 6)]);

        let smaller = StrategicWeights::for_actor(&smaller_state, "ours", 1000, 650).unwrap();
        let larger = StrategicWeights::for_actor(&larger_state, "ours", 1000, 650).unwrap();

        assert!(smaller.food > smaller.hunting);
        assert!(smaller.food > smaller.survival);
        assert!(larger.hunting > larger.food);
        assert!(larger.hunting > larger.survival);
    }
}
