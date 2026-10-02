use super::ActorUtilityMetrics;
use crate::simulation::state::SimulatedGameState;

const STRATEGIC_BUDGET: u16 = 1000;
const MIN_CATEGORY_WEIGHT: u16 = 100;
const MAX_CATEGORY_WEIGHT: u16 = 900;
const MIN_SURVIVAL_WEIGHT: u16 = 150;
const MAX_SURVIVAL_WEIGHT: u16 = 900;
const MAX_TERRITORY_ONLY_SURVIVAL_WEIGHT: u16 = 650;
const SIZE_NEUTRAL_BAND_MILLI: u16 = 120;
const DOMINANT_SIZE_START_MILLI: u32 = 1200;
const DOMINANT_SIZE_FULL_MILLI: u32 = 1400;
const DOMINANT_FOOD_SHARE_AT_START: u16 = 500;
const DOMINANT_FOOD_SHARE_AT_FULL: u16 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StrategicWeights {
    pub(crate) food: u16,
    pub(crate) hunting: u16,
    pub(crate) survival: u16,
}

impl StrategicWeights {
    pub(crate) fn for_actor_metrics(
        state: &SimulatedGameState,
        actor_id: &str,
        metrics: &ActorUtilityMetrics,
    ) -> Option<Self> {
        strategic_weights(state, actor_id, survival_weight_from_metrics(metrics))
    }

    pub(crate) fn for_actor(
        state: &SimulatedGameState,
        actor_id: &str,
        space_capacity_milli: u16,
        territory_control_milli: u16,
    ) -> Option<Self> {
        strategic_weights(
            state,
            actor_id,
            survival_weight(space_capacity_milli, territory_control_milli),
        )
    }

    pub(crate) const fn total(self) -> u16 {
        self.food
            .saturating_add(self.hunting)
            .saturating_add(self.survival)
    }
}

fn strategic_weights(
    state: &SimulatedGameState,
    actor_id: &str,
    survival: u16,
) -> Option<StrategicWeights> {
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
    let offensive_budget = STRATEGIC_BUDGET.saturating_sub(survival);

    let base_food_share = 500_i32
        .saturating_add(i32::from(size_disadvantage_milli) / 2)
        .saturating_sub(i32::from(size_advantage_milli) / 2)
        .clamp(
            i32::from(MIN_CATEGORY_WEIGHT),
            i32::from(MAX_CATEGORY_WEIGHT),
        ) as u16;
    let food_share = base_food_share.min(dominant_food_share_cap(
        actor_length,
        largest_enemy,
    ));

    let food = u32::from(offensive_budget)
        .saturating_mul(u32::from(food_share))
        .saturating_div(u32::from(STRATEGIC_BUDGET))
        .try_into()
        .unwrap_or(offensive_budget);
    let hunting = offensive_budget.saturating_sub(food);

    Some(StrategicWeights {
        food,
        hunting,
        survival,
    })
}

fn survival_weight_from_metrics(metrics: &ActorUtilityMetrics) -> u16 {
    let border_pressure = metrics
        .border_structural_risk_milli
        .max(metrics.border_exposure_milli)
        .max(metrics.border_pin_risk_milli)
        .max(metrics.border_escape_pressure_milli);
    let mut pressures = [
        space_survival_weight(metrics.space_capacity_milli),
        territory_survival_weight(metrics.territory_control_milli)
            .min(MAX_TERRITORY_ONLY_SURVIVAL_WEIGHT),
        match metrics.enclosure_risk {
            0 => MIN_SURVIVAL_WEIGHT,
            1 => 350,
            2 => 650,
            _ => MAX_SURVIVAL_WEIGHT,
        },
        pressure_to_survival_weight(border_pressure),
        pressure_to_survival_weight(metrics.food_survival_pressure_milli),
        pressure_to_survival_weight(metrics.health_pressure_milli),
    ];
    pressures.sort_unstable_by(|left, right| right.cmp(left));

    let primary = pressures[0];
    let secondary_extra = pressures[1]
        .saturating_sub(MIN_SURVIVAL_WEIGHT)
        .saturating_mul(200)
        .saturating_div(1000);

    primary
        .saturating_add(secondary_extra)
        .min(MAX_SURVIVAL_WEIGHT)
}

fn pressure_to_survival_weight(pressure_milli: u16) -> u16 {
    let pressure = pressure_milli.min(1000);
    MIN_SURVIVAL_WEIGHT.saturating_add(
        u32::from(MAX_SURVIVAL_WEIGHT.saturating_sub(MIN_SURVIVAL_WEIGHT))
            .saturating_mul(u32::from(pressure))
            .saturating_div(1000)
            .try_into()
            .unwrap_or(0),
    )
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

fn dominant_food_share_cap(actor_length: u32, largest_enemy: u32) -> u16 {
    if largest_enemy == 0 {
        return DOMINANT_FOOD_SHARE_AT_FULL;
    }

    let ratio_milli = u64::from(actor_length)
        .saturating_mul(1000)
        .saturating_div(u64::from(largest_enemy))
        .min(u64::from(u32::MAX)) as u32;

    if ratio_milli <= DOMINANT_SIZE_START_MILLI {
        return STRATEGIC_BUDGET;
    }
    if ratio_milli >= DOMINANT_SIZE_FULL_MILLI {
        return DOMINANT_FOOD_SHARE_AT_FULL;
    }

    let progress = ratio_milli.saturating_sub(DOMINANT_SIZE_START_MILLI);
    let span = DOMINANT_SIZE_FULL_MILLI
        .saturating_sub(DOMINANT_SIZE_START_MILLI)
        .max(1);
    let reduction = u32::from(
        DOMINANT_FOOD_SHARE_AT_START.saturating_sub(DOMINANT_FOOD_SHARE_AT_FULL),
    )
    .saturating_mul(progress)
    .saturating_div(span);

    u32::from(DOMINANT_FOOD_SHARE_AT_START)
        .saturating_sub(reduction)
        .try_into()
        .unwrap_or(DOMINANT_FOOD_SHARE_AT_FULL)
}

fn relative_size_pressure(actor_length: u32, reference_enemy: u32) -> (u16, u16) {
    if reference_enemy == 0 {
        return (1000, 0);
    }

    if actor_length >= reference_enemy {
        let lead = actor_length.saturating_sub(reference_enemy);
        (
            pressure_above_neutral_band(ratio_milli(lead, reference_enemy.max(1))),
            0,
        )
    } else {
        let deficit = reference_enemy.saturating_sub(actor_length);
        (
            0,
            pressure_above_neutral_band(ratio_milli(deficit, actor_length.max(1))),
        )
    }
}

fn pressure_above_neutral_band(raw_milli: u16) -> u16 {
    if raw_milli <= SIZE_NEUTRAL_BAND_MILLI {
        return 0;
    }

    let remaining = 1000_u32.saturating_sub(u32::from(SIZE_NEUTRAL_BAND_MILLI));
    u32::from(raw_milli.saturating_sub(SIZE_NEUTRAL_BAND_MILLI))
        .saturating_mul(1000)
        .saturating_div(remaining.max(1))
        .min(1000)
        .try_into()
        .unwrap_or(1000)
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
            assert!(territory_survival_weight(pair[0]) >= territory_survival_weight(pair[1]));
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

    fn metrics(
        space: u16,
        territory: u16,
        enclosure: u8,
        border: u16,
        starvation: u16,
        health: u16,
    ) -> ActorUtilityMetrics {
        ActorUtilityMetrics {
            safe_non_reverse_moves: 3,
            enclosure_risk: enclosure,
            border_structural_risk_milli: border,
            border_exposure_milli: 0,
            border_pin_risk_milli: 0,
            border_escape_pressure_milli: 0,
            space_capacity_milli: space,
            territory_control_milli: territory,
            food_potential_milli: 0,
            food_survival_pressure_milli: starvation,
            health_pressure_milli: health,
        }
    }

    #[test]
    fn border_escape_pressure_dominates_hunting_size_advantage() {
        let state = state(&[("ours", 15), ("enemy", 6)]);
        let mut current = metrics(1000, 800, 0, 0, 0, 0);
        current.border_escape_pressure_milli = 900;
        let weights = StrategicWeights::for_actor_metrics(&state, "ours", &current).unwrap();

        assert!(weights.survival >= 825);
        assert!(weights.survival > weights.hunting);
    }

    #[test]
    fn critical_starvation_dominates_comfortable_space_and_territory() {
        let state = state(&[("ours", 12), ("enemy", 6)]);
        let current = metrics(1000, 800, 0, 0, 1000, 0);
        let weights = StrategicWeights::for_actor_metrics(&state, "ours", &current).unwrap();

        assert_eq!(weights.survival, MAX_SURVIVAL_WEIGHT);
        assert!(weights.survival > weights.food.saturating_add(weights.hunting));
    }

    #[test]
    fn critical_enclosure_dominates_size_advantage() {
        let state = state(&[("ours", 15), ("enemy", 6)]);
        let current = metrics(1000, 800, 3, 0, 0, 0);
        let weights = StrategicWeights::for_actor_metrics(&state, "ours", &current).unwrap();

        assert_eq!(weights.survival, MAX_SURVIVAL_WEIGHT);
    }

    #[test]
    fn healthy_open_state_keeps_survival_low() {
        let state = state(&[("ours", 8), ("enemy", 8)]);
        let current = metrics(1000, 700, 0, 0, 0, 0);
        let weights = StrategicWeights::for_actor_metrics(&state, "ours", &current).unwrap();

        assert!(weights.survival <= 250);
    }

    #[test]
    fn near_equal_sizes_stay_inside_neutral_offensive_band() {
        assert_eq!(relative_size_pressure(10, 11), (0, 0));
        assert_eq!(relative_size_pressure(11, 10), (0, 0));
    }

    #[test]
    fn size_pressure_grows_smoothly_after_neutral_band() {
        let (_, small_deficit) = relative_size_pressure(8, 9);
        let (_, large_deficit) = relative_size_pressure(5, 10);

        assert!(small_deficit < large_deficit);
        assert!(large_deficit > 0);
    }

    #[test]
    fn forty_percent_size_lead_almost_eliminates_food_weight() {
        let state = state(&[("ours", 14), ("enemy-a", 10), ("enemy-b", 8)]);
        let weights = StrategicWeights::for_actor(&state, "ours", 1000, 650).unwrap();
        let offensive = weights.food.saturating_add(weights.hunting);

        assert!(weights.hunting > weights.food);
        assert!(
            u32::from(weights.food).saturating_mul(100)
                <= u32::from(offensive).saturating_mul(6)
        );
    }

    #[test]
    fn dominant_food_cap_tightens_smoothly_toward_forty_percent_lead() {
        let cap_20 = dominant_food_share_cap(12, 10);
        let cap_30 = dominant_food_share_cap(13, 10);
        let cap_40 = dominant_food_share_cap(14, 10);

        assert!(cap_20 > cap_30);
        assert!(cap_30 > cap_40);
        assert_eq!(cap_40, DOMINANT_FOOD_SHARE_AT_FULL);
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
