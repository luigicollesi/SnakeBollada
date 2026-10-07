#![allow(dead_code)]

use crate::evaluation::{ActorSnapshot, ActorVec};
use crate::simulation::state::{ActorIndex, SimulatedGameState};

use super::TerritoryAnalysis;

const TERRITORY_WEIGHT: i64 = 350;
const LENGTH_WEIGHT: i64 = 200;
const MOBILITY_WEIGHT: i64 = 200;
const ESCAPE_WEIGHT: i64 = 250;
const COMPONENT_BUDGET: i64 = 1000;

const PRESSURE_START_MILLI: u16 = 300;
const DOMINANCE_START_MILLI: u16 = 500;
const CLOSURE_START_MILLI: u16 = 750;
const CLOSURE_MAX_SAFE_MOVES: u8 = 2;
const CLOSURE_MAX_ESCAPE_FRONTIER: u8 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum DominationPhase {
    #[default]
    Neutral,
    Pressure,
    Dominance,
    Closure,
}

impl DominationPhase {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Pressure => "pressure",
            Self::Dominance => "dominance",
            Self::Closure => "closure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DominationSnapshot {
    pub(crate) target: ActorIndex,
    pub(crate) territory_advantage_milli: i16,
    pub(crate) length_security_milli: i16,
    pub(crate) mobility_pressure_milli: i16,
    pub(crate) escape_pressure_milli: i16,
    pub(crate) progress_milli: u16,
    pub(crate) phase: DominationPhase,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DominationAnalysis {
    pairs: ActorVec<ActorVec<DominationSnapshot>>,
}

impl DominationAnalysis {
    pub(crate) fn from_parts(
        state: &SimulatedGameState,
        territory: &TerritoryAnalysis,
        actor_snapshots: &ActorVec<ActorSnapshot>,
    ) -> Self {
        let mut pairs = ActorVec::with_capacity(state.snakes.len());

        for (actor_index, actor_snake) in state.snakes.iter().enumerate() {
            if !actor_snake.alive {
                continue;
            }
            let Some(actor) = ActorIndex::new(actor_index) else {
                continue;
            };
            let Some(actor_snapshot) = actor_snapshots.get(actor) else {
                continue;
            };
            let Some(actor_territory) = territory.for_actor(actor) else {
                continue;
            };

            let mut targets = ActorVec::with_capacity(state.snakes.len());
            for (target_index, target_snake) in state.snakes.iter().enumerate() {
                if !target_snake.alive || target_index == actor_index {
                    continue;
                }
                let Some(target) = ActorIndex::new(target_index) else {
                    continue;
                };
                let Some(target_snapshot) = actor_snapshots.get(target) else {
                    continue;
                };
                let Some(target_territory) = territory.for_actor(target) else {
                    continue;
                };

                let snapshot = pair_snapshot(
                    target,
                    actor_snake.length(),
                    target_snake.length(),
                    actor_snapshot,
                    target_snapshot,
                    actor_territory.escape_frontier,
                    target_territory.escape_frontier,
                );
                targets.insert(target, snapshot);
            }
            pairs.insert(actor, targets);
        }

        Self { pairs }
    }

    pub(crate) fn against(
        &self,
        actor: ActorIndex,
        target: ActorIndex,
    ) -> Option<&DominationSnapshot> {
        self.pairs.get(actor)?.get(target)
    }

    pub(crate) fn best_target_for(&self, actor: ActorIndex) -> Option<&DominationSnapshot> {
        self.pairs
            .get(actor)?
            .iter()
            .map(|(_, snapshot)| snapshot)
            .max_by_key(|snapshot| {
                (
                    snapshot.progress_milli,
                    snapshot.mobility_pressure_milli,
                    snapshot.escape_pressure_milli,
                )
            })
    }
}

fn pair_snapshot(
    target: ActorIndex,
    actor_length: usize,
    target_length: usize,
    actor: &ActorSnapshot,
    target_snapshot: &ActorSnapshot,
    actor_escape_frontier: u8,
    target_escape_frontier: u8,
) -> DominationSnapshot {
    let territory_advantage_milli = signed_component(
        i32::from(actor.metrics.territory_control_milli)
            .saturating_sub(i32::from(target_snapshot.metrics.territory_control_milli)),
    );
    let length_security_milli = capped_length_security(actor_length, target_length);
    let mobility_pressure_milli = signed_component(
        i32::from(mobility_constriction(target_snapshot.metrics.safe_non_reverse_moves))
            .saturating_sub(i32::from(mobility_constriction(
                actor.metrics.safe_non_reverse_moves,
            ))),
    );
    let escape_pressure_milli = signed_component(
        i32::from(escape_constriction(target_escape_frontier))
            .saturating_sub(i32::from(escape_constriction(actor_escape_frontier))),
    );

    let signed_progress = i64::from(territory_advantage_milli)
        .saturating_mul(TERRITORY_WEIGHT)
        .saturating_add(i64::from(length_security_milli).saturating_mul(LENGTH_WEIGHT))
        .saturating_add(i64::from(mobility_pressure_milli).saturating_mul(MOBILITY_WEIGHT))
        .saturating_add(i64::from(escape_pressure_milli).saturating_mul(ESCAPE_WEIGHT))
        .saturating_div(COMPONENT_BUDGET);

    let progress_milli = signed_progress
        .clamp(0, 1000)
        .try_into()
        .unwrap_or(1000);
    let closure_supported = target_snapshot.metrics.safe_non_reverse_moves
        <= CLOSURE_MAX_SAFE_MOVES
        || target_escape_frontier <= CLOSURE_MAX_ESCAPE_FRONTIER;
    let phase = phase_for(progress_milli, closure_supported);

    DominationSnapshot {
        target,
        territory_advantage_milli,
        length_security_milli,
        mobility_pressure_milli,
        escape_pressure_milli,
        progress_milli,
        phase,
    }
}

fn phase_for(progress_milli: u16, closure_supported: bool) -> DominationPhase {
    if progress_milli >= CLOSURE_START_MILLI && closure_supported {
        DominationPhase::Closure
    } else if progress_milli >= DOMINANCE_START_MILLI {
        DominationPhase::Dominance
    } else if progress_milli >= PRESSURE_START_MILLI {
        DominationPhase::Pressure
    } else {
        DominationPhase::Neutral
    }
}

fn capped_length_security(actor_length: usize, target_length: usize) -> i16 {
    let diff = i64::try_from(actor_length)
        .unwrap_or(i64::MAX)
        .saturating_sub(i64::try_from(target_length).unwrap_or(i64::MAX))
        .clamp(-3, 3);
    match diff {
        -3 => -1000,
        -2 => -667,
        -1 => -333,
        0 => 0,
        1 => 333,
        2 => 667,
        _ => 1000,
    }
}

fn mobility_constriction(moves: u8) -> u16 {
    match moves {
        0 => 1000,
        1 => 700,
        2 => 250,
        _ => 0,
    }
}

fn escape_constriction(frontier: u8) -> u16 {
    match frontier {
        0 => 1000,
        1 => 800,
        2 => 600,
        3 => 400,
        4 => 200,
        _ => 0,
    }
}

fn signed_component(value: i32) -> i16 {
    value.clamp(-1000, 1000).try_into().unwrap_or_else(|_| {
        if value.is_negative() {
            -1000
        } else {
            1000
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::evaluation::{ActorUtilityMetrics, StrategicWeights};

    use super::*;

    fn actor_snapshot(
        moves: u8,
        territory: u16,
    ) -> ActorSnapshot {
        ActorSnapshot::new(
            ActorUtilityMetrics {
                safe_non_reverse_moves: moves,
                enclosure_risk: 0,
                border_structural_risk_milli: 0,
                border_exposure_milli: 0,
                border_pin_risk_milli: 0,
                border_escape_pressure_milli: 0,
                space_capacity_milli: 800,
                territory_control_milli: territory,
                food_potential_milli: 0,
                growth_pressure_milli: 0,
                size_security_milli: 600,
                claimable_food_eta: None,
                food_survival_pressure_milli: 0,
                health_pressure_milli: 0,
            },
            StrategicWeights {
                food: 300,
                hunting: 500,
                survival: 200,
            },
        )
    }

    fn target() -> ActorIndex {
        ActorIndex::new(1).unwrap()
    }

    #[test]
    fn equal_position_is_neutral() {
        let ours = actor_snapshot(3, 500);
        let enemy = actor_snapshot(3, 500);

        let snapshot = pair_snapshot(target(), 5, 5, &ours, &enemy, 5, 5);

        assert_eq!(snapshot.progress_milli, 0);
        assert_eq!(snapshot.phase, DominationPhase::Neutral);
        assert_eq!(snapshot.territory_advantage_milli, 0);
        assert_eq!(snapshot.length_security_milli, 0);
    }

    #[test]
    fn length_security_saturates_at_three_squares() {
        assert_eq!(capped_length_security(8, 5), 1000);
        assert_eq!(capped_length_security(15, 5), 1000);
        assert_eq!(capped_length_security(5, 8), -1000);
    }

    #[test]
    fn constrained_enemy_drives_closure() {
        let ours = actor_snapshot(3, 800);
        let enemy = actor_snapshot(1, 200);

        let snapshot = pair_snapshot(target(), 8, 5, &ours, &enemy, 5, 1);

        assert!(snapshot.progress_milli >= CLOSURE_START_MILLI);
        assert_eq!(snapshot.phase, DominationPhase::Closure);
        assert!(snapshot.mobility_pressure_milli > 0);
        assert!(snapshot.escape_pressure_milli > 0);
    }

    #[test]
    fn strong_position_without_constriction_is_not_closure() {
        let ours = actor_snapshot(3, 1000);
        let enemy = actor_snapshot(3, 0);

        let snapshot = pair_snapshot(target(), 8, 5, &ours, &enemy, 5, 5);

        assert!(snapshot.progress_milli >= CLOSURE_START_MILLI);
        assert_eq!(snapshot.phase, DominationPhase::Dominance);
    }

    #[test]
    fn self_constriction_reduces_domination_progress() {
        let free = actor_snapshot(3, 650);
        let trapped = actor_snapshot(1, 650);
        let enemy = actor_snapshot(2, 350);

        let free_snapshot = pair_snapshot(target(), 7, 5, &free, &enemy, 5, 2);
        let trapped_snapshot = pair_snapshot(target(), 7, 5, &trapped, &enemy, 1, 2);

        assert!(free_snapshot.progress_milli > trapped_snapshot.progress_milli);
    }
}
