//! Structural safety gate for the Hobbs decision flow.
//!
//! Survival is not a strategic score. Only a proved absence of legal,
//! non-terminal next moves marks a state as a trap. A corridor with one exit
//! remains viable; temporary tail occupancy is handled by MobilityAnalysis.

use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrapAssessment {
    Viable,
    ProvenTrap,
    Unknown,
}

pub(crate) fn assess_state(state: &SimulatedGameState) -> TrapAssessment {
    let Some(ours) = state.snake(&state.our_snake_id) else {
        return TrapAssessment::Unknown;
    };
    if !ours.alive {
        return TrapAssessment::Unknown;
    }
    if !state
        .snakes
        .iter()
        .any(|snake| snake.alive && snake.id != state.our_snake_id)
    {
        return TrapAssessment::Viable;
    }

    let mobility = MobilityAnalysis::from_state(state);
    if mobility
        .deterministic_moves_for(state, &state.our_snake_id)
        .is_empty()
    {
        TrapAssessment::ProvenTrap
    } else {
        TrapAssessment::Viable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedSnake};
    use crate::Coord;

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            body: body.iter().map(|&(x, y)| Coord { x, y }).collect(),
            alive: true,
            health: 80,
        }
    }

    fn board(ours: Vec<(i32, i32)>, enemy: Vec<(i32, i32)>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 2,
            width: 4,
            height: 4,
            food: vec![],
            hazards: vec![],
            snakes: vec![snake("ours", &ours), snake("enemy", &enemy)],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn boxed_head_with_no_vacating_tail_is_proven_trap() {
        let state = board(vec![(0, 0), (0, 1), (1, 1), (1, 0), (1, 0)], vec![(3, 3)]);
        assert_eq!(assess_state(&state), TrapAssessment::ProvenTrap);
    }

    #[test]
    fn one_escape_lane_is_not_a_trap() {
        let state = board(vec![(0, 0), (0, 1), (1, 1), (2, 1), (2, 0)], vec![(3, 3)]);
        assert_eq!(assess_state(&state), TrapAssessment::Viable);
    }

    #[test]
    fn vacating_tail_preserves_escape() {
        let state = board(vec![(0, 0), (0, 1), (1, 1), (1, 0)], vec![(3, 3)]);
        assert_eq!(assess_state(&state), TrapAssessment::Viable);
    }

    #[test]
    fn terminal_win_does_not_need_more_moves() {
        let mut state = board(vec![(0, 0)], vec![(3, 3)]);
        state.snakes[1].alive = false;
        assert_eq!(assess_state(&state), TrapAssessment::Viable);
    }
}
