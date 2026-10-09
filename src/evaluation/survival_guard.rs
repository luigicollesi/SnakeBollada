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
    /// No loss is proven, but a larger opponent can threaten our sole exit.
    Constrained,
    /// An equal-or-larger rival can exploit a sole exit along the board edge.
    ForcedCorridor,
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
    let legal = mobility.deterministic_moves_for(state, &state.our_snake_id);
    if legal.is_empty() {
        return TrapAssessment::ProvenTrap;
    }
    let Some(head) = ours.head() else {
        return TrapAssessment::Unknown;
    };
    let only_exit = legal.iter().next().map(|direction| direction.apply(head));
    let near_wall = head.x == 0
        || head.y == 0
        || i64::from(head.x) + 1 == i64::from(state.width)
        || i64::from(head.y) + 1 == i64::from(state.height);
    let threatened = state
        .snakes
        .iter()
        .filter(|enemy| enemy.alive && enemy.id != ours.id && enemy.length() > ours.length())
        .filter_map(|enemy| enemy.head())
        .any(|enemy_head| {
            let Some(exit) = only_exit else {
                return false;
            };
            let distance_to_exit = (enemy_head.x - exit.x).abs() + (enemy_head.y - exit.y).abs();
            let distance_to_head = (enemy_head.x - head.x).abs() + (enemy_head.y - head.y).abs();
            if legal.len() == 1 {
                // A single escape corridor near an equal-or-larger opponent
                // is vulnerable to a forced pin, but not a proven death.
                distance_to_exit <= i32::try_from(ours.length().min(4)).unwrap_or(4) + 1
            } else {
                // An edge corridor with another nominal move can still be
                // contested if the stronger enemy is already at the exit.
                legal.len() == 2 && distance_to_head <= 3
            }
        });
    if near_wall && threatened {
        if legal.len() == 1 {
            TrapAssessment::ForcedCorridor
        } else {
            TrapAssessment::Constrained
        }
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
    fn larger_opponent_near_forced_edge_exit_marks_constraint_not_certain_death() {
        let mut state = board(
            vec![(10, 10), (9, 10), (9, 9)],
            vec![(7, 8), (7, 7), (8, 7), (8, 6), (7, 6)],
        );
        state.width = 11;
        state.height = 11;
        assert_eq!(assess_state(&state), TrapAssessment::ForcedCorridor);
    }

    #[test]
    fn stronger_enemy_near_top_edge_creates_a_contested_exit() {
        let mut board = board(
            vec![(5, 10), (4, 10), (4, 9), (4, 8)],
            vec![(5, 8), (5, 7), (4, 7), (3, 7), (2, 7), (1, 7)],
        );
        board.width = 11;
        board.height = 11;
        assert_eq!(assess_state(&board), TrapAssessment::Constrained);
    }

    #[test]
    fn shorter_opponent_does_not_make_single_exit_proven_trap() {
        let state = board(vec![(0, 0), (0, 1), (1, 1), (2, 1), (2, 0)], vec![(1, 3)]);
        assert_eq!(assess_state(&state), TrapAssessment::Viable);
    }

    #[test]
    fn equal_size_enemy_does_not_dominate_edge_lane() {
        let mut state = board(
            vec![(10, 10), (9, 10), (9, 9)],
            vec![(7, 8), (7, 7), (8, 7)],
        );
        state.width = 11;
        state.height = 11;
        assert_eq!(assess_state(&state), TrapAssessment::Viable);
    }

    #[test]
    fn terminal_win_does_not_need_more_moves() {
        let mut state = board(vec![(0, 0)], vec![(3, 3)]);
        state.snakes[1].alive = false;
        assert_eq!(assess_state(&state), TrapAssessment::Viable);
    }
}
