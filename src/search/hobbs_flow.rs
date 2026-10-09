//! Hobbs-style paranoid search over the existing FutureGraph.
//!
//! The route value comes from the reached state, not the sum of Food,
//! Hunting and Survival transition scores. Every enumerated enemy response
//! participates in MIN; opponent intent only affects expansion order.
//! Iterative deepening commits the previous complete depth on timeout.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::analysis::TemporalTerritory;
use crate::direction::Direction;
use crate::evaluation::{
    assess_survival_state, evaluate_hobbs_state, HobbsScoreParams, StateScore, TrapAssessment,
};
use crate::simulation::joint_action::JointAction;

use super::path::{FuturePath, FutureStep, MAX_SEARCH_DEPTH};
use super::budget::SearchBudget;
use super::forecast::{ForecastCertainty, ForecastDelta};
use super::graph::{FutureGraph, NodeId, SearchError};

#[derive(Debug, Clone)]
pub(crate) struct HobbsSearchResult {
    pub(crate) direction: Direction,
    pub(crate) score: StateScore,
    pub(crate) survival: TrapAssessment,
    pub(crate) path: FuturePath,
    pub(crate) completed_depth: u8,
    pub(crate) root_directions: usize,
    pub(crate) certainty: ForecastCertainty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RouteRank {
    score: StateScore,
    safety: TrapAssessment,
}

impl RouteRank {
    fn tier(self) -> u8 {
        match (self.score, self.safety) {
            (StateScore::Win, _) => 6,
            (StateScore::Loss, _) | (_, TrapAssessment::ProvenTrap) => 0,
            (StateScore::Tie, _) => 1,
            (_, TrapAssessment::Viable) => 5,
            (_, TrapAssessment::Unknown) => 4,
            (_, TrapAssessment::Constrained) => 3,
            (_, TrapAssessment::ForcedCorridor) => 2,
        }
    }

    fn safety_rank(self) -> u8 {
        match self.safety {
            TrapAssessment::Viable => 4,
            TrapAssessment::Unknown => 3,
            TrapAssessment::Constrained => 2,
            TrapAssessment::ForcedCorridor => 1,
            TrapAssessment::ProvenTrap => 0,
        }
    }
}

impl Ord for RouteRank {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tier()
            .cmp(&other.tier())
            .then_with(|| self.score.cmp(&other.score))
            .then_with(|| self.safety_rank().cmp(&other.safety_rank()))
    }
}

impl PartialOrd for RouteRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
struct Route {
    rank: RouteRank,
    path: FuturePath,
    certainty: ForecastCertainty,
}

type ScoredCache = HashMap<NodeId, RouteRank>;

pub(crate) fn search_hobbs(
    graph: &mut FutureGraph,
    budget: &SearchBudget,
) -> Result<Option<HobbsSearchResult>, SearchError> {
    // The soft deadline leaves headroom for serialization and runtime jitter.
    let search_budget = budget.limited_to_soft_deadline();
    let mut scored = ScoredCache::new();
    let root = graph.root();
    let mut last_complete = None;
    for depth in 1..=MAX_SEARCH_DEPTH {
        if search_budget.expired() {
            break;
        }
        let Some(route) = evaluate_minimax(
            graph,
            root,
            depth,
            ForecastCertainty::Deterministic,
            &search_budget,
            &mut scored,
        )?
        else {
            break;
        };

        let Some(our_actor) = graph
            .node(root)
            .state
            .actor_index(&graph.node(root).state.our_snake_id)
        else {
            return Ok(last_complete);
        };
        let Some(direction) = route
            .path
            .first()
            .and_then(|step| step.joint_action.direction_for(our_actor))
        else {
            return Ok(last_complete);
        };

        let directions = graph
            .node(root)
            .children
            .iter()
            .filter_map(|edge| edge.joint_action.direction_for(our_actor))
            .collect::<std::collections::HashSet<_>>();
        let survival = route.rank.safety;
        log::debug!(
            target: "search_diagnostics",
            "hobbs_depth turn={} depth={} move={:?} score={:?} survival={:?} root_directions={} nodes={}",
            graph.node(root).state.turn,
            depth,
            direction,
            route.rank.score,
            survival,
            directions.len(),
            graph.node_count(),
        );
        last_complete = Some(HobbsSearchResult {
            direction,
            score: route.rank.score,
            survival,
            path: route.path,
            completed_depth: depth,
            root_directions: directions.len(),
            certainty: route.certainty,
        });
        // If the winner/loser is known at an immediate horizon and verified
        // deterministic, continuing cannot change that branch's outcome.
        if depth > 1
            && matches!(route.rank.score, StateScore::Win)
            && !route.certainty.is_provisional()
        {
            break;
        }
    }
    Ok(last_complete)
}

fn evaluate_minimax(
    graph: &mut FutureGraph,
    node_id: NodeId,
    depth: u8,
    certainty: ForecastCertainty,
    budget: &SearchBudget,
    cache: &mut ScoredCache,
) -> Result<Option<Route>, SearchError> {
    if budget.expired() {
        return Ok(None);
    }
    if graph.node(node_id).is_terminal() || depth == 0 {
        let rank = *cache
            .entry(node_id)
            .or_insert_with(|| evaluate_leaf(graph, node_id));
        return Ok(Some(Route {
            rank,
            path: FuturePath::empty(),
            certainty,
        }));
    }

    if !graph.expand_one(node_id, budget)? {
        return Ok(None);
    }

    let node = graph.node(node_id);
    let Some(our_actor) = node.state.actor_index(&node.state.our_snake_id) else {
        return Ok(None);
    };

    // Copy only the lightweight descriptors; we must release the graph
    // borrow before exploring descendants mutably.
    let mut choices = Vec::<(Direction, Vec<(JointAction, NodeId, ForecastDelta)>)>::new();
    for direction in Direction::ALL {
        let alternatives = node
            .children
            .iter()
            .filter(|edge| edge.joint_action.direction_for(our_actor) == Some(direction))
            .map(|edge| (edge.joint_action.clone(), edge.child, edge.forecast_delta))
            .collect::<Vec<_>>();
        if !alternatives.is_empty() {
            choices.push((direction, alternatives));
        }
    }
    if choices.is_empty() {
        let rank = *cache
            .entry(node_id)
            .or_insert_with(|| evaluate_leaf(graph, node_id));
        return Ok(Some(Route {
            rank,
            path: FuturePath::empty(),
            certainty,
        }));
    }

    let mut best: Option<Route> = None;
    for (direction, responses) in choices {
        let mut worst: Option<Route> = None;
        for (joint_action, child, forecast_delta) in responses {
            if budget.expired() {
                return Ok(None);
            }
            let child_certainty = certainty.after(forecast_delta);
            let Some(mut candidate) =
                evaluate_minimax(graph, child, depth - 1, child_certainty, budget, cache)?
            else {
                return Ok(None);
            };
            // Preserve structural exposure encountered along the entire
            // forecast path; otherwise a deep leaf can hide an earlier pin.
            let immediate_safety = assess_survival_state(&graph.node(child).state);
            candidate.rank.safety = worst_safety(candidate.rank.safety, immediate_safety);
            candidate.path = candidate.path.prepend(FutureStep {
                node: node_id,
                joint_action,
                child,
            });
            if worst
                .as_ref()
                .is_none_or(|previous| candidate.rank < previous.rank)
            {
                worst = Some(candidate);
            }
        }
        if let Some(worst_response) = worst {
            if best.as_ref().is_none_or(|previous| {
                worst_response.rank > previous.rank
                    || (worst_response.rank == previous.rank
                        && direction.rank()
                            < direction_of_first(graph, node_id, &previous.path)
                                .map_or(u8::MAX, Direction::rank))
            }) {
                best = Some(worst_response);
            }
        }
    }
    Ok(best)
}

fn worst_safety(left: TrapAssessment, right: TrapAssessment) -> TrapAssessment {
    use TrapAssessment::{Constrained, ForcedCorridor, ProvenTrap, Unknown, Viable};
    match (left, right) {
        (ProvenTrap, _) | (_, ProvenTrap) => ProvenTrap,
        (ForcedCorridor, _) | (_, ForcedCorridor) => ForcedCorridor,
        (Constrained, _) | (_, Constrained) => Constrained,
        (Unknown, _) | (_, Unknown) => Unknown,
        (Viable, Viable) => Viable,
    }
}

fn direction_of_first(graph: &FutureGraph, node: NodeId, path: &FuturePath) -> Option<Direction> {
    let state = &graph.node(node).state;
    let our_actor = state.actor_index(&state.our_snake_id)?;
    path.first()?.joint_action.direction_for(our_actor)
}

fn evaluate_leaf(graph: &FutureGraph, node_id: NodeId) -> RouteRank {
    let state = &graph.node(node_id).state;
    let Some(actor) = state.actor_index(&state.our_snake_id) else {
        return RouteRank {
            score: StateScore::Loss,
            safety: TrapAssessment::Unknown,
        };
    };
    let params = HobbsScoreParams::STANDARD;
    let territory = TemporalTerritory::from_state(state, params.fill_cycles, params.cell_weights);
    let score = evaluate_hobbs_state(state, &territory, actor, params).score;
    let survival = assess_survival_state(state);
    RouteRank {
        score,
        safety: survival,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
    use crate::Coord;
    use std::time::Duration;

    fn snake(id: &str, x: i32, y: i32, length: usize) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: vec![Coord { x, y }; length],
            alive: true,
        }
    }

    fn state() -> SimulatedGameState {
        SimulatedGameState {
            turn: 0,
            width: 7,
            height: 7,
            food: vec![Coord { x: 3, y: 4 }],
            hazards: vec![],
            snakes: vec![snake("ours", 1, 2, 3), snake("enemy", 5, 5, 3)],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn loss_tie_prefers_less_restricted_escape() {
        let less_restricted = RouteRank {
            score: StateScore::Loss,
            safety: TrapAssessment::Constrained,
        };
        let forced = RouteRank {
            score: StateScore::Loss,
            safety: TrapAssessment::ForcedCorridor,
        };
        assert!(less_restricted > forced);
    }

    #[test]
    fn forecast_preserves_worst_intermediate_safety() {
        assert_eq!(
            worst_safety(TrapAssessment::Viable, TrapAssessment::Constrained),
            TrapAssessment::Constrained
        );
        assert_eq!(
            worst_safety(TrapAssessment::Constrained, TrapAssessment::ProvenTrap),
            TrapAssessment::ProvenTrap
        );
    }

    #[test]
    fn terminal_win_outweighs_nonterminal_viability() {
        let win = RouteRank {
            score: StateScore::Win,
            safety: TrapAssessment::Unknown,
        };
        let normal = RouteRank {
            score: StateScore::Normal {
                utility_milli: 2000,
            },
            safety: TrapAssessment::Viable,
        };
        assert!(win > normal);
    }

    #[test]
    fn constrained_exit_loses_to_viable_alternative_without_score_weight() {
        let constrained = RouteRank {
            score: StateScore::Normal {
                utility_milli: 1000,
            },
            safety: TrapAssessment::Constrained,
        };
        let viable = RouteRank {
            score: StateScore::Normal {
                utility_milli: -200,
            },
            safety: TrapAssessment::Viable,
        };
        assert!(viable > constrained);
        assert!(
            constrained
                > RouteRank {
                    score: StateScore::Tie,
                    safety: TrapAssessment::Unknown,
                }
        );
    }

    #[test]
    fn all_root_directions_receive_a_pessimistic_answer() {
        let mut graph = FutureGraph::new_beam(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(4));
        let result = search_hobbs(&mut graph, &budget).unwrap().unwrap();
        assert!(result.completed_depth >= 1);
        assert!(result.root_directions >= 2);
        assert_eq!(
            result
                .path
                .first()
                .unwrap()
                .joint_action
                .direction_for(graph.node(graph.root()).state.actor_index("ours").unwrap()),
            Some(result.direction)
        );
    }

    #[test]
    fn no_depth_is_committed_when_budget_is_empty() {
        let mut graph = FutureGraph::new_beam(state());
        let budget = SearchBudget::for_duration(Duration::ZERO);
        assert!(search_hobbs(&mut graph, &budget).unwrap().is_none());
    }
}
