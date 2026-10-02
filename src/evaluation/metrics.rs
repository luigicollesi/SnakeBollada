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
    pub(crate) food_survival_pressure_milli: u16,
    pub(crate) health_pressure_milli: u16,
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
            food_survival_pressure_milli: food_survival_pressure_milli(state, territory, actor),
            health_pressure_milli: health_pressure_milli(
                actor_snake.health,
                state.rules.max_health,
            ),
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

fn health_pressure_milli(health: i32, max_health: i32) -> u16 {
    if max_health <= 0 {
        return 1000;
    }

    let clamped = health.clamp(0, max_health);
    let reserve_milli = i64::from(clamped)
        .saturating_mul(1000)
        .saturating_div(i64::from(max_health))
        .clamp(0, 1000) as u16;

    match reserve_milli {
        0..=100 => 1000,
        101..=200 => interpolate_pressure(reserve_milli, 100, 200, 1000, 700),
        201..=350 => interpolate_pressure(reserve_milli, 200, 350, 700, 350),
        351..=500 => interpolate_pressure(reserve_milli, 350, 500, 350, 120),
        501..=700 => interpolate_pressure(reserve_milli, 500, 700, 120, 0),
        _ => 0,
    }
}

fn interpolate_pressure(value: u16, x0: u16, x1: u16, y0: u16, y1: u16) -> u16 {
    let span = u32::from(x1.saturating_sub(x0)).max(1);
    let offset = u32::from(value.saturating_sub(x0).min(x1.saturating_sub(x0)));
    let drop = u32::from(y0.saturating_sub(y1))
        .saturating_mul(offset)
        .saturating_div(span);
    u32::from(y0).saturating_sub(drop).try_into().unwrap_or(y1)
}

fn food_survival_pressure_milli(
    state: &SimulatedGameState,
    territory: &TerritoryAnalysis,
    actor: ActorIndex,
) -> u16 {
    let Some(actor_snake) = state.snake_at(actor).filter(|snake| snake.alive) else {
        return 1000;
    };

    let health = actor_snake.health.max(0) as u32;
    if health == 0 {
        return 1000;
    }

    let claimable_eta = state
        .food
        .iter()
        .filter_map(|food| {
            let own_distance = territory.distance_for_actor(actor, *food)?;
            let contested_earlier_or_equal = state
                .snakes
                .iter()
                .enumerate()
                .filter(|(index, snake)| snake.alive && ActorIndex::new(*index) != Some(actor))
                .filter_map(|(index, enemy)| {
                    let enemy_actor = ActorIndex::new(index)?;
                    let enemy_distance = territory.distance_for_actor(enemy_actor, *food)?;
                    Some((enemy, enemy_distance))
                })
                .any(|(enemy, enemy_distance)| {
                    enemy_distance < own_distance
                        || (enemy_distance == own_distance
                            && enemy.length() >= actor_snake.length())
                });

            (!contested_earlier_or_equal).then_some(own_distance)
        })
        .min();

    let buffer = match claimable_eta {
        Some(eta) => i32::try_from(health)
            .unwrap_or(i32::MAX)
            .saturating_sub(i32::from(eta)),
        None => i32::try_from(health).unwrap_or(i32::MAX).saturating_sub(20),
    };

    runway_pressure_milli(buffer)
}

fn runway_pressure_milli(buffer_turns: i32) -> u16 {
    match buffer_turns {
        i32::MIN..=0 => 1000,
        1..=3 => 900,
        4..=6 => 800,
        7..=10 => 650,
        11..=15 => 450,
        16..=20 => 250,
        21..=30 => {
            let offset = u32::try_from(buffer_turns.saturating_sub(20)).unwrap_or(10);
            250_u32
                .saturating_sub(offset.saturating_mul(25))
                .try_into()
                .unwrap_or(0)
        }
        _ => 0,
    }
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
    fn health_pressure_tracks_current_health_reserve() {
        assert_eq!(health_pressure_milli(100, 100), 0);
        assert_eq!(health_pressure_milli(70, 100), 0);
        assert!(health_pressure_milli(35, 100) >= 350);
        assert!(health_pressure_milli(15, 100) >= 700);
        assert_eq!(health_pressure_milli(5, 100), 1000);
    }

    #[test]
    fn starvation_pressure_is_zero_with_large_runway() {
        assert_eq!(runway_pressure_milli(40), 0);
        assert_eq!(runway_pressure_milli(30), 0);
    }

    #[test]
    fn starvation_pressure_rises_as_food_runway_collapses() {
        assert!(runway_pressure_milli(15) < runway_pressure_milli(8));
        assert!(runway_pressure_milli(8) < runway_pressure_milli(3));
        assert_eq!(runway_pressure_milli(0), 1000);
    }

    #[test]
    fn claimable_near_food_reduces_low_health_survival_pressure() {
        let mut near = state(Coord { x: 2, y: 3 });
        near.snake_mut("ours").unwrap().health = 12;
        let mut far = state(Coord { x: 2, y: 6 });
        far.snake_mut("ours").unwrap().health = 12;

        let near_territory = TerritoryAnalysis::from_state(&near);
        let far_territory = TerritoryAnalysis::from_state(&far);
        let actor = near.actor_index("ours").unwrap();

        assert!(
            food_survival_pressure_milli(&near, &near_territory, actor)
                < food_survival_pressure_milli(&far, &far_territory, actor)
        );
    }

    #[test]
    fn enemy_claimed_food_does_not_relieve_starvation_pressure() {
        let mut claimable = state(Coord { x: 2, y: 4 });
        claimable.snake_mut("ours").unwrap().health = 18;

        let mut enemy_claimed = state(Coord { x: 5, y: 4 });
        enemy_claimed.snake_mut("ours").unwrap().health = 18;

        let claimable_territory = TerritoryAnalysis::from_state(&claimable);
        let enemy_territory = TerritoryAnalysis::from_state(&enemy_claimed);
        let actor = claimable.actor_index("ours").unwrap();

        assert!(
            food_survival_pressure_milli(&enemy_claimed, &enemy_territory, actor)
                > food_survival_pressure_milli(&claimable, &claimable_territory, actor)
        );
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
