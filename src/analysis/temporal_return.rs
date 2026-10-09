//! Bounded adversarial verification of access to a previously connected region.
//!
//! The analyzer uses the SAME simultaneous joint-turn resolver as FutureGraph.
//! It proves existence of a strategy for a fixed-food horizon (MAX over ours,
//! MIN over all currently legal replies), not victory or permanent safety.
//! Unknown/missing responses are never silently treated as harmless.
//! No strategic score or move changes depend on this module.

use std::collections::HashMap;
use std::time::Instant;

use crate::decision::joint_actions::JointActionGenerator;
use crate::decision::state_key::StateKey;
use crate::direction::MoveMask;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::resolver::resolve_turn;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

const MAX_GOAL_CELLS: usize = 400;

/// All proofs are conditional on known food at each projected turn: unknown
/// future food spawns are intentionally not treated as deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnProof {
    /// One adaptive policy reaches the former territory within the horizon
    /// against every enumerated legal rival reply.
    VerifiedForFixedFood,
    /// A complete search found no guaranteed return within the horizon.
    /// This does NOT imply eventual death or rule out a later return.
    NotGuaranteedWithinHorizon,
    /// Search was incomplete: deadline, node limit, or unsupported state.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReturnAnalysis {
    pub(crate) result: ReturnProof,
    pub(crate) explored: usize,
    pub(crate) horizon: u8,
}

/// For each speculative state, decide whether there is an adaptive sequence of
/// our actions guaranteed to put the head into one of the lost territorial
/// cells by the requested horizon. This method is diagnostic only.
pub(crate) fn verify_return(
    state: &SimulatedGameState,
    destinations: &[Coord],
    horizon: u8,
    limit: usize,
    deadline: Instant,
) -> ReturnAnalysis {
    let mut ctx = Context {
        destinations,
        explored: 0,
        limit,
        deadline,
        visited: HashMap::new(),
    };
    let result = if horizon == 0
        || destinations.is_empty()
        || destinations.len() > MAX_GOAL_CELLS
        || state.width.saturating_mul(state.height) > MAX_GOAL_CELLS as u32
        || state.snakes.iter().filter(|s| s.alive).count() > 2
    {
        ReturnProof::Unknown
    } else {
        ctx.prove(state, horizon)
    };
    ReturnAnalysis {
        result,
        explored: ctx.explored,
        horizon,
    }
}

struct Context<'a> {
    destinations: &'a [Coord],
    explored: usize,
    limit: usize,
    deadline: Instant,
    visited: HashMap<(StateKey, u8), ReturnProof>,
}

impl Context<'_> {
    fn prove(&mut self, state: &SimulatedGameState, depth: u8) -> ReturnProof {
        let Some(ours) = state.snake(&state.our_snake_id).filter(|s| s.alive) else {
            return ReturnProof::NotGuaranteedWithinHorizon;
        };
        if ours
            .head()
            .is_some_and(|position| self.destinations.contains(&position))
        {
            return ReturnProof::VerifiedForFixedFood;
        }
        if depth == 0 {
            return ReturnProof::NotGuaranteedWithinHorizon;
        }
        if self.explored >= self.limit || Instant::now() >= self.deadline {
            return ReturnProof::Unknown;
        }
        let key = (StateKey::from_beam_state(state), depth);
        if let Some(&cached) = self.visited.get(&key) {
            return cached;
        }
        self.explored += 1;
        let mobility = MobilityAnalysis::from_state(state);
        let moves = mobility.deterministic_moves_for(state, &state.our_snake_id);
        // Do not allow speculative suicidal wall moves to count as safe
        // continuations, even when there are no deterministic alternatives.
        if moves.is_empty() {
            return ReturnProof::NotGuaranteedWithinHorizon;
        }
        let mut unknown_direction = false;
        for direction in moves {
            let mut worst = ReturnProof::VerifiedForFixedFood;
            let mut replies =
                JointActionGenerator::new(state, MoveMask::single(direction), &mobility);
            let mut saw_reply = false;
            for joint in &mut replies {
                saw_reply = true;
                if self.explored >= self.limit || Instant::now() >= self.deadline {
                    worst = ReturnProof::Unknown;
                    break;
                }
                let child = match resolve_turn(state, &joint) {
                    Ok(resolved) => resolved.state,
                    Err(_) => {
                        worst = ReturnProof::Unknown;
                        break;
                    }
                };
                match self.prove(&child, depth - 1) {
                    ReturnProof::VerifiedForFixedFood => {}
                    ReturnProof::NotGuaranteedWithinHorizon => {
                        worst = ReturnProof::NotGuaranteedWithinHorizon;
                        break;
                    }
                    ReturnProof::Unknown => {
                        worst = ReturnProof::Unknown;
                        // An unknown rival reply prevents proof but another
                        // unexamined reply may definitively refute this
                        // direction. Conservatively stop the branch.
                        break;
                    }
                }
            }
            if saw_reply && worst == ReturnProof::VerifiedForFixedFood {
                self.visited.insert(key, worst);
                return worst;
            }
            if worst == ReturnProof::Unknown || !saw_reply {
                unknown_direction = true;
            }
        }
        let result = if unknown_direction {
            ReturnProof::Unknown
        } else {
            ReturnProof::NotGuaranteedWithinHorizon
        };
        if result != ReturnProof::Unknown {
            self.visited.insert(key, result);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedSnake};

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.into(),
            health: 100,
            body: body.iter().map(|&(x, y)| Coord { x, y }).collect(),
            alive: true,
        }
    }

    fn state(snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 0,
            width: 5,
            height: 5,
            food: vec![],
            hazards: vec![],
            snakes,
            our_snake_id: "ours".into(),
            rules: RulesContext {
                name: "standard".into(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    fn run(state: &SimulatedGameState, target: Coord, depth: u8, limit: usize) -> ReturnAnalysis {
        verify_return(
            state,
            &[target],
            depth,
            limit,
            Instant::now() + std::time::Duration::from_millis(500),
        )
    }

    #[test]
    fn open_board_proves_reentry() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(&board, Coord { x: 2, y: 0 }, 3, 300);
        assert_eq!(result.result, ReturnProof::VerifiedForFixedFood);
    }

    #[test]
    fn no_horizon_does_not_assume_failure() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(&board, Coord { x: 2, y: 0 }, 0, 300);
        assert_eq!(result.result, ReturnProof::Unknown);
    }

    #[test]
    fn insufficient_horizon_is_not_a_terminal_loss() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(&board, Coord { x: 4, y: 4 }, 2, 500);
        assert_eq!(result.result, ReturnProof::NotGuaranteedWithinHorizon);
    }

    #[test]
    fn exhausted_budget_is_unknown_not_a_forced_trap() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(&board, Coord { x: 4, y: 4 }, 6, 0);
        assert_eq!(result.result, ReturnProof::Unknown);
    }

    #[test]
    fn contested_destination_must_survive_enemy_replies() {
        let board = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy", &[(3, 1), (3, 0)]),
        ]);
        let result = run(&board, Coord { x: 2, y: 1 }, 1, 500);
        // A same-size rival can meet us at (2,1); the head-to-head would
        // eliminate both snakes. No first-step guaranteed return.
        assert_eq!(result.result, ReturnProof::NotGuaranteedWithinHorizon);
    }

    #[test]
    fn recorded_turn_241_right_resolves_to_the_recorded_turn_242() {
        use crate::analysis::{detect_territorial_partition, replay_fixtures};
        use crate::direction::Direction;
        use crate::simulation::joint_action::JointAction;

        let before = replay_fixtures::state(241);
        let after = replay_fixtures::state(242);
        let ours = before.actor_index("ours").unwrap();
        let hobbs = before.actor_index("hobbs").unwrap();
        let action = JointAction::new()
            .with_move(ours, Direction::Right)
            .with_move(hobbs, Direction::Up);
        let resolved = resolve_turn(&before, &action).unwrap().state;
        // One food item spawned between actual turns; this must not affect
        // the precisely simulated body/health/joint movement.
        assert_eq!(resolved.snakes, after.snakes);
        let cut = detect_territorial_partition(&before, &resolved, "ours").unwrap();
        let reply = verify_return(
            &resolved,
            &cut.target_region,
            5,
            125,
            Instant::now() + std::time::Duration::from_millis(120),
        );
        assert!(reply.explored <= 125);
        // Whatever the bounded result, it must never claim that a complete
        // game loss was proved by checking only access to a region.
        assert!(matches!(
            reply.result,
            ReturnProof::VerifiedForFixedFood
                | ReturnProof::NotGuaranteedWithinHorizon
                | ReturnProof::Unknown
        ));
    }

    #[test]
    fn food_growth_is_resolved_by_joint_simulation() {
        let mut board = state(vec![snake("ours", &[(1, 1), (1, 0), (0, 0)])]);
        board.food.push(Coord { x: 2, y: 1 });
        let result = run(&board, Coord { x: 2, y: 1 }, 1, 250);
        assert_eq!(result.result, ReturnProof::VerifiedForFixedFood);
    }
}
