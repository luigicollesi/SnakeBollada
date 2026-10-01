use std::cmp::Reverse;

use crate::analysis::{
    BorderFobicAnalysis, EnclosureAnalysis, StateAnalysis, TacticalStateAnalysis, TerritoryAnalysis,
};
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActorMetrics {
    pub(crate) health_milli: u16,
    pub(crate) safe_moves: u8,
    pub(crate) reachable_space: u32,
    pub(crate) space_to_length_milli: u32,
    pub(crate) escape_frontier: u8,
    pub(crate) enclosure_risk: u8,
    pub(crate) useful_chokes: u8,
    pub(crate) boundary_support: u8,
    pub(crate) edge_distance: u16,
    pub(crate) body_on_edge: u16,
    pub(crate) body_near_edge: u16,
    pub(crate) leading_edge_chain: u16,
    pub(crate) inward_safe_moves: u8,
    pub(crate) corner_contact: bool,
    pub(crate) border_preference_milli: u16,
    pub(crate) border_structural_risk_milli: u16,
    pub(crate) inward_control_milli: u16,
    pub(crate) enemy_pin_risk_milli: u16,
    pub(crate) reachable_territory: u32,
    pub(crate) exclusive_space: u32,
    pub(crate) contested_space: u32,
    pub(crate) control_ratio_milli: u16,
    pub(crate) controlled_food: u8,
    pub(crate) contested_food: u8,
    pub(crate) winning_frontier: u16,
    pub(crate) losing_frontier: u16,
    pub(crate) dominance_claim_cells: u16,
    pub(crate) dominance_frontier_cells: u16,
    pub(crate) favorable_head_frontier: u16,
    pub(crate) best_food_distance: Option<u16>,
    pub(crate) best_food_claim_margin: Option<i16>,
    pub(crate) best_food_contested: bool,
    pub(crate) hunting_opportunity_milli: u16,
    pub(crate) pressure_opportunity_milli: u16,
}

impl ActorMetrics {
    pub(crate) fn from_parts(
        state: &SimulatedGameState,
        actor_id: &str,
        state_analysis: &StateAnalysis,
        tactical: &TacticalStateAnalysis,
        territory: &TerritoryAnalysis,
        enclosure: &EnclosureAnalysis,
        border: &BorderFobicAnalysis,
    ) -> Option<Self> {
        let actor = state.snake(actor_id).filter(|snake| snake.alive)?;
        let health_milli = actor
            .health
            .max(0)
            .saturating_mul(1000)
            .saturating_div(state.rules.max_health.max(1))
            .clamp(0, 1000)
            .try_into()
            .unwrap_or(1000);

        let (safe_moves, reachable_space) = actor_mobility(state, tactical, actor_id);
        let territory_snapshot = territory.for_snake(actor_id)?;
        let competitive = territory.competitive_for_snake(actor_id);
        let enclosure_snapshot = enclosure.for_snake(actor_id)?;
        let border_snapshot = border.for_snake(actor_id);

        let (best_food_distance, best_food_claim_margin, best_food_contested) =
            best_food(state, state_analysis, actor_id);

        let (hunting_opportunity_milli, pressure_opportunity_milli) =
            offensive_opportunity(state, actor_id, tactical, territory, enclosure);

        Some(Self {
            health_milli,
            safe_moves,
            reachable_space,
            space_to_length_milli: territory_snapshot.space_to_length_milli(actor.length()),
            escape_frontier: territory_snapshot.escape_frontier,
            enclosure_risk: enclosure_snapshot.risk.rank(),
            useful_chokes: territory_snapshot
                .useful_chokes
                .len()
                .try_into()
                .unwrap_or(u8::MAX),
            boundary_support: territory_snapshot.boundary_support(),
            edge_distance: territory_snapshot.edge_distance,
            body_on_edge: border_snapshot.map_or(0, |snapshot| snapshot.body_on_edge),
            body_near_edge: border_snapshot.map_or(0, |snapshot| snapshot.body_near_edge),
            leading_edge_chain: border_snapshot.map_or(0, |snapshot| snapshot.leading_edge_chain),
            inward_safe_moves: border_snapshot.map_or(0, |snapshot| snapshot.inward_safe_moves),
            corner_contact: border_snapshot.is_some_and(|snapshot| snapshot.corner_contact),
            border_preference_milli: border_snapshot
                .map_or(0, |snapshot| snapshot.preference_milli),
            border_structural_risk_milli: border_snapshot
                .map_or(0, |snapshot| snapshot.structural_risk_milli),
            inward_control_milli: border_snapshot
                .map_or(1000, |snapshot| snapshot.inward_control_milli),
            enemy_pin_risk_milli: border_snapshot
                .map_or(0, |snapshot| snapshot.enemy_pin_risk_milli),
            reachable_territory: territory_snapshot.reachable_space,
            exclusive_space: territory_snapshot.exclusive_space,
            contested_space: territory_snapshot.contested_space,
            control_ratio_milli: competitive.map_or(0, |snapshot| snapshot.control_ratio_milli),
            controlled_food: competitive.map_or(0, |snapshot| snapshot.controlled_food),
            contested_food: competitive.map_or(0, |snapshot| snapshot.contested_food),
            winning_frontier: competitive.map_or(0, |snapshot| snapshot.winning_frontier),
            losing_frontier: competitive.map_or(0, |snapshot| snapshot.losing_frontier),
            dominance_claim_cells: competitive.map_or(0, |snapshot| snapshot.dominance_claim_cells),
            dominance_frontier_cells: competitive
                .map_or(0, |snapshot| snapshot.dominance_frontier_cells),
            favorable_head_frontier: competitive
                .map_or(0, |snapshot| snapshot.favorable_head_frontier),
            best_food_distance,
            best_food_claim_margin,
            best_food_contested,
            hunting_opportunity_milli,
            pressure_opportunity_milli,
        })
    }
}

fn actor_mobility(
    state: &SimulatedGameState,
    tactical: &TacticalStateAnalysis,
    actor_id: &str,
) -> (u8, u32) {
    if actor_id == state.our_snake_id {
        return (
            tactical.ours.safe_moves.len(),
            tactical.ours.best_reachable_space,
        );
    }

    tactical.enemies.get(actor_id).map_or((0, 0), |enemy| {
        let moves = if enemy.plausible_moves.is_empty() {
            enemy.legal_moves
        } else {
            enemy.plausible_moves
        };
        (moves.len(), enemy.best_reachable_space)
    })
}

fn best_food(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    actor_id: &str,
) -> (Option<u16>, Option<i16>, bool) {
    state
        .food
        .iter()
        .filter_map(|food| {
            let route = analysis.route_for(actor_id, *food)?;
            if !route.reachable {
                return None;
            }
            let distance = route.distance?;
            let claim = analysis.claim_for_actor(actor_id, *food);
            let margin = claim.as_ref().and_then(|claim| claim.claim_margin);
            let contested = claim.as_ref().is_some_and(|claim| claim.contested);
            let claim_class = match margin {
                Some(value) if value < 0 => 2,
                Some(0) => 1,
                _ => 0,
            };
            Some((
                (claim_class, distance, Reverse(margin.unwrap_or(i16::MAX))),
                distance,
                margin,
                contested,
            ))
        })
        .min_by_key(|candidate| candidate.0)
        .map_or((None, None, false), |(_, distance, margin, contested)| {
            (Some(distance), margin, contested)
        })
}

fn offensive_opportunity(
    state: &SimulatedGameState,
    actor_id: &str,
    tactical: &TacticalStateAnalysis,
    territory: &TerritoryAnalysis,
    enclosure: &EnclosureAnalysis,
) -> (u16, u16) {
    let Some(actor) = state.snake(actor_id).filter(|snake| snake.alive) else {
        return (0, 0);
    };

    let mut best_hunt = 0_u16;
    let mut best_pressure = 0_u16;

    for target in state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != actor_id)
    {
        let length_advantage = actor.length().saturating_sub(target.length());
        let length_pressure = if actor.length() <= target.length() {
            0
        } else {
            ratio_milli(length_advantage as u32, target.length().max(1) as u32)
        };
        let target_moves = actor_mobility(state, tactical, &target.id).0;
        let mobility_pressure =
            1000_u16.saturating_sub(u16::from(target_moves.min(4)).saturating_mul(250));
        let target_enclosure = enclosure
            .for_snake(&target.id)
            .map_or(0, |snapshot| {
                u16::from(snapshot.risk.rank()).saturating_mul(333)
            })
            .min(1000);
        let target_control_denial = territory
            .competitive_for_snake(&target.id)
            .map_or(0, |snapshot| {
                1000_u16.saturating_sub(snapshot.control_ratio_milli)
            });

        let pressure = weighted_milli(&[
            (mobility_pressure, 350),
            (target_enclosure, 350),
            (target_control_denial, 300),
        ]);
        let hunt = if length_advantage == 0 {
            0
        } else {
            weighted_milli(&[
                (length_pressure, 350),
                (mobility_pressure, 250),
                (target_enclosure, 250),
                (target_control_denial, 150),
            ])
        };

        best_pressure = best_pressure.max(pressure);
        best_hunt = best_hunt.max(hunt);
    }

    (best_hunt, best_pressure)
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

fn weighted_milli(parts: &[(u16, u16)]) -> u16 {
    parts
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
