//! Goal-specific return verification using the persistent FutureGraph.
//!
//! The analysis explores the SAME joint-action edges as the main Minimax;
//! it never owns a second simulation tree, resolver, or transposition index.
//! The result is conditional on the graph's known-food forecast and is NOT
//! proof of game survival, permanent territory ownership, or eventual victory.

use std::collections::HashMap;
use std::time::Instant;

use crate::search::budget::SearchBudget;
use crate::search::forecast::ForecastCertainty;
use crate::search::graph::{FutureGraph, NodeId, ResponseCoverage, ResponseLookup};
use crate::Coord;

const MAX_GOAL_CELLS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnProof {
    /// An adaptive strategy reaches the region for every legal simultaneous
    /// enemy response *within the simulated, fixed-food scenario*.
    VerifiedForFixedFood,
    /// Complete reasoning refutes a guaranteed return within this horizon.
    /// This does NOT imply death, or even failure to return after the horizon.
    NotGuaranteedWithinHorizon,
    /// Partial expansion, unavailable time/nodes, or otherwise incomplete.
    Unknown,
}

/// Only the root engine may authorize new graph edges, and both modes share
/// one response enumerator. Shadow uses ReadOnly to avoid consuming search
/// depth or influencing the persistent search tree after a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReturnExpansion {
    ReadOnly,
    #[cfg_attr(not(test), allow(dead_code))]
    WithinBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReturnAnalysis {
    pub(crate) result: ReturnProof,
    pub(crate) explored: usize,
    pub(crate) horizon: u8,
    /// A simulated transition may omit food spawned randomly in the future;
    /// an affirmative result is consequently always conditional.
    pub(crate) provisional_food: bool,
}

/// Assess whether our head has a guaranteed route to any destination cell.
/// Reuses already-expanded FutureGraph edges and can optionally request
/// additional edges via FutureGraph::next_response under a bounded budget.
///
/// Never infer legal-reply coverage merely from known_responses(). A branch
/// with a missing rival response cannot establish a verified return.
pub(crate) fn verify_return(
    graph: &mut FutureGraph,
    start: NodeId,
    destinations: &[Coord],
    horizon: u8,
    limit: usize,
    deadline: Instant,
    expansion: ReturnExpansion,
    budget: &SearchBudget,
) -> ReturnAnalysis {
    let mut context = Context {
        destinations,
        explored: 0,
        limit,
        deadline,
        expansion,
        visited: HashMap::new(),
        provisional_food: false,
    };
    let state = &graph.node(start).state;
    let result = if horizon == 0
        || destinations.is_empty()
        || destinations.len() > MAX_GOAL_CELLS
        || state.width.saturating_mul(state.height) > MAX_GOAL_CELLS as u32
        || state.snakes.iter().filter(|snake| snake.alive).count() > 2
    {
        ReturnProof::Unknown
    } else {
        context.prove(
            graph,
            start,
            horizon,
            ForecastCertainty::Deterministic,
            budget,
        )
    };
    ReturnAnalysis {
        result,
        explored: context.explored,
        horizon,
        provisional_food: context.provisional_food,
    }
}

struct Context<'a> {
    destinations: &'a [Coord],
    explored: usize,
    limit: usize,
    deadline: Instant,
    expansion: ReturnExpansion,
    // The target region is fixed for the whole query. Node IDs remain stable
    // during append-only expansion; no compact/reroot runs inside the query.
    visited: HashMap<(NodeId, u8, ForecastCertainty), ReturnProof>,
    provisional_food: bool,
}

impl Context<'_> {
    fn out_of_budget(&self, budget: &SearchBudget) -> bool {
        self.explored >= self.limit || Instant::now() >= self.deadline || budget.expired()
    }

    fn prove(
        &mut self,
        graph: &mut FutureGraph,
        node: NodeId,
        depth: u8,
        certainty: ForecastCertainty,
        budget: &SearchBudget,
    ) -> ReturnProof {
        let state = &graph.node(node).state;
        let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
            return ReturnProof::NotGuaranteedWithinHorizon;
        };
        if ours
            .head()
            .is_some_and(|position| self.destinations.contains(&position))
        {
            return ReturnProof::VerifiedForFixedFood;
        }
        if depth == 0 || graph.node(node).is_terminal() {
            return ReturnProof::NotGuaranteedWithinHorizon;
        }
        // Retention prunes sibling actions and may falsely mark their lazy
        // generators exhausted. Do not use such nodes as proof substrates.
        if graph.responses_retained_after_decision(node) || self.out_of_budget(budget) {
            return ReturnProof::Unknown;
        }
        let key = (node, depth, certainty);
        if let Some(&cached) = self.visited.get(&key) {
            return cached;
        }

        self.explored += 1;
        let directions = graph.available_directions(node);
        if directions.is_empty() {
            return ReturnProof::NotGuaranteedWithinHorizon;
        }
        let mut unknown_direction = false;
        for direction in directions {
            if self.out_of_budget(budget) {
                return ReturnProof::Unknown;
            }
            let known = graph.known_responses(node, direction);
            let mut index = 0_usize;
            let mut saw_reply = false;
            let mut unknown_reply = false;
            let mut refuted = false;
            let mut exhausted =
                graph.response_coverage(node, direction) == ResponseCoverage::Complete;

            loop {
                if self.out_of_budget(budget) {
                    unknown_reply = true;
                    break;
                }
                let edge = if let Some(edge) = known.get(index) {
                    Some(edge.clone())
                } else if exhausted || self.expansion == ReturnExpansion::ReadOnly {
                    None
                } else {
                    match graph.next_response(node, direction, index, budget) {
                        Ok(ResponseLookup::Edge(edge)) => Some(edge),
                        Ok(ResponseLookup::Exhausted) => {
                            exhausted = true;
                            None
                        }
                        Ok(ResponseLookup::Deadline) | Err(_) => {
                            unknown_reply = true;
                            None
                        }
                    }
                };
                let Some(edge) = edge else {
                    break;
                };
                index += 1;
                saw_reply = true;
                let child_certainty = certainty.after(edge.forecast_delta);
                if child_certainty.is_provisional() {
                    self.provisional_food = true;
                }
                match self.prove(graph, edge.child, depth - 1, child_certainty, budget) {
                    ReturnProof::VerifiedForFixedFood => {}
                    ReturnProof::NotGuaranteedWithinHorizon => {
                        // A single refuting enemy reply is sufficient even if
                        // other replies have not been expanded.
                        refuted = true;
                        break;
                    }
                    ReturnProof::Unknown => {
                        // Another reply may refute this direction, but budget
                        // and node limits make this branch inconclusive.
                        unknown_reply = true;
                        break;
                    }
                }
            }
            if refuted {
                continue;
            }
            // The lazy generator is *complete only after Exhausted*. Observing
            // all cached replies is not enough to certify a guaranteed return.
            if saw_reply
                && !unknown_reply
                && (exhausted
                    || graph.response_coverage(node, direction) == ResponseCoverage::Complete)
            {
                self.visited.insert(key, ReturnProof::VerifiedForFixedFood);
                return ReturnProof::VerifiedForFixedFood;
            }
            unknown_direction = true;
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
    use std::time::Duration;

    use super::*;
    use crate::analysis::{detect_territorial_partition, replay_fixtures};
    use crate::direction::Direction;
    use crate::search::forecast::FoodForecastPolicy;
    use crate::simulation::joint_action::JointAction;
    use crate::simulation::resolver::resolve_turn;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};

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

    fn run(state: SimulatedGameState, target: Coord, depth: u8, limit: usize) -> ReturnAnalysis {
        let mut graph = FutureGraph::new_beam(state);
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let root = graph.root();
        verify_return(
            &mut graph,
            root,
            &[target],
            depth,
            limit,
            Instant::now() + Duration::from_millis(750),
            ReturnExpansion::WithinBudget,
            &budget,
        )
    }

    #[test]
    fn open_board_proves_reentry_through_shared_graph() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(board, Coord { x: 2, y: 0 }, 3, 600);
        assert_eq!(result.result, ReturnProof::VerifiedForFixedFood);
    }

    #[test]
    fn no_horizon_is_unknown() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(board, Coord { x: 2, y: 0 }, 0, 300);
        assert_eq!(result.result, ReturnProof::Unknown);
    }

    #[test]
    fn insufficient_horizon_is_not_a_terminal_loss() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(board, Coord { x: 4, y: 4 }, 2, 600);
        assert_eq!(result.result, ReturnProof::NotGuaranteedWithinHorizon);
    }

    #[test]
    fn exhausted_budget_never_claims_a_trap() {
        let board = state(vec![snake("ours", &[(0, 0)])]);
        let result = run(board, Coord { x: 4, y: 4 }, 6, 0);
        assert_eq!(result.result, ReturnProof::Unknown);
    }

    #[test]
    fn partial_replies_cannot_prove_return() {
        let board = state(vec![snake("ours", &[(1, 1)]), snake("enemy", &[(4, 4)])]);
        let mut graph = FutureGraph::new_beam(board);
        let budget = SearchBudget::for_duration(Duration::from_secs(1));
        let root = graph.root();
        let result = verify_return(
            &mut graph,
            root,
            &[Coord { x: 2, y: 1 }],
            1,
            300,
            Instant::now() + Duration::from_millis(100),
            ReturnExpansion::ReadOnly,
            &budget,
        );
        assert_eq!(result.result, ReturnProof::Unknown);
        assert_eq!(graph.node_count(), 1);
        // Shared expansion, not our own resolver, supplies the exact replies.
        assert_eq!(
            run(
                state(vec![snake("ours", &[(1, 1)]), snake("enemy", &[(4, 4)])]),
                Coord { x: 2, y: 1 },
                1,
                600
            )
            .result,
            ReturnProof::VerifiedForFixedFood,
        );
    }

    #[test]
    fn contested_destination_requires_all_adversarial_replies() {
        let board = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy", &[(3, 1), (3, 0)]),
        ]);
        let result = run(board, Coord { x: 2, y: 1 }, 1, 600);
        assert_eq!(result.result, ReturnProof::NotGuaranteedWithinHorizon);
    }

    #[test]
    fn food_growth_is_resolved_by_shared_graph() {
        let mut board = state(vec![snake("ours", &[(1, 1), (1, 0), (0, 0)])]);
        board.food.push(Coord { x: 2, y: 1 });
        let result = run(board, Coord { x: 2, y: 1 }, 1, 400);
        assert_eq!(result.result, ReturnProof::VerifiedForFixedFood);
    }

    #[test]
    fn provisional_food_is_propagated_across_graph_edges() {
        let board = state(vec![snake("ours", &[(1, 1)])]);
        let forecast = FoodForecastPolicy {
            spawn_chance_percent: 10,
            minimum_food: 0,
        };
        let mut graph = FutureGraph::new_beam_with_forecast(board, forecast);
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let root = graph.root();
        let result = verify_return(
            &mut graph,
            root,
            &[Coord { x: 2, y: 1 }],
            1,
            500,
            Instant::now() + Duration::from_millis(600),
            ReturnExpansion::WithinBudget,
            &budget,
        );
        assert_eq!(result.result, ReturnProof::VerifiedForFixedFood);
        assert!(result.provisional_food);
    }

    #[test]
    fn recorded_241_242_turn_uses_same_joint_resolver_and_shared_graph() {
        let before = replay_fixtures::state(241);
        let after = replay_fixtures::state(242);
        let ours = before.actor_index("ours").unwrap();
        let hobbs = before.actor_index("hobbs").unwrap();
        let action = JointAction::new()
            .with_move(ours, Direction::Right)
            .with_move(hobbs, Direction::Up);
        let resolved = resolve_turn(&before, &action).unwrap().state;
        assert_eq!(resolved.snakes, after.snakes);
        let cut = detect_territorial_partition(&before, &resolved, "ours").unwrap();
        let mut graph = FutureGraph::new_beam(resolved);
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let root = graph.root();
        let report = verify_return(
            &mut graph,
            root,
            &cut.target_region,
            5,
            125,
            Instant::now() + Duration::from_millis(200),
            ReturnExpansion::WithinBudget,
            &budget,
        );
        assert!(report.explored <= 125);
        assert!(matches!(
            report.result,
            ReturnProof::VerifiedForFixedFood
                | ReturnProof::NotGuaranteedWithinHorizon
                | ReturnProof::Unknown
        ));
    }

    #[test]
    fn retained_root_cannot_claim_complete_opponent_coverage() {
        let board = state(vec![snake("ours", &[(1, 1)])]);
        let mut graph = FutureGraph::new_beam(board);
        graph.retain_chosen_direction(Direction::Right);
        let budget = SearchBudget::for_duration(Duration::from_secs(1));
        let root = graph.root();
        let result = verify_return(
            &mut graph,
            root,
            &[Coord { x: 2, y: 1 }],
            1,
            300,
            Instant::now() + Duration::from_millis(100),
            ReturnExpansion::ReadOnly,
            &budget,
        );
        assert_eq!(result.result, ReturnProof::Unknown);
    }
}
