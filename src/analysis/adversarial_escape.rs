//! Cached adversarial escape verification over already-generated joint moves.
//!
//! This is a read-only bounded AND/OR inspection of the FutureGraph. The
//! search policy, MIN ordering, route values and graph expansion stay unchanged.
//! A losing reply can refute an action without complete reply coverage, while
//! survival demands complete coverage of every required adversarial reply.
//! Food uncertainty propagates into a conditional, never deterministic proof.

use std::collections::HashMap;
use std::time::Instant;

use crate::direction::Direction;
use crate::search::forecast::ForecastCertainty;
use crate::search::graph::{FutureGraph, NodeId, ResponseCoverage};

const MAX_INSPECTED_STATES: usize = 192;
const MAX_INSPECTED_REPLIES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdversarialEscapeVerdict {
    /// All cached responses were exhausted across the requested horizon,
    /// and a MAX escape policy survives every MIN reply within that horizon.
    SurvivesHorizon,
    /// For this specific action, MIN has a reply that defeats every
    /// subsequent MAX choice in the fixed-food model within the horizon.
    ForcedLossWithinHorizon,
    /// Incomplete coverage, tied terminal outcome, or exhausted work budget.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdversarialEscapeAnalysis {
    pub(crate) verdict: AdversarialEscapeVerdict,
    pub(crate) certainty: Option<ForecastCertainty>,
    pub(crate) requested_horizon: u8,
    pub(crate) root_coverage: ResponseCoverage,
    pub(crate) known_root_replies: usize,
    pub(crate) observed_losing_root_replies: usize,
    pub(crate) checked_states: usize,
    pub(crate) checked_replies: usize,
    pub(crate) budget_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proof {
    Survives(ForecastCertainty),
    Loses(ForecastCertainty),
    Unknown,
}

impl Proof {
    fn uncertainty(self) -> Option<ForecastCertainty> {
        match self {
            Self::Survives(value) | Self::Loses(value) => Some(value),
            Self::Unknown => None,
        }
    }
}

/// Search-scoped verifier. Reuse its memo across root directions, but never
/// across a FutureGraph reroot or node-ID compaction.
pub(crate) struct AdversarialEscapeAnalyzer<'a> {
    graph: &'a FutureGraph,
    deadline: Instant,
    memo: HashMap<(NodeId, u8, ForecastCertainty), Proof>,
    checked_states: usize,
    checked_replies: usize,
    budget_truncated: bool,
}

impl<'a> AdversarialEscapeAnalyzer<'a> {
    pub(crate) fn new(graph: &'a FutureGraph, deadline: Instant) -> Self {
        Self {
            graph,
            deadline,
            memo: HashMap::new(),
            checked_states: 0,
            checked_replies: 0,
            budget_truncated: false,
        }
    }

    fn within_budget(&mut self) -> bool {
        let available = Instant::now() < self.deadline
            && self.checked_states < MAX_INSPECTED_STATES
            && self.checked_replies < MAX_INSPECTED_REPLIES;
        if !available {
            self.budget_truncated = true;
        }
        available
    }

    fn terminal(&self, node_id: NodeId, certainty: ForecastCertainty) -> Option<Proof> {
        let state = &self.graph.node(node_id).state;
        let ours = state.snake(&state.our_snake_id)?;
        let others_alive = state
            .snakes
            .iter()
            .any(|snake| snake.alive && snake.id != state.our_snake_id);
        if !ours.alive {
            // Simultaneous elimination is a tie, not a proven loss.
            return Some(if others_alive {
                Proof::Loses(certainty)
            } else {
                Proof::Unknown
            });
        }
        if !others_alive {
            return Some(Proof::Survives(certainty));
        }
        None
    }

    fn state_proof(
        &mut self,
        node_id: NodeId,
        remaining: u8,
        certainty: ForecastCertainty,
    ) -> Proof {
        if !self.within_budget() {
            return Proof::Unknown;
        }
        if let Some(terminal) = self.terminal(node_id, certainty) {
            return terminal;
        }
        if remaining == 0 {
            return Proof::Survives(certainty);
        }
        let key = (node_id, remaining, certainty);
        if let Some(&cached) = self.memo.get(&key) {
            return cached;
        }
        self.checked_states += 1;
        let Some(directions) = self.graph.cached_directions(node_id) else {
            return Proof::Unknown;
        };
        if directions.is_empty() {
            return Proof::Unknown;
        }
        let mut all_losing = true;
        let mut any_conditional_loss = false;
        let mut conditional_survival = None;
        for direction in directions {
            match self.action_proof(node_id, direction, remaining, certainty) {
                Proof::Survives(ForecastCertainty::Deterministic) => {
                    self.memo
                        .insert(key, Proof::Survives(ForecastCertainty::Deterministic));
                    return Proof::Survives(ForecastCertainty::Deterministic);
                }
                Proof::Survives(food) => {
                    conditional_survival = Some(food);
                    all_losing = false;
                }
                Proof::Loses(food) => {
                    any_conditional_loss |= food.is_provisional();
                }
                Proof::Unknown => {
                    all_losing = false;
                }
            }
        }
        let proof = if let Some(food) = conditional_survival {
            Proof::Survives(food)
        } else if all_losing {
            Proof::Loses(if any_conditional_loss {
                ForecastCertainty::FoodProvisional
            } else {
                ForecastCertainty::Deterministic
            })
        } else {
            Proof::Unknown
        };
        // Do not cache work-budget truncations as stable search evidence.
        if !self.budget_truncated {
            self.memo.insert(key, proof);
        }
        proof
    }

    fn action_proof(
        &mut self,
        node_id: NodeId,
        direction: Direction,
        remaining: u8,
        certainty: ForecastCertainty,
    ) -> Proof {
        if !self.within_budget() {
            return Proof::Unknown;
        }
        let replies = self.graph.known_responses(node_id, direction);
        if replies.is_empty() {
            return Proof::Unknown;
        }
        let complete =
            self.graph.response_coverage(node_id, direction) == ResponseCoverage::Complete;
        let mut all_survive = complete;
        let mut survival_food_uncertain = certainty.is_provisional();
        let mut conditional_loss = None;
        for reply in replies {
            if !self.within_budget() {
                return Proof::Unknown;
            }
            self.checked_replies += 1;
            let child_certainty = certainty.after(reply.forecast_delta);
            let proof = self.state_proof(reply.child, remaining.saturating_sub(1), child_certainty);
            match proof {
                // One actual legal MIN response is sufficient to defeat the
                // selected MAX action; other replies may be alpha-pruned.
                Proof::Loses(ForecastCertainty::Deterministic) => {
                    return Proof::Loses(ForecastCertainty::Deterministic);
                }
                Proof::Loses(food) => {
                    conditional_loss = Some(food);
                    all_survive = false;
                }
                Proof::Survives(food) => {
                    survival_food_uncertain |= food.is_provisional();
                }
                Proof::Unknown => {
                    all_survive = false;
                }
            }
        }
        if let Some(food) = conditional_loss {
            Proof::Loses(food)
        } else if all_survive {
            Proof::Survives(if survival_food_uncertain {
                ForecastCertainty::FoodProvisional
            } else {
                ForecastCertainty::Deterministic
            })
        } else {
            Proof::Unknown
        }
    }

    /// Evaluate ONE specified root action. An unsafe action is not a proof
    /// that the entire position is lost: another root action might escape.
    pub(crate) fn analyze_root_direction(
        &mut self,
        direction: Direction,
        requested_horizon: u8,
    ) -> AdversarialEscapeAnalysis {
        let root = self.graph.root();
        let root_coverage = self.graph.response_coverage(root, direction);
        let root_replies = self.graph.known_responses(root, direction);
        let known_root_replies = root_replies.len();
        let observed_losing_root_replies = root_replies
            .iter()
            .filter(|edge| {
                matches!(
                    self.terminal(edge.child, ForecastCertainty::Deterministic),
                    Some(Proof::Loses(_))
                )
            })
            .count();
        let proof = if requested_horizon == 0 {
            Proof::Unknown
        } else {
            self.action_proof(
                root,
                direction,
                requested_horizon,
                ForecastCertainty::Deterministic,
            )
        };
        let verdict = match proof {
            Proof::Survives(_) => AdversarialEscapeVerdict::SurvivesHorizon,
            Proof::Loses(_) => AdversarialEscapeVerdict::ForcedLossWithinHorizon,
            Proof::Unknown => AdversarialEscapeVerdict::Unknown,
        };
        AdversarialEscapeAnalysis {
            verdict,
            certainty: proof.uncertainty(),
            requested_horizon,
            root_coverage,
            known_root_replies,
            observed_losing_root_replies,
            checked_states: self.checked_states,
            checked_replies: self.checked_replies,
            budget_truncated: self.budget_truncated,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::analysis::replay_fixtures::late_seed_20261003;
    use crate::search::budget::SearchBudget;
    use crate::search::forecast::FoodForecastPolicy;
    use crate::search::graph::ResponseLookup;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
    use crate::Coord;

    fn simple_board() -> SimulatedGameState {
        SimulatedGameState {
            turn: 0,
            width: 7,
            height: 7,
            food: Vec::new(),
            hazards: Vec::new(),
            snakes: vec![
                SimulatedSnake {
                    id: "ours".into(),
                    body: vec![Coord { x: 1, y: 1 }],
                    health: 100,
                    alive: true,
                },
                SimulatedSnake {
                    id: "hobbs".into(),
                    body: vec![Coord { x: 5, y: 5 }],
                    health: 100,
                    alive: true,
                },
            ],
            our_snake_id: "ours".into(),
            rules: RulesContext {
                name: "standard".into(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    fn populate(graph: &mut FutureGraph, direction: Direction, number_of_replies: Option<usize>) {
        let root = graph.root();
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let mut index = 0;
        loop {
            if number_of_replies.is_some_and(|cap| index >= cap) {
                break;
            }
            match graph
                .next_response(root, direction, index, &budget)
                .unwrap()
            {
                ResponseLookup::Edge(_) => index += 1,
                ResponseLookup::Exhausted => break,
                ResponseLookup::Deadline | ResponseLookup::ResourceLimit => {
                    panic!("fixture response generation stopped before coverage was complete")
                }
            }
        }
    }

    #[test]
    fn unexpanded_replies_never_prove_safety() {
        let graph = FutureGraph::new_beam(simple_board());
        let mut analyzer =
            AdversarialEscapeAnalyzer::new(&graph, Instant::now() + Duration::from_secs(2));
        let result = analyzer.analyze_root_direction(Direction::Right, 1);
        assert_eq!(result.verdict, AdversarialEscapeVerdict::Unknown);
        assert_eq!(result.root_coverage, ResponseCoverage::Unexpanded);
    }

    #[test]
    fn partial_replies_cannot_verify_survival() {
        let mut graph = FutureGraph::new_beam(simple_board());
        populate(&mut graph, Direction::Right, Some(1));
        let before = (graph.node_count(), graph.edge_count());
        let mut analyzer =
            AdversarialEscapeAnalyzer::new(&graph, Instant::now() + Duration::from_secs(2));
        let result = analyzer.analyze_root_direction(Direction::Right, 1);
        assert_eq!(result.verdict, AdversarialEscapeVerdict::Unknown);
        assert_eq!(result.root_coverage, ResponseCoverage::Partial);
        assert_eq!(before, (graph.node_count(), graph.edge_count()));
    }

    #[test]
    fn complete_replies_verify_one_ply_survival_without_graph_expansion() {
        let mut graph = FutureGraph::new_beam(simple_board());
        populate(&mut graph, Direction::Right, None);
        let before = (graph.node_count(), graph.edge_count());
        let mut analyzer =
            AdversarialEscapeAnalyzer::new(&graph, Instant::now() + Duration::from_secs(2));
        let result = analyzer.analyze_root_direction(Direction::Right, 1);
        assert_eq!(result.verdict, AdversarialEscapeVerdict::SurvivesHorizon);
        assert_eq!(result.certainty, Some(ForecastCertainty::Deterministic));
        assert_eq!(result.root_coverage, ResponseCoverage::Complete);
        assert_eq!(before, (graph.node_count(), graph.edge_count()));
    }

    #[test]
    fn captured_final_exit_allows_hobbs_to_force_immediate_loss() {
        let mut graph = FutureGraph::new_beam(late_seed_20261003(333));
        populate(&mut graph, Direction::Left, None);
        let mut analyzer =
            AdversarialEscapeAnalyzer::new(&graph, Instant::now() + Duration::from_secs(2));
        let result = analyzer.analyze_root_direction(Direction::Left, 1);
        assert_eq!(
            result.verdict,
            AdversarialEscapeVerdict::ForcedLossWithinHorizon
        );
        assert!(result.observed_losing_root_replies >= 1);
        assert_eq!(result.certainty, Some(ForecastCertainty::Deterministic));
    }

    #[test]
    fn random_future_food_downgrades_complete_survival_to_conditional() {
        let policy = FoodForecastPolicy {
            spawn_chance_percent: 15,
            minimum_food: 1,
        };
        let mut graph = FutureGraph::new_beam_with_forecast(simple_board(), policy);
        populate(&mut graph, Direction::Right, None);
        let mut analyzer =
            AdversarialEscapeAnalyzer::new(&graph, Instant::now() + Duration::from_secs(2));
        let result = analyzer.analyze_root_direction(Direction::Right, 1);
        assert_eq!(result.verdict, AdversarialEscapeVerdict::SurvivesHorizon);
        assert_eq!(result.certainty, Some(ForecastCertainty::FoodProvisional));
    }

    #[test]
    fn expired_deadline_never_produces_a_proof() {
        let mut graph = FutureGraph::new_beam(simple_board());
        populate(&mut graph, Direction::Right, None);
        let mut analyzer = AdversarialEscapeAnalyzer::new(&graph, Instant::now());
        let result = analyzer.analyze_root_direction(Direction::Right, 4);
        assert_eq!(result.verdict, AdversarialEscapeVerdict::Unknown);
        assert!(result.budget_truncated);
    }

    #[test]
    fn deep_cached_horizon_without_expanded_children_remains_unknown() {
        let mut graph = FutureGraph::new_beam(simple_board());
        populate(&mut graph, Direction::Right, None);
        let mut analyzer =
            AdversarialEscapeAnalyzer::new(&graph, Instant::now() + Duration::from_secs(2));
        let result = analyzer.analyze_root_direction(Direction::Right, 4);
        assert_eq!(result.verdict, AdversarialEscapeVerdict::Unknown);
        assert!(!result.budget_truncated);
    }
}
