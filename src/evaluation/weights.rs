use super::context::{enemy_pressure_milli, ActorContext};

const MAX_WEIGHT: u32 = 2000;

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
