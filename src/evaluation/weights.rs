use super::context::{enemy_pressure_milli, ActorContext};
use super::metrics::ActorMetrics;

const MAX_WEIGHT: u32 = 2000;
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
    pub(crate) fn from_actor(context: &ActorContext, metrics: &ActorMetrics) -> Self {
        Self::from_territory_share(context, metrics.territory_share_milli)
    }

    pub(crate) fn from_territory_share(
        context: &ActorContext,
        territory_share_milli: u16,
    ) -> Self {
        let survival = STRATEGIC_BUDGET
            .saturating_sub(territory_share_milli)
            .clamp(MIN_CATEGORY_WEIGHT, MAX_CATEGORY_WEIGHT);
        let offensive_budget = STRATEGIC_BUDGET.saturating_sub(survival);

        let food_share = 500_i32
            .saturating_add(i32::from(context.size_disadvantage_milli) / 2)
            .saturating_sub(i32::from(context.size_advantage_milli) / 2)
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

        Self {
            food,
            hunting,
            survival,
        }
    }

    pub(crate) const fn total(self) -> u16 {
        self.food
            .saturating_add(self.hunting)
            .saturating_add(self.survival)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NeedWeights {
    pub(crate) survival: u16,
    pub(crate) mobility: u16,
    pub(crate) space: u16,
    pub(crate) enclosure: u16,
    pub(crate) border: u16,
    pub(crate) food: u16,
    pub(crate) growth: u16,
    pub(crate) territory: u16,
    pub(crate) dominance: u16,
    pub(crate) hunting: u16,
    pub(crate) pressure: u16,
    pub(crate) stability: u16,
}

impl NeedWeights {
    pub(crate) fn from_context(context: &ActorContext) -> Self {
        let health_pressure = 1000_u16.saturating_sub(context.health_milli);
        let food_scarcity = 1000_u16.saturating_sub(context.food_per_snake_milli);
        let enemy_pressure = enemy_pressure_milli(usize::from(context.enemy_count));

        let survival = weight(
            1000,
            &[
                (enemy_pressure, 350),
                (context.size_disadvantage_milli, 350),
                (context.crowding_milli, 300),
            ],
        );
        let mobility = weight(
            500,
            &[
                (context.body_density_milli, 700),
                (context.crowding_milli, 500),
                (enemy_pressure, 300),
            ],
        );
        let space = weight(
            500,
            &[
                (context.body_density_milli, 500),
                (context.crowding_milli, 500),
                (context.size_disadvantage_milli, 300),
            ],
        );
        let enclosure = weight(
            500,
            &[
                (context.body_density_milli, 650),
                (context.crowding_milli, 550),
                (enemy_pressure, 250),
            ],
        );
        let border = weight(
            200,
            &[
                (context.body_density_milli, 800),
                (context.crowding_milli, 500),
            ],
        );
        let food = weight(
            200,
            &[
                (health_pressure, 1200),
                (context.size_disadvantage_milli, 500),
                (food_scarcity, 300),
            ],
        );
        let growth = weight(
            250,
            &[
                (context.size_disadvantage_milli, 1000),
                (food_scarcity, 250),
            ],
        );
        let territory = weight(
            350,
            &[
                (context.crowding_milli, 600),
                (enemy_pressure, 400),
                (context.body_density_milli, 350),
            ],
        );
        let dominance = weight(
            150,
            &[
                (context.size_advantage_milli, 900),
                (context.duel_milli, 350),
            ],
        );

        let offensive_health = 1000_u16.saturating_sub(
            u32::from(health_pressure)
                .saturating_mul(700)
                .saturating_div(1000)
                .min(1000)
                .try_into()
                .unwrap_or(1000),
        );
        let multi_enemy_damping = 1000_u16.saturating_sub(
            u32::from(enemy_pressure)
                .saturating_mul(350)
                .saturating_div(1000)
                .min(1000)
                .try_into()
                .unwrap_or(1000),
        );
        let hunting_raw = weight(
            100,
            &[
                (context.size_advantage_milli, 1000),
                (context.duel_milli, 600),
            ],
        );
        let hunting = damp(damp(hunting_raw, offensive_health), multi_enemy_damping);
        let pressure = damp(
            weight(
                150,
                &[
                    (context.size_advantage_milli, 750),
                    (context.duel_milli, 450),
                    (context.crowding_milli, 250),
                ],
            ),
            offensive_health,
        );
        let stability = weight(
            250,
            &[
                (1000_u16.saturating_sub(health_pressure), 250),
                (1000_u16.saturating_sub(context.crowding_milli), 200),
            ],
        );

        Self {
            survival,
            mobility,
            space,
            enclosure,
            border,
            food,
            growth,
            territory,
            dominance,
            hunting,
            pressure,
            stability,
        }
    }
}

fn weight(base: u32, signals: &[(u16, u16)]) -> u16 {
    let value = signals.iter().fold(base, |sum, (signal, gain)| {
        sum.saturating_add(
            u32::from(*signal)
                .saturating_mul(u32::from(*gain))
                .saturating_div(1000),
        )
    });

    value
        .min(MAX_WEIGHT)
        .try_into()
        .unwrap_or(MAX_WEIGHT as u16)
}

fn damp(weight: u16, factor_milli: u16) -> u16 {
    u32::from(weight)
        .saturating_mul(u32::from(factor_milli))
        .saturating_div(1000)
        .min(MAX_WEIGHT)
        .try_into()
        .unwrap_or(MAX_WEIGHT as u16)
}

#[cfg(test)]
mod tests {
    use crate::evaluation::context::ActorContext;
    use crate::evaluation::metrics::ActorMetrics;

    use super::*;

    fn context() -> ActorContext {
        ActorContext {
            actor_id: "ours".to_string(),
            health_milli: 900,
            length: 6,
            enemy_count: 1,
            largest_enemy_length: 6,
            average_enemy_length: 6,
            lead_over_largest: 0,
            stronger_enemies: 0,
            equal_enemies: 1,
            weaker_enemies: 0,
            board_cells: 121,
            board_occupancy_milli: 100,
            free_space_milli: 900,
            body_density_milli: 50,
            food_density_milli: 25,
            food_per_snake_milli: 500,
            hazard_density_milli: 0,
            crowding_milli: 150,
            duel_milli: 1000,
            size_advantage_milli: 0,
            size_disadvantage_milli: 0,
        }
    }

    fn metrics(territory_share_milli: u16) -> ActorMetrics {
        ActorMetrics {
            health_milli: 900,
            safe_moves: 3,
            safe_non_reverse_moves: 3,
            reachable_space: 40,
            space_to_length_milli: 4000,
            escape_frontier: 3,
            enclosure_risk: 0,
            useful_chokes: 0,
            boundary_support: 0,
            edge_distance: 3,
            body_on_edge: 0,
            body_near_edge: 0,
            leading_edge_chain: 0,
            inward_safe_moves: 2,
            corner_contact: false,
            border_preference_milli: 0,
            border_structural_risk_milli: 0,
            inward_control_milli: 900,
            enemy_pin_risk_milli: 0,
            reachable_territory: 40,
            exclusive_space: 30,
            contested_space: 5,
            control_ratio_milli: 700,
            territory_share_milli,
            controlled_food: 1,
            contested_food: 0,
            winning_frontier: 2,
            losing_frontier: 1,
            dominance_claim_cells: 2,
            dominance_frontier_cells: 1,
            favorable_head_frontier: 1,
            best_food_distance: Some(3),
            best_food_claim_margin: Some(1),
            best_food_contested: false,
            hunting_opportunity_milli: 300,
            pressure_opportunity_milli: 300,
        }
    }

    #[test]
    fn strategic_weights_always_consume_the_whole_budget() {
        let weights = StrategicWeights::from_actor(&context(), &metrics(400));

        assert_eq!(weights.total(), STRATEGIC_BUDGET);
    }

    #[test]
    fn less_territory_increases_survival_and_suppresses_offense() {
        let ctx = context();
        let comfortable = StrategicWeights::from_actor(&ctx, &metrics(700));
        let constrained = StrategicWeights::from_actor(&ctx, &metrics(150));

        assert!(constrained.survival > comfortable.survival);
        assert!(
            constrained.food.saturating_add(constrained.hunting)
                < comfortable.food.saturating_add(comfortable.hunting)
        );
    }

    #[test]
    fn size_disadvantage_shifts_offense_from_hunting_to_food() {
        let mut smaller = context();
        smaller.size_disadvantage_milli = 800;
        smaller.size_advantage_milli = 0;

        let mut larger = context();
        larger.size_disadvantage_milli = 0;
        larger.size_advantage_milli = 800;

        let smaller = StrategicWeights::from_actor(&smaller, &metrics(600));
        let larger = StrategicWeights::from_actor(&larger, &metrics(600));

        assert!(smaller.food > smaller.hunting);
        assert!(larger.hunting > larger.food);
        assert!(smaller.food > larger.food);
        assert!(larger.hunting > smaller.hunting);
    }

    #[test]
    fn lower_health_never_reduces_food_need() {
        let healthy = context();
        let mut hungry = healthy.clone();
        hungry.health_milli = 200;

        assert!(
            NeedWeights::from_context(&hungry).food >= NeedWeights::from_context(&healthy).food
        );
    }

    #[test]
    fn size_disadvantage_increases_growth_need() {
        let even = context();
        let mut smaller = even.clone();
        smaller.size_disadvantage_milli = 700;

        assert!(
            NeedWeights::from_context(&smaller).growth > NeedWeights::from_context(&even).growth
        );
        assert!(NeedWeights::from_context(&smaller).food > NeedWeights::from_context(&even).food);
    }

    #[test]
    fn crowding_increases_space_and_enclosure_need() {
        let open = context();
        let mut crowded = open.clone();
        crowded.crowding_milli = 900;
        crowded.body_density_milli = 500;

        let open_weights = NeedWeights::from_context(&open);
        let crowded_weights = NeedWeights::from_context(&crowded);

        assert!(crowded_weights.mobility > open_weights.mobility);
        assert!(crowded_weights.space > open_weights.space);
        assert!(crowded_weights.enclosure > open_weights.enclosure);
        assert!(crowded_weights.border > open_weights.border);
    }

    #[test]
    fn duel_and_size_advantage_raise_hunting_need() {
        let neutral = context();
        let mut dominant = neutral.clone();
        dominant.size_advantage_milli = 800;

        assert!(
            NeedWeights::from_context(&dominant).hunting
                > NeedWeights::from_context(&neutral).hunting
        );
        assert!(
            NeedWeights::from_context(&dominant).dominance
                > NeedWeights::from_context(&neutral).dominance
        );
    }

    #[test]
    fn multiple_enemies_dampen_single_target_hunting() {
        let duel = context();
        let mut crowded = duel.clone();
        crowded.enemy_count = 4;
        crowded.duel_milli = 0;

        assert!(
            NeedWeights::from_context(&crowded).hunting < NeedWeights::from_context(&duel).hunting
        );
    }
}
