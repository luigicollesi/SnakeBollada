use crate::analysis::{BorderFobicAnalysis, EnclosureAnalysis, TerritoryAnalysis};
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::{ActorIndex, SimulatedGameState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActorUtilityMetrics {
    pub(crate) safe_non_reverse_moves: u8,
    pub(crate) enclosure_risk: u8,
    pub(crate) border_structural_risk_milli: u16,
    pub(crate) border_exposure_milli: u16,
    pub(crate) border_pin_risk_milli: u16,
    pub(crate) space_capacity_milli: u16,
    pub(crate) territory_control_milli: u16,
    pub(crate) food_potential_milli: u16,
}

impl ActorUtilityMetrics {
    pub(crate) fn from_parts(
        state: &SimulatedGameState,
        actor: ActorIndex,
        mobility: &MobilityAnalysis,
        territory: &TerritoryAnalysis,
        enclosure: &EnclosureAnalysis,
        border: &BorderFobicAnalysis,
    ) -> Option<Self> {
        let actor_snake = state.snake_at(actor).filter(|snake| snake.alive)?;
        let actor_id = actor_snake.id.as_str();
        let safe_moves = mobility.deterministic_moves_for(state, actor_id).len();
        let territory_snapshot = territory.for_actor(actor)?;
        let enclosure_snapshot = enclosure.for_snake(actor_id)?;
        let border_snapshot = border.for_snake(actor_id);

        Some(Self {
            safe_non_reverse_moves: safe_moves.min(3),
            enclosure_risk: enclosure_snapshot.risk.rank(),
            border_structural_risk_milli: border_snapshot
                .map_or(0, |snapshot| snapshot.structural_risk_milli),
            border_exposure_milli: border_snapshot.map_or(0, |snapshot| {
                if snapshot.head_edge_distance == 0 {
                    snapshot.preference_milli
                } else {
                    0
                }
            }),
            border_pin_risk_milli: border_snapshot
                .map_or(0, |snapshot| snapshot.enemy_pin_risk_milli),
            space_capacity_milli: space_capacity_milli(
                territory_snapshot.reachable_space,
                actor_snake.length(),
            ),
            territory_control_milli: territory.competitive_control_milli(actor),
            food_potential_milli: food_potential_milli(state, territory, actor),
        })
    }
}

fn food_potential_milli(
    state: &SimulatedGameState,
    territory: &TerritoryAnalysis,
    actor: ActorIndex,
) -> u16 {
    let mut candidates = state
        .food
        .iter()
        .filter_map(|food| {
            let own_distance = territory.distance_for_actor(actor, *food)?;
            let nearest_enemy = state
                .snakes
                .iter()
                .enumerate()
                .filter(|(index, snake)| snake.alive && ActorIndex::new(*index) != Some(actor))
                .filter_map(|(index, _)| {
                    territory.distance_for_actor(ActorIndex::new(index)?, *food)
                })
                .min();

            let proximity = 1000_u32.saturating_div(u32::from(own_distance).saturating_add(1));
            let claim_factor = match nearest_enemy {
                None => 1300_u32,
                Some(enemy_distance) if own_distance < enemy_distance => 1000_u32.saturating_add(
                    u32::from(enemy_distance.saturating_sub(own_distance))
                        .saturating_mul(100)
                        .min(400),
                ),
                Some(enemy_distance) if own_distance == enemy_distance => 650,
                Some(enemy_distance) => 400_u32.saturating_sub(
                    u32::from(own_distance.saturating_sub(enemy_distance))
                        .saturating_mul(80)
                        .min(300),
                ),
            };

            Some(proximity.saturating_mul(claim_factor).saturating_div(1000))
        })
        .collect::<Vec<_>>();

    candidates.sort_unstable_by(|left, right| right.cmp(left));

    let primary = candidates.first().copied().unwrap_or(0);
    let secondary = candidates
        .get(1)
        .copied()
        .unwrap_or(0)
        .saturating_mul(350)
        .saturating_div(1000);
    let tertiary = candidates
        .get(2)
        .copied()
        .unwrap_or(0)
        .saturating_mul(150)
        .saturating_div(1000);

    primary
        .saturating_add(secondary)
        .saturating_add(tertiary)
        .min(u32::from(u16::MAX))
        .try_into()
        .unwrap_or(u16::MAX)
}

fn space_capacity_milli(reachable_space: u32, length: usize) -> u16 {
    let length = u32::try_from(length).unwrap_or(u32::MAX).max(1);
    let ratio_milli = reachable_space.saturating_mul(1000).saturating_div(length);

    if ratio_milli <= 1000 {
        return 0;
    }

    ratio_milli
        .saturating_sub(1000)
        .min(2000)
        .saturating_mul(1000)
        .saturating_div(2000)
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
    use crate::analysis::TerritoryAnalysis;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
    use crate::Coord;

    fn snake(id: &str, head: Coord) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: vec![head],
            alive: true,
        }
    }

    fn state(food: Coord) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![food],
            hazards: vec![],
            snakes: vec![
                snake("ours", Coord { x: 2, y: 2 }),
                snake("enemy", Coord { x: 5, y: 5 }),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn space_capacity_tracks_reachable_space_relative_to_body_length() {
        assert_eq!(space_capacity_milli(10, 10), 0);
        assert_eq!(space_capacity_milli(15, 10), 250);
        assert_eq!(space_capacity_milli(20, 10), 500);
        assert_eq!(space_capacity_milli(30, 10), 1000);
        assert_eq!(space_capacity_milli(50, 10), 1000);
    }

    #[test]
    fn food_potential_prefers_food_we_can_claim() {
        let claimable = state(Coord { x: 2, y: 4 });
        let losing = state(Coord { x: 5, y: 4 });

        let claimable_territory = TerritoryAnalysis::from_state(&claimable);
        let losing_territory = TerritoryAnalysis::from_state(&losing);
        let ours = claimable.actor_index("ours").unwrap();

        let claimable_value = food_potential_milli(&claimable, &claimable_territory, ours);
        let losing_value = food_potential_milli(&losing, &losing_territory, ours);

        assert!(claimable_value > losing_value);
    }
}
