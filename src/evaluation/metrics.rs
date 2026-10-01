use crate::analysis::{BorderFobicAnalysis, EnclosureAnalysis, StateAnalysis, TerritoryAnalysis};
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActorUtilityMetrics {
    pub(crate) safe_non_reverse_moves: u8,
    pub(crate) enclosure_risk: u8,
    pub(crate) border_structural_risk_milli: u16,
    pub(crate) territory_share_milli: u16,
    pub(crate) best_food_distance: Option<u16>,
}

impl ActorUtilityMetrics {
    pub(crate) fn from_parts(
        state: &SimulatedGameState,
        actor_id: &str,
        state_analysis: &StateAnalysis,
        mobility: &MobilityAnalysis,
        territory: &TerritoryAnalysis,
        enclosure: &EnclosureAnalysis,
        border: &BorderFobicAnalysis,
    ) -> Option<Self> {
        state.snake(actor_id).filter(|snake| snake.alive)?;
        let safe_moves = mobility.deterministic_moves_for(state, actor_id).len();
        let territory_snapshot = territory.for_snake(actor_id)?;
        let enclosure_snapshot = enclosure.for_snake(actor_id)?;
        let border_snapshot = border.for_snake(actor_id);

        Some(Self {
            safe_non_reverse_moves: safe_moves.min(3),
            enclosure_risk: enclosure_snapshot.risk.rank(),
            border_structural_risk_milli: border_snapshot
                .map_or(0, |snapshot| snapshot.structural_risk_milli),
            territory_share_milli: territory_share_milli(
                state,
                territory_snapshot.exclusive_space,
                territory_snapshot.contested_space,
            ),
            best_food_distance: nearest_food_distance(state, state_analysis, actor_id),
        })
    }
}

fn nearest_food_distance(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    actor_id: &str,
) -> Option<u16> {
    state
        .food
        .iter()
        .filter_map(|food| {
            let route = analysis.route_for(actor_id, *food)?;
            route.reachable.then_some(route.distance).flatten()
        })
        .min()
}

fn territory_share_milli(
    state: &SimulatedGameState,
    exclusive_space: u32,
    contested_space: u32,
) -> u16 {
    let board_cells = state.width.saturating_mul(state.height).max(1);
    let effective_control = exclusive_space.saturating_add(contested_space / 2);
    ratio_milli(effective_control, board_cells)
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
