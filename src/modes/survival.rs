#![allow(dead_code)]

use crate::analysis::TacticalStateAnalysis;
use crate::direction::Direction;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SurvivalCandidate {
    pub(crate) first_move: Direction,
    pub(crate) deterministic: bool,
    pub(crate) robust_safe: bool,
    pub(crate) immediate_reachable_space: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SurvivalModeOutput {
    pub(crate) candidates: Vec<SurvivalCandidate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurvivalStateSnapshot {
    pub(crate) alive: bool,
    pub(crate) deterministic_moves: u8,
    pub(crate) safe_moves: u8,
    pub(crate) reachable_space: u32,
}

impl SurvivalStateSnapshot {
    pub(crate) fn from_tactical(
        state: &SimulatedGameState,
        tactical: &TacticalStateAnalysis,
    ) -> Self {
        let alive = state
            .snake(&state.our_snake_id)
            .is_some_and(|snake| snake.alive);

        Self {
            alive,
            deterministic_moves: tactical.ours.deterministic_moves.len(),
            safe_moves: tactical.ours.safe_moves.len(),
            reachable_space: tactical.ours.best_reachable_space,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SurvivalRouteAssessment {
    pub(crate) died: bool,
    pub(crate) dead_end: bool,
    pub(crate) final_safe_moves: u8,
    pub(crate) min_safe_moves: u8,
    pub(crate) final_reachable_space: u32,
    pub(crate) min_reachable_space: u32,
    pub(crate) second_order_mobility: u32,
}

impl SurvivalRouteAssessment {
    pub(crate) fn from_snapshots(
        snapshots: &[SurvivalStateSnapshot],
        second_order_mobility: u32,
    ) -> Option<Self> {
        let first = *snapshots.first()?;
        let last = *snapshots.last()?;

        let died = snapshots.iter().any(|snapshot| !snapshot.alive);
        let dead_end = snapshots
            .iter()
            .any(|snapshot| snapshot.alive && snapshot.safe_moves == 0);

        Some(Self {
            died,
            dead_end,
            final_safe_moves: last.safe_moves,
            min_safe_moves: snapshots
                .iter()
                .filter(|snapshot| snapshot.alive)
                .map(|snapshot| snapshot.safe_moves)
                .min()
                .unwrap_or(first.safe_moves),
            final_reachable_space: last.reachable_space,
            min_reachable_space: snapshots
                .iter()
                .filter(|snapshot| snapshot.alive)
                .map(|snapshot| snapshot.reachable_space)
                .min()
                .unwrap_or(first.reachable_space),
            second_order_mobility,
        })
    }
}

pub(crate) fn analyze(
    state: &SimulatedGameState,
    tactical: &TacticalStateAnalysis,
) -> SurvivalModeOutput {
    let mobility = MobilityAnalysis::from_state(state);

    let candidates = tactical
        .ours
        .deterministic_moves
        .iter()
        .map(|direction| SurvivalCandidate {
            first_move: direction,
            deterministic: true,
            robust_safe: tactical.ours.safe_moves.contains(direction),
            immediate_reachable_space: mobility.reachable_space(
                state,
                &state.our_snake_id,
                direction,
            ),
        })
        .collect();

    SurvivalModeOutput { candidates }
}

#[cfg(test)]
mod tests {
    use crate::analysis::StateAnalysis;
    use crate::enemy::tracing::trace;
    use crate::forecast::ForecastCertainty;
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};
    use crate::Coord;

    use super::*;

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes,
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        }
    }

    fn tactical(state: &SimulatedGameState) -> TacticalStateAnalysis {
        let analysis = StateAnalysis::from_simulated(state);
        let tracing = trace(state, &analysis);
        TacticalStateAnalysis::from_state(state, &tracing)
    }

    #[test]
    fn candidate_keeps_deterministic_and_robust_safety_separate() {
        let state = state(vec![
            snake("ours", &[(2, 1), (1, 1), (1, 0)]),
            snake("enemy", &[(2, 3), (3, 3), (3, 2)]),
        ]);
        let tactical = tactical(&state);
        let output = analyze(&state, &tactical);
        let up = output
            .candidates
            .iter()
            .find(|candidate| candidate.first_move == Direction::Up)
            .unwrap();

        assert!(up.deterministic);
        assert!(!up.robust_safe);
    }

    #[test]
    fn route_assessment_tracks_worst_survival_point() {
        let snapshots = [
            SurvivalStateSnapshot {
                alive: true,
                deterministic_moves: 4,
                safe_moves: 4,
                reachable_space: 30,
            },
            SurvivalStateSnapshot {
                alive: true,
                deterministic_moves: 3,
                safe_moves: 2,
                reachable_space: 20,
            },
            SurvivalStateSnapshot {
                alive: true,
                deterministic_moves: 2,
                safe_moves: 1,
                reachable_space: 12,
            },
        ];

        let assessment = SurvivalRouteAssessment::from_snapshots(&snapshots, 5).unwrap();

        assert!(!assessment.died);
        assert!(!assessment.dead_end);
        assert_eq!(assessment.min_safe_moves, 1);
        assert_eq!(assessment.min_reachable_space, 12);
        assert_eq!(assessment.second_order_mobility, 5);
    }

    #[test]
    fn route_assessment_records_dead_end() {
        let snapshots = [
            SurvivalStateSnapshot {
                alive: true,
                deterministic_moves: 2,
                safe_moves: 2,
                reachable_space: 10,
            },
            SurvivalStateSnapshot {
                alive: true,
                deterministic_moves: 1,
                safe_moves: 0,
                reachable_space: 4,
            },
        ];

        let assessment = SurvivalRouteAssessment::from_snapshots(&snapshots, 0).unwrap();

        assert!(assessment.dead_end);
        assert_eq!(assessment.min_safe_moves, 0);
    }

    #[test]
    fn death_is_terminally_recorded() {
        let snapshots = [
            SurvivalStateSnapshot {
                alive: true,
                deterministic_moves: 2,
                safe_moves: 2,
                reachable_space: 10,
            },
            SurvivalStateSnapshot {
                alive: false,
                deterministic_moves: 0,
                safe_moves: 0,
                reachable_space: 0,
            },
        ];

        let assessment = SurvivalRouteAssessment::from_snapshots(&snapshots, 0).unwrap();

        assert!(assessment.died);
    }
}
