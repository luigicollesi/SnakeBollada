use super::{ActorContext, ActorMetrics, NeedWeights};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActorBenefits {
    pub(crate) survival_margin: i32,
    pub(crate) mobility: i32,
    pub(crate) space: i32,
    pub(crate) food_access: i32,
    pub(crate) growth: i32,
    pub(crate) territory: i32,
    pub(crate) dominance: i32,
    pub(crate) hunting: i32,
    pub(crate) pressure: i32,
    pub(crate) stability: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActorHarms {
    pub(crate) dead_end: i32,
    pub(crate) constrained: i32,
    pub(crate) low_mobility: i32,
    pub(crate) insufficient_space: i32,
    pub(crate) enclosure: i32,
    pub(crate) border: i32,
    pub(crate) corner: i32,
    pub(crate) enemy_pin: i32,
    pub(crate) starvation: i32,
    pub(crate) food_denial: i32,
    pub(crate) territory_loss: i32,
    pub(crate) enemy_dominance: i32,
    pub(crate) instability: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActorEvaluation {
    pub(crate) context: ActorContext,
    pub(crate) weights: NeedWeights,
    pub(crate) metrics: ActorMetrics,
    pub(crate) benefits: ActorBenefits,
    pub(crate) harms: ActorHarms,
    pub(crate) benefit_total: i64,
    pub(crate) harm_total: i64,
    pub(crate) net: i64,
}

impl ActorEvaluation {
    pub(crate) fn from_metrics(context: ActorContext, metrics: ActorMetrics) -> Self {
        let weights = NeedWeights::from_context(&context);
        let benefits = classify_benefits(&context, &metrics);
        let harms = classify_harms(&context, &metrics);

        let benefit_total = weighted_benefits(&benefits, &weights);
        let harm_total = weighted_harms(&harms, &weights);

        Self {
            context,
            weights,
            metrics,
            benefits,
            harms,
            benefit_total,
            harm_total,
            net: benefit_total.saturating_sub(harm_total),
        }
    }
}

fn classify_benefits(context: &ActorContext, metrics: &ActorMetrics) -> ActorBenefits {
    let mobility = u32::from(metrics.safe_moves.min(4))
        .saturating_mul(250)
        .try_into()
        .unwrap_or(1000);
    let space = space_comfort_milli(metrics);
    let survival_margin = average_milli(&[
        mobility,
        space,
        metrics.inward_control_milli,
        u16::from(metrics.escape_frontier.min(4)).saturating_mul(250),
    ]);
    let food_access = food_access_milli(metrics);
    let growth = u32::from(food_access)
        .saturating_mul(u32::from(context.size_disadvantage_milli.max(250)))
        .saturating_div(1000)
        .min(1000)
        .try_into()
        .unwrap_or(1000);
    let territory = territory_benefit_milli(metrics);
    let dominance = dominance_benefit_milli(metrics);
    let stability = average_milli(&[
        metrics.inward_control_milli,
        1000_u16.saturating_sub(metrics.border_structural_risk_milli),
        u16::from(metrics.escape_frontier.min(4)).saturating_mul(250),
    ]);

    ActorBenefits {
        survival_margin: i32::from(survival_margin),
        mobility: i32::from(mobility),
        space: i32::from(space),
        food_access: i32::from(food_access),
        growth,
        territory: i32::from(territory),
        dominance: i32::from(dominance),
        hunting: i32::from(metrics.hunting_opportunity_milli),
        pressure: i32::from(metrics.pressure_opportunity_milli),
        stability: i32::from(stability),
    }
}

fn classify_harms(_context: &ActorContext, metrics: &ActorMetrics) -> ActorHarms {
    let dead_end = if metrics.safe_moves == 0 { 1000 } else { 0 };
    let constrained = match metrics.safe_moves {
        0 => 1000,
        1 => 800,
        2 => 350,
        _ => 0,
    };
    let mobility = u16::from(metrics.safe_moves.min(4)).saturating_mul(250);
    let low_mobility = 1000_u16.saturating_sub(mobility);
    let insufficient_space = 1000_u16.saturating_sub(space_comfort_milli(metrics));

    let choke_pressure = u16::from(metrics.useful_chokes.min(4)).saturating_mul(125);
    let boundary_pressure = u16::from(metrics.boundary_support.min(3)).saturating_mul(150);
    let escape_pressure =
        1000_u16.saturating_sub(u16::from(metrics.escape_frontier.min(4)).saturating_mul(250));
    let enclosure = average_milli(&[
        u16::from(metrics.enclosure_risk.min(3))
            .saturating_mul(333)
            .min(1000),
        insufficient_space,
        escape_pressure,
        choke_pressure.saturating_add(boundary_pressure).min(1000),
    ]);

    let body_edge_pressure = ratio_milli(
        u32::from(metrics.body_on_edge)
            .saturating_add(u32::from(metrics.body_near_edge))
            .saturating_add(u32::from(metrics.leading_edge_chain)),
        u32::from(metrics.body_on_edge)
            .saturating_add(u32::from(metrics.body_near_edge))
            .saturating_add(u32::from(metrics.leading_edge_chain))
            .saturating_add(6),
    );
    let edge_distance_pressure = match metrics.edge_distance {
        0 => 1000,
        1 => 500,
        2 => 200,
        _ => 0,
    };
    let border = average_milli(&[
        metrics.border_structural_risk_milli,
        metrics.border_preference_milli,
        body_edge_pressure,
        edge_distance_pressure,
    ]);
    let corner = if metrics.corner_contact { 1000 } else { 0 };
    let starvation = 1000_u16.saturating_sub(metrics.health_milli);
    let food_denial = food_denial_milli(metrics);
    let territory_loss = territory_loss_milli(metrics);
    let enemy_dominance = dominance_harm_milli(metrics);
    let instability = average_milli(&[
        1000_u16.saturating_sub(metrics.inward_control_milli),
        escape_pressure,
        metrics.enemy_pin_risk_milli,
        u16::from(4_u8.saturating_sub(metrics.inward_safe_moves.min(4))).saturating_mul(250),
    ]);

    ActorHarms {
        dead_end,
        constrained,
        low_mobility: i32::from(low_mobility),
        insufficient_space: i32::from(insufficient_space),
        enclosure: i32::from(enclosure),
        border: i32::from(border),
        corner,
        enemy_pin: i32::from(metrics.enemy_pin_risk_milli),
        starvation: i32::from(starvation),
        food_denial: i32::from(food_denial),
        territory_loss: i32::from(territory_loss),
        enemy_dominance: i32::from(enemy_dominance),
        instability: i32::from(instability),
    }
}

fn weighted_benefits(benefits: &ActorBenefits, weights: &NeedWeights) -> i64 {
    weighted(benefits.survival_margin, weights.survival)
        + weighted(benefits.mobility, weights.mobility)
        + weighted(benefits.space, weights.space)
        + weighted(benefits.food_access, weights.food)
        + weighted(benefits.growth, weights.growth)
        + weighted(benefits.territory, weights.territory)
        + weighted(benefits.dominance, weights.dominance)
        + weighted(benefits.hunting, weights.hunting)
        + weighted(benefits.pressure, weights.pressure)
        + weighted(benefits.stability, weights.stability)
}

fn weighted_harms(harms: &ActorHarms, weights: &NeedWeights) -> i64 {
    weighted(harms.dead_end, weights.survival)
        + weighted(harms.constrained, weights.survival)
        + weighted(harms.low_mobility, weights.mobility)
        + weighted(harms.insufficient_space, weights.space)
        + weighted(harms.enclosure, weights.enclosure)
        + weighted(harms.border, weights.border)
        + weighted(harms.corner, weights.border)
        + weighted(harms.enemy_pin, weights.survival)
        + weighted(harms.starvation, weights.food)
        + weighted(harms.food_denial, weights.food)
        + weighted(harms.territory_loss, weights.territory)
        + weighted(harms.enemy_dominance, weights.dominance)
        + weighted(harms.instability, weights.stability)
}

fn weighted(value: i32, weight: u16) -> i64 {
    i64::from(value)
        .saturating_mul(i64::from(weight))
        .saturating_div(1000)
}

fn space_comfort_milli(metrics: &ActorMetrics) -> u16 {
    let ratio = metrics.space_to_length_milli;
    let ratio_score = match ratio {
        0..=1000 => 0,
        1001..=2000 => (ratio.saturating_sub(1000) / 2).min(500) as u16,
        2001..=4000 => (500 + ratio.saturating_sub(2000) / 4).min(1000) as u16,
        _ => 1000,
    };
    let reachable_score = ratio_milli(
        metrics.reachable_space.min(metrics.reachable_territory),
        metrics.reachable_territory.max(1),
    );

    average_milli(&[ratio_score, reachable_score])
}

fn food_access_milli(metrics: &ActorMetrics) -> u16 {
    let Some(distance) = metrics.best_food_distance else {
        return 0;
    };
    let distance_score = match distance {
        0 | 1 => 1000,
        2 => 850,
        3 => 700,
        4 => 550,
        5 => 400,
        6 => 300,
        _ => 200,
    };
    let claim_score = match metrics.best_food_claim_margin {
        Some(margin) if margin >= 2 => 1000,
        Some(1) => 850,
        Some(0) => 550,
        Some(-1) => 250,
        Some(_) => 100,
        None => 700,
    };
    let contest_score = if metrics.best_food_contested {
        500
    } else {
        1000
    };

    average_milli(&[distance_score, claim_score, contest_score])
}

fn food_denial_milli(metrics: &ActorMetrics) -> u16 {
    let claim_harm = match metrics.best_food_claim_margin {
        Some(margin) if margin < -1 => 900,
        Some(-1) => 700,
        Some(0) => 350,
        _ => 0,
    };
    let contested = if metrics.best_food_contested { 300 } else { 0 };
    claim_harm.max(contested)
}

fn territory_benefit_milli(metrics: &ActorMetrics) -> u16 {
    let exclusive_ratio = ratio_milli(metrics.exclusive_space, metrics.reachable_territory.max(1));
    let contested_ratio = ratio_milli(metrics.contested_space, metrics.reachable_territory.max(1));
    let food_control = u16::from(metrics.controlled_food.min(3)).saturating_mul(250);
    let contested_food = u16::from(metrics.contested_food.min(3)).saturating_mul(100);

    weighted_average(&[
        (metrics.control_ratio_milli, 450),
        (exclusive_ratio, 300),
        (1000_u16.saturating_sub(contested_ratio), 100),
        (food_control.saturating_add(contested_food).min(1000), 150),
    ])
}

fn territory_loss_milli(metrics: &ActorMetrics) -> u16 {
    let contested_ratio = ratio_milli(metrics.contested_space, metrics.reachable_territory.max(1));
    let losing = frontier_ratio(metrics.losing_frontier, metrics.winning_frontier);
    weighted_average(&[
        (1000_u16.saturating_sub(metrics.control_ratio_milli), 500),
        (contested_ratio, 200),
        (losing, 300),
    ])
}

fn dominance_benefit_milli(metrics: &ActorMetrics) -> u16 {
    let winning = frontier_ratio(metrics.winning_frontier, metrics.losing_frontier);
    let claims = count_pressure(metrics.dominance_claim_cells, 8);
    let frontier = count_pressure(metrics.dominance_frontier_cells, 6);
    let favorable = count_pressure(metrics.favorable_head_frontier, 4);

    weighted_average(&[
        (winning, 250),
        (claims, 200),
        (frontier, 250),
        (favorable, 300),
    ])
}

fn dominance_harm_milli(metrics: &ActorMetrics) -> u16 {
    let losing = frontier_ratio(metrics.losing_frontier, metrics.winning_frontier);
    weighted_average(&[
        (losing, 600),
        (1000_u16.saturating_sub(metrics.inward_control_milli), 400),
    ])
}

fn frontier_ratio(primary: u16, secondary: u16) -> u16 {
    ratio_milli(
        u32::from(primary),
        u32::from(primary)
            .saturating_add(u32::from(secondary))
            .max(1),
    )
}

fn count_pressure(value: u16, full_at: u16) -> u16 {
    u32::from(value)
        .saturating_mul(1000)
        .saturating_div(u32::from(full_at).max(1))
        .min(1000)
        .try_into()
        .unwrap_or(1000)
}

fn average_milli(values: &[u16]) -> u16 {
    if values.is_empty() {
        return 0;
    }

    values
        .iter()
        .fold(0_u32, |sum, value| sum.saturating_add(u32::from(*value)))
        .saturating_div(values.len().try_into().unwrap_or(1))
        .min(1000)
        .try_into()
        .unwrap_or(1000)
}

fn weighted_average(values: &[(u16, u16)]) -> u16 {
    values
        .iter()
        .fold(0_u32, |sum, (value, weight)| {
            sum.saturating_add(
                u32::from(*value)
                    .saturating_mul(u32::from(*weight))
                    .saturating_div(1000),
            )
        })
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
    use super::*;

    fn context() -> ActorContext {
        ActorContext {
            actor_id: "ours".to_string(),
            health_milli: 800,
            length: 8,
            enemy_count: 1,
            largest_enemy_length: 8,
            average_enemy_length: 8,
            lead_over_largest: 0,
            stronger_enemies: 0,
            equal_enemies: 1,
            weaker_enemies: 0,
            board_cells: 121,
            board_occupancy_milli: 150,
            free_space_milli: 850,
            body_density_milli: 70,
            food_density_milli: 20,
            food_per_snake_milli: 500,
            hazard_density_milli: 0,
            crowding_milli: 200,
            duel_milli: 1000,
            size_advantage_milli: 0,
            size_disadvantage_milli: 0,
        }
    }

    fn metrics() -> ActorMetrics {
        ActorMetrics {
            health_milli: 800,
            safe_moves: 3,
            safe_non_reverse_moves: 3,
            reachable_space: 30,
            space_to_length_milli: 3750,
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
            reachable_territory: 35,
            exclusive_space: 25,
            contested_space: 5,
            control_ratio_milli: 650,
            territory_share_milli: 300,
            controlled_food: 1,
            contested_food: 0,
            winning_frontier: 3,
            losing_frontier: 1,
            dominance_claim_cells: 2,
            dominance_frontier_cells: 1,
            favorable_head_frontier: 1,
            best_food_distance: Some(3),
            best_food_claim_margin: Some(2),
            best_food_contested: false,
            hunting_opportunity_milli: 300,
            pressure_opportunity_milli: 400,
        }
    }

    #[test]
    fn benefits_minus_harms_is_the_actor_net_value() {
        let evaluation = ActorEvaluation::from_metrics(context(), metrics());

        assert_eq!(
            evaluation.net,
            evaluation.benefit_total - evaluation.harm_total
        );
    }

    #[test]
    fn trapping_the_actor_reduces_net_value() {
        let safe = ActorEvaluation::from_metrics(context(), metrics());
        let mut trapped_metrics = metrics();
        trapped_metrics.safe_moves = 0;
        trapped_metrics.escape_frontier = 0;
        trapped_metrics.enclosure_risk = 3;
        trapped_metrics.space_to_length_milli = 900;

        let trapped = ActorEvaluation::from_metrics(context(), trapped_metrics);

        assert!(trapped.harm_total > safe.harm_total);
        assert!(trapped.net < safe.net);
    }

    #[test]
    fn better_food_claim_increases_benefit_when_other_facts_match() {
        let mut poor = metrics();
        poor.best_food_claim_margin = Some(-2);
        poor.best_food_contested = true;
        let mut strong = poor;
        strong.best_food_claim_margin = Some(3);
        strong.best_food_contested = false;

        let poor = ActorEvaluation::from_metrics(context(), poor);
        let strong = ActorEvaluation::from_metrics(context(), strong);

        assert!(strong.benefit_total > poor.benefit_total);
        assert!(strong.harm_total < poor.harm_total);
        assert!(strong.net > poor.net);
    }
}
