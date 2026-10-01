#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use rayon::prelude::*;

use crate::analysis::transition::analyze_transition_parts;
use crate::analysis::{
    BorderFobicAnalysis, EnclosureAnalysis, StateAnalysis, StrategicPosture, TacticalStateAnalysis,
    TerritoryAnalysis,
};
use crate::decision::joint_actions::JointActionGenerator;
use crate::decision::state_key::StateKey;
use crate::direction::MoveMask;
use crate::enemy::profile::OpponentProfiles;
use crate::enemy::tracing::{trace_with_mobility, EnemyTracingOutput};
use crate::evaluation::{ActorContext, ActorSnapshot, ActorTable, ActorUtilityMetrics, TransitionScore};
use crate::modes::hunting::{self, HuntingModeOutput};
use crate::modes::survival::{self, SurvivalModeOutput};
use crate::simulation::joint_action::JointAction;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::resolver::{
    resolve_turn, ForecastDelta, InstantEvent, ResolveError, TurnResolution,
};
use crate::simulation::state::SimulatedGameState;
use crate::spatial::SpatialOccupancy;

use super::actor_priority::ordered_child_ids_for_search;
use super::budget::SearchBudget;

pub(crate) type NodeId = usize;

const MAX_PARALLEL_ACTION_BATCH: usize = 8;
const INITIAL_BATCH_ESTIMATE: Duration = Duration::from_millis(2);
const BATCH_DEADLINE_RESERVE: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnalysisProfile {
    Full,
    BeamLean,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct GraphPerfStats {
    pub(crate) action_batches: u32,
    pub(crate) parallel_action_batches: u32,
    pub(crate) resolved_actions: u32,
    pub(crate) new_nodes_built: u32,
    pub(crate) resolve_us: u64,
    pub(crate) node_build_us: u64,
    pub(crate) merge_us: u64,
    pub(crate) edge_score_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExpansionReport {
    pub(crate) completed_depth: u8,
    pub(crate) nodes: u32,
    pub(crate) edges: u32,
    pub(crate) transposition_hits: u32,
    pub(crate) elapsed_us: u64,
    pub(crate) safety_reserve_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NodeExpansion {
    pub(crate) node_id: NodeId,
    pub(crate) completed: bool,
    pub(crate) expanded: bool,
    pub(crate) new_nodes: u32,
    pub(crate) new_edges: u32,
    pub(crate) transposition_hits: u32,
    pub(crate) elapsed_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DepthExpansion {
    pub(crate) completed: bool,
    pub(crate) frontier_nodes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubtreeExpansion {
    pub(crate) start_node: NodeId,
    pub(crate) requested_depth: u8,
    pub(crate) completed: bool,
    pub(crate) expanded_nodes: u32,
    pub(crate) new_nodes: u32,
    pub(crate) new_edges: u32,
    pub(crate) transposition_hits: u32,
    pub(crate) elapsed_us: u64,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchEdge {
    pub(crate) joint_action: JointAction,
    pub(crate) events: Vec<InstantEvent>,
    pub(crate) forecast_delta: ForecastDelta,
    pub(crate) transition: TransitionScore,
    pub(crate) child: NodeId,
}

struct ResolvedCandidate {
    joint_action: JointAction,
    state: SimulatedGameState,
    key: StateKey,
    resolution_events: Vec<InstantEvent>,
    forecast_delta: ForecastDelta,
}

#[derive(Debug, Clone)]
pub(crate) struct NodeAnalysis {
    pub(crate) state: Arc<StateAnalysis>,
    pub(crate) mobility: Arc<MobilityAnalysis>,
    pub(crate) tracing: Arc<EnemyTracingOutput>,
    pub(crate) tactical: Arc<TacticalStateAnalysis>,
    pub(crate) territory: Arc<TerritoryAnalysis>,
    pub(crate) border: Arc<BorderFobicAnalysis>,
    pub(crate) posture: Arc<StrategicPosture>,
    pub(crate) enclosure: Arc<EnclosureAnalysis>,
    pub(crate) survival: Arc<SurvivalModeOutput>,
    pub(crate) hunting: Arc<HuntingModeOutput>,
    pub(crate) actor_snapshots: ActorTable<ActorSnapshot>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchNode {
    pub(crate) state: SimulatedGameState,
    pub(crate) key: StateKey,
    pub(crate) analysis: Option<Arc<NodeAnalysis>>,
    pub(crate) children: Vec<SearchEdge>,
    expansion_complete: bool,
    pending_actions: Option<JointActionGenerator>,
}

impl SearchNode {
    pub(crate) fn active_analysis(&self) -> Option<&NodeAnalysis> {
        self.analysis.as_deref()
    }

    pub(crate) fn is_terminal(&self) -> bool {
        is_terminal_state(&self.state)
    }

    pub(crate) fn expansion_complete(&self) -> bool {
        self.expansion_complete
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SearchError {
    Resolve(ResolveError),
}

impl From<ResolveError> for SearchError {
    fn from(value: ResolveError) -> Self {
        Self::Resolve(value)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FutureGraph {
    root: NodeId,
    nodes: Vec<SearchNode>,
    transpositions: HashMap<StateKey, NodeId>,
    transposition_hits: u32,
    edge_count: u32,
    opponent_profiles: OpponentProfiles,
    analysis_profile: AnalysisProfile,
    action_batch_estimate: Duration,
    perf_stats: GraphPerfStats,
}

impl FutureGraph {
    pub(crate) fn new(root_state: SimulatedGameState) -> Self {
        Self::new_with_opponent_profiles(root_state, OpponentProfiles::default())
    }

    pub(crate) fn new_with_opponent_profiles(
        root_state: SimulatedGameState,
        opponent_profiles: OpponentProfiles,
    ) -> Self {
        Self::new_with_profile(root_state, opponent_profiles, AnalysisProfile::Full)
    }

    fn new_with_profile(
        root_state: SimulatedGameState,
        opponent_profiles: OpponentProfiles,
        analysis_profile: AnalysisProfile,
    ) -> Self {
        let root_node = build_node(root_state, analysis_profile);
        let root_key = root_node.key.clone();

        Self {
            root: 0,
            nodes: vec![root_node],
            transpositions: HashMap::from([(root_key, 0)]),
            transposition_hits: 0,
            edge_count: 0,
            opponent_profiles,
            analysis_profile,
            action_batch_estimate: INITIAL_BATCH_ESTIMATE,
            perf_stats: GraphPerfStats::default(),
        }
    }

    pub(crate) fn independent_beam_graph(&self) -> Self {
        Self::new_with_profile(
            self.nodes[self.root].state.clone(),
            self.opponent_profiles.clone(),
            AnalysisProfile::BeamLean,
        )
    }

    pub(crate) fn set_opponent_profiles(&mut self, opponent_profiles: OpponentProfiles) {
        self.opponent_profiles = opponent_profiles;
    }

    pub(crate) fn use_beam_lean_analysis(&mut self) {
        self.analysis_profile = AnalysisProfile::BeamLean;
    }

    pub(crate) fn reset_performance(&mut self) {
        self.perf_stats = GraphPerfStats::default();
    }

    pub(crate) fn performance(&self) -> GraphPerfStats {
        self.perf_stats
    }

    pub(crate) fn root(&self) -> NodeId {
        self.root
    }

    pub(crate) fn node(&self, node_id: NodeId) -> &SearchNode {
        &self.nodes[node_id]
    }

    pub(crate) fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub(crate) fn edge_count(&self) -> u32 {
        self.edge_count
    }

    pub(crate) fn transposition_hits(&self) -> u32 {
        self.transposition_hits
    }

    pub(crate) fn find_node_by_key(&self, key: &StateKey) -> Option<NodeId> {
        self.transpositions.get(key).copied()
    }

    pub(crate) fn root_children_match_food(&self, actual_key: &StateKey) -> bool {
        self.nodes[self.root]
            .children
            .iter()
            .any(|edge| self.nodes[edge.child].key.food() == actual_key.food())
    }

    pub(crate) fn retain_chosen_direction(&mut self, direction: crate::direction::Direction) {
        if !self.nodes[self.root].expansion_complete {
            self.nodes[self.root].children.clear();
            self.nodes[self.root].pending_actions = None;
            self.garbage_collect();
            return;
        }

        let our_id = self.nodes[self.root].state.our_snake_id.clone();
        self.nodes[self.root]
            .children
            .retain(|edge| edge.joint_action.direction_for(&our_id) == Some(direction));
        self.garbage_collect();
    }

    pub(crate) fn reroot(&mut self, node_id: NodeId) {
        self.root = node_id;
    }

    fn garbage_collect(&mut self) {
        let mut order = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([self.root]);

        while let Some(node_id) = queue.pop_front() {
            if !seen.insert(node_id) {
                continue;
            }

            order.push(node_id);
            for edge in &self.nodes[node_id].children {
                queue.push_back(edge.child);
            }
        }

        let remap = order
            .iter()
            .enumerate()
            .map(|(new_id, old_id)| (*old_id, new_id))
            .collect::<HashMap<_, _>>();

        let old_nodes = std::mem::take(&mut self.nodes);
        let mut old_nodes = old_nodes.into_iter().map(Some).collect::<Vec<_>>();
        let mut nodes = Vec::with_capacity(order.len());
        let mut edge_count = 0_u32;

        for old_id in order {
            let mut node = old_nodes[old_id]
                .take()
                .expect("reachable node must exist during compaction");
            node.children = node
                .children
                .into_iter()
                .filter_map(|mut edge| {
                    let child = remap.get(&edge.child).copied()?;
                    edge.child = child;
                    Some(edge)
                })
                .collect();
            edge_count =
                edge_count.saturating_add(node.children.len().try_into().unwrap_or(u32::MAX));
            nodes.push(node);
        }

        self.nodes = nodes;
        self.root = 0;
        self.transpositions = self
            .nodes
            .iter()
            .enumerate()
            .map(|(node_id, node)| (node.key.clone(), node_id))
            .collect();
        self.edge_count = edge_count;
    }

    pub(crate) fn expand_iteratively(
        &mut self,
        minimum_target_depth: u8,
        maximum_depth: u8,
        budget: &SearchBudget,
    ) -> Result<ExpansionReport, SearchError> {
        let mut completed_depth = 0_u8;
        let mut previous_layer_elapsed = Duration::ZERO;

        for depth in 1..=maximum_depth {
            if budget.expired() {
                break;
            }

            if depth > minimum_target_depth {
                let estimate = previous_layer_elapsed
                    .checked_mul(2)
                    .unwrap_or(Duration::MAX)
                    .max(Duration::from_millis(1));
                if !budget.can_afford(estimate) {
                    break;
                }
            }

            let layer_started = std::time::Instant::now();
            let expansion = self.expand_depth(depth, budget)?;
            if !expansion.completed {
                self.garbage_collect();
                break;
            }

            previous_layer_elapsed = layer_started.elapsed();
            completed_depth = depth;
        }

        Ok(ExpansionReport {
            completed_depth,
            nodes: self.nodes.len().try_into().unwrap_or(u32::MAX),
            edges: self.edge_count,
            transposition_hits: self.transposition_hits,
            elapsed_us: budget.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
            safety_reserve_us: budget
                .safety_reserve()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX),
        })
    }

    pub(crate) fn expand_depth(
        &mut self,
        depth: u8,
        budget: &SearchBudget,
    ) -> Result<DepthExpansion, SearchError> {
        let parents = self.nodes_at_depth(depth.saturating_sub(1));
        let frontier_nodes = parents.len().try_into().unwrap_or(u32::MAX);

        for node_id in parents {
            if budget.expired() {
                self.garbage_collect();
                return Ok(DepthExpansion {
                    completed: false,
                    frontier_nodes,
                });
            }

            if !self.expand_node_budgeted(node_id, Some(budget))? {
                self.garbage_collect();
                return Ok(DepthExpansion {
                    completed: false,
                    frontier_nodes,
                });
            }
        }

        Ok(DepthExpansion {
            completed: true,
            frontier_nodes,
        })
    }

    pub(crate) fn expand_subtree(
        &mut self,
        start_node: NodeId,
        additional_depth: u8,
        budget: &SearchBudget,
    ) -> Result<SubtreeExpansion, SearchError> {
        let nodes_before = self.nodes.len();
        let edges_before = self.edge_count;
        let transpositions_before = self.transposition_hits;
        let started = std::time::Instant::now();
        let mut queue = VecDeque::from([(start_node, 0_u8)]);
        let mut visited = HashSet::new();
        let mut expanded_nodes = 0_u32;

        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= additional_depth || !visited.insert(node_id) {
                continue;
            }

            if budget.expired() {
                return Ok(SubtreeExpansion {
                    start_node,
                    requested_depth: additional_depth,
                    completed: false,
                    expanded_nodes,
                    new_nodes: self
                        .nodes
                        .len()
                        .saturating_sub(nodes_before)
                        .try_into()
                        .unwrap_or(u32::MAX),
                    new_edges: self.edge_count.saturating_sub(edges_before),
                    transposition_hits: self
                        .transposition_hits
                        .saturating_sub(transpositions_before),
                    elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                });
            }

            if !self.expand_node_budgeted(node_id, Some(budget))? {
                return Ok(SubtreeExpansion {
                    start_node,
                    requested_depth: additional_depth,
                    completed: false,
                    expanded_nodes,
                    new_nodes: self
                        .nodes
                        .len()
                        .saturating_sub(nodes_before)
                        .try_into()
                        .unwrap_or(u32::MAX),
                    new_edges: self.edge_count.saturating_sub(edges_before),
                    transposition_hits: self
                        .transposition_hits
                        .saturating_sub(transpositions_before),
                    elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                });
            }

            expanded_nodes = expanded_nodes.saturating_add(1);
            let children = self.nodes[node_id]
                .children
                .iter()
                .map(|edge| edge.child)
                .collect::<Vec<_>>();
            for child in children {
                queue.push_back((child, depth.saturating_add(1)));
            }
        }

        Ok(SubtreeExpansion {
            start_node,
            requested_depth: additional_depth,
            completed: true,
            expanded_nodes,
            new_nodes: self
                .nodes
                .len()
                .saturating_sub(nodes_before)
                .try_into()
                .unwrap_or(u32::MAX),
            new_edges: self.edge_count.saturating_sub(edges_before),
            transposition_hits: self
                .transposition_hits
                .saturating_sub(transpositions_before),
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        })
    }

    pub(crate) fn expand_prioritized_subtree(
        &mut self,
        start_node: NodeId,
        additional_depth: u8,
        budget: &SearchBudget,
    ) -> Result<SubtreeExpansion, SearchError> {
        let nodes_before = self.nodes.len();
        let edges_before = self.edge_count;
        let transpositions_before = self.transposition_hits;
        let started = std::time::Instant::now();
        let mut queue = VecDeque::from([(start_node, 0_u8)]);
        let mut visited = HashSet::new();
        let mut expanded_nodes = 0_u32;

        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= additional_depth || !visited.insert(node_id) {
                continue;
            }

            if budget.expired() {
                return Ok(SubtreeExpansion {
                    start_node,
                    requested_depth: additional_depth,
                    completed: false,
                    expanded_nodes,
                    new_nodes: self
                        .nodes
                        .len()
                        .saturating_sub(nodes_before)
                        .try_into()
                        .unwrap_or(u32::MAX),
                    new_edges: self.edge_count.saturating_sub(edges_before),
                    transposition_hits: self
                        .transposition_hits
                        .saturating_sub(transpositions_before),
                    elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                });
            }

            if !self.expand_node_budgeted(node_id, Some(budget))? {
                return Ok(SubtreeExpansion {
                    start_node,
                    requested_depth: additional_depth,
                    completed: false,
                    expanded_nodes,
                    new_nodes: self
                        .nodes
                        .len()
                        .saturating_sub(nodes_before)
                        .try_into()
                        .unwrap_or(u32::MAX),
                    new_edges: self.edge_count.saturating_sub(edges_before),
                    transposition_hits: self
                        .transposition_hits
                        .saturating_sub(transpositions_before),
                    elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                });
            }

            expanded_nodes = expanded_nodes.saturating_add(1);
            let children = ordered_child_ids_for_search(self, node_id);
            for child in children {
                queue.push_back((child, depth.saturating_add(1)));
            }
        }

        Ok(SubtreeExpansion {
            start_node,
            requested_depth: additional_depth,
            completed: true,
            expanded_nodes,
            new_nodes: self
                .nodes
                .len()
                .saturating_sub(nodes_before)
                .try_into()
                .unwrap_or(u32::MAX),
            new_edges: self.edge_count.saturating_sub(edges_before),
            transposition_hits: self
                .transposition_hits
                .saturating_sub(transpositions_before),
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        })
    }

    pub(crate) fn expand_frontier(
        &mut self,
        node_id: NodeId,
        budget: &SearchBudget,
    ) -> Result<NodeExpansion, SearchError> {
        let nodes_before = self.nodes.len();
        let edges_before = self.edge_count;
        let transpositions_before = self.transposition_hits;
        let already_expanded =
            self.nodes[node_id].expansion_complete || self.nodes[node_id].is_terminal();
        let started = std::time::Instant::now();

        let completed = self.expand_node_budgeted(node_id, Some(budget))?;
        if !completed {
            self.garbage_collect();
        }

        Ok(NodeExpansion {
            node_id,
            completed,
            expanded: completed && !already_expanded,
            new_nodes: self
                .nodes
                .len()
                .saturating_sub(nodes_before)
                .try_into()
                .unwrap_or(u32::MAX),
            new_edges: self.edge_count.saturating_sub(edges_before),
            transposition_hits: self
                .transposition_hits
                .saturating_sub(transpositions_before),
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        })
    }

    pub(crate) fn node_count_at_depth(&self, depth: u8) -> usize {
        self.nodes_at_depth(depth).len()
    }

    fn nodes_at_depth(&self, depth: u8) -> Vec<NodeId> {
        let mut current = vec![self.root];

        for _ in 0..depth {
            let mut next = Vec::new();
            let mut seen = HashSet::new();

            for node_id in current {
                for edge in &self.nodes[node_id].children {
                    if seen.insert(edge.child) {
                        next.push(edge.child);
                    }
                }
            }

            current = next;
            if current.is_empty() {
                break;
            }
        }

        current
    }

    pub(crate) fn expand_to_depth(&mut self, target_depth: u8) -> Result<(), SearchError> {
        let mut queue = VecDeque::from([(self.root, 0_u8)]);
        let mut expanded = HashSet::new();

        while let Some((node_id, depth)) = queue.pop_front() {
            if depth >= target_depth || !expanded.insert(node_id) {
                continue;
            }

            self.expand_node(node_id)?;

            for edge in self.nodes[node_id].children.clone() {
                queue.push_back((edge.child, depth.saturating_add(1)));
            }
        }

        Ok(())
    }

    fn expand_node(&mut self, node_id: NodeId) -> Result<(), SearchError> {
        self.expand_node_budgeted(node_id, None).map(|_| ())
    }

    fn expand_node_budgeted(
        &mut self,
        node_id: NodeId,
        budget: Option<&SearchBudget>,
    ) -> Result<bool, SearchError> {
        if self.nodes[node_id].expansion_complete || self.is_terminal(node_id) {
            return Ok(true);
        }

        let state = self.nodes[node_id].state.clone();
        let Some(parent_analysis) = self.nodes[node_id].analysis.as_ref() else {
            self.nodes[node_id].expansion_complete = true;
            return Ok(true);
        };
        let tracing = Arc::clone(&parent_analysis.tracing);
        let before_tactical = Arc::clone(&parent_analysis.tactical);

        let our_moves = if before_tactical.ours.deterministic_moves.is_empty() {
            MoveMask::all()
        } else {
            before_tactical.ours.deterministic_moves
        };

        let analysis_profile = self.analysis_profile;
        let mut actions = self.nodes[node_id]
            .pending_actions
            .take()
            .unwrap_or_else(|| match analysis_profile {
                AnalysisProfile::Full => JointActionGenerator::new_with_profiles(
                    &state,
                    our_moves,
                    &tracing,
                    &self.opponent_profiles,
                ),
                AnalysisProfile::BeamLean => {
                    JointActionGenerator::new_actor_relative_with_profiles(
                        &state,
                        our_moves,
                        &tracing,
                        &self.opponent_profiles,
                    )
                }
            });

        loop {
            if budget.is_some_and(SearchBudget::expired) {
                self.nodes[node_id].pending_actions = Some(actions);
                return Ok(false);
            }

            let batch_size = self.action_batch_size(budget);
            let mut batch_actions = Vec::with_capacity(batch_size);
            for _ in 0..batch_size {
                let Some(joint_action) = actions.next() else {
                    break;
                };
                batch_actions.push(joint_action);
            }

            if batch_actions.is_empty() {
                self.nodes[node_id].pending_actions = None;
                self.nodes[node_id].expansion_complete = true;
                return Ok(true);
            }

            let batch_started = std::time::Instant::now();
            let resolve_started = std::time::Instant::now();
            let resolved = batch_actions
                .into_par_iter()
                .map(|joint_action| {
                    resolve_turn(&state, &joint_action).map(|resolution| {
                        let key = StateKey::from_state(&resolution.state);
                        let TurnResolution {
                            state,
                            events,
                            forecast_delta,
                        } = resolution;
                        ResolvedCandidate {
                            joint_action,
                            state,
                            key,
                            resolution_events: events,
                            forecast_delta,
                        }
                    })
                })
                .collect::<Result<Vec<_>, ResolveError>>()?;
            let resolve_elapsed = resolve_started.elapsed();

            let mut unique_new = Vec::new();
            let mut seen_new = HashSet::new();
            let mut batch_transposition_hits = 0_u32;

            for candidate in &resolved {
                if self.transpositions.contains_key(&candidate.key) {
                    batch_transposition_hits = batch_transposition_hits.saturating_add(1);
                } else if seen_new.insert(candidate.key.clone()) {
                    unique_new.push((candidate.key.clone(), candidate.state.clone()));
                } else {
                    batch_transposition_hits = batch_transposition_hits.saturating_add(1);
                }
            }

            let profile = self.analysis_profile;
            let node_build_started = std::time::Instant::now();
            let built_nodes = unique_new
                .into_par_iter()
                .map(|(key, state)| {
                    let node = build_node_with_key(state, key.clone(), profile);
                    (key, node)
                })
                .collect::<Vec<_>>();
            let node_build_elapsed = node_build_started.elapsed();
            let built_node_count = built_nodes.len().try_into().unwrap_or(u32::MAX);

            let merge_started = std::time::Instant::now();
            for (key, node) in built_nodes {
                if self.transpositions.contains_key(&key) {
                    batch_transposition_hits = batch_transposition_hits.saturating_add(1);
                    continue;
                }
                let child = self.nodes.len();
                self.transpositions.insert(key, child);
                self.nodes.push(node);
            }
            self.transposition_hits = self
                .transposition_hits
                .saturating_add(batch_transposition_hits);

            let child_ids = resolved
                .iter()
                .map(|candidate| {
                    self.transpositions
                        .get(&candidate.key)
                        .copied()
                        .expect("resolved child must exist after deterministic merge")
                })
                .collect::<Vec<_>>();
            let merge_elapsed = merge_started.elapsed();

            let edge_score_started = std::time::Instant::now();
            let parent = &self.nodes[node_id];
            let nodes = &self.nodes;
            let prepared_edges = resolved
                .into_par_iter()
                .zip(child_ids.into_par_iter())
                .map(|(candidate, child)| {
                    let mut events = if let Some(child_analysis) = nodes[child].analysis.as_ref() {
                        let after_tactical = Arc::clone(&child_analysis.tactical);
                        analyze_transition_parts(
                            &before_tactical,
                            &nodes[child].state,
                            &candidate.resolution_events,
                            &after_tactical,
                        )
                        .events
                    } else {
                        candidate.resolution_events.clone()
                    };
                    append_border_exposure_event(&nodes[child], &mut events);

                    let transition = TransitionScore::from_parts(parent, &events, &nodes[child]);

                    SearchEdge {
                        joint_action: candidate.joint_action,
                        events,
                        forecast_delta: candidate.forecast_delta,
                        transition,
                        child,
                    }
                })
                .collect::<Vec<_>>();
            let edge_score_elapsed = edge_score_started.elapsed();

            let edge_count = prepared_edges.len().try_into().unwrap_or(u32::MAX);
            self.nodes[node_id].children.extend(prepared_edges);
            self.edge_count = self.edge_count.saturating_add(edge_count);

            self.perf_stats.action_batches = self.perf_stats.action_batches.saturating_add(1);
            if edge_count > 1 {
                self.perf_stats.parallel_action_batches =
                    self.perf_stats.parallel_action_batches.saturating_add(1);
            }
            self.perf_stats.resolved_actions =
                self.perf_stats.resolved_actions.saturating_add(edge_count);
            self.perf_stats.new_nodes_built = self
                .perf_stats
                .new_nodes_built
                .saturating_add(built_node_count);
            self.perf_stats.resolve_us = self
                .perf_stats
                .resolve_us
                .saturating_add(duration_us(resolve_elapsed));
            self.perf_stats.node_build_us = self
                .perf_stats
                .node_build_us
                .saturating_add(duration_us(node_build_elapsed));
            self.perf_stats.merge_us = self
                .perf_stats
                .merge_us
                .saturating_add(duration_us(merge_elapsed));
            self.perf_stats.edge_score_us = self
                .perf_stats
                .edge_score_us
                .saturating_add(duration_us(edge_score_elapsed));

            self.observe_action_batch(batch_started.elapsed());
        }
    }

    fn action_batch_size(&self, budget: Option<&SearchBudget>) -> usize {
        let parallelism = rayon::current_num_threads().clamp(1, MAX_PARALLEL_ACTION_BATCH);
        if parallelism == 1 {
            return 1;
        }

        let estimated = self
            .action_batch_estimate
            .saturating_add(BATCH_DEADLINE_RESERVE);
        if budget.is_some_and(|budget| !budget.can_afford_hard(estimated)) {
            1
        } else {
            parallelism
        }
    }

    fn observe_action_batch(&mut self, observed: Duration) {
        let previous = self.action_batch_estimate.as_micros();
        let observed = observed.as_micros();
        let smoothed = previous
            .saturating_mul(3)
            .saturating_add(observed)
            .saturating_div(4)
            .min(u128::from(u64::MAX));
        self.action_batch_estimate = Duration::from_micros(smoothed as u64);
    }

    fn is_terminal(&self, node_id: NodeId) -> bool {
        self.nodes[node_id].is_terminal()
    }
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}

fn append_border_exposure_event(child: &SearchNode, events: &mut Vec<InstantEvent>) {
    let Some(ours) = child
        .state
        .snake(&child.state.our_snake_id)
        .filter(|snake| snake.alive)
    else {
        return;
    };
    let Some(head) = ours.head() else {
        return;
    };
    let right = child.state.width as i32 - 1;
    let top = child.state.height as i32 - 1;
    let on_edge = head.x == 0 || head.y == 0 || head.x == right || head.y == top;
    if !on_edge {
        return;
    }

    let fear_milli = child
        .active_analysis()
        .and_then(|analysis| analysis.border.ours())
        .map_or(0, |snapshot| snapshot.fear_milli);
    let corner = (head.x == 0 || head.x == right) && (head.y == 0 || head.y == top);

    events.push(InstantEvent::SelfBorderExposure { fear_milli, corner });
}

fn is_terminal_state(state: &SimulatedGameState) -> bool {
    let ours_alive = state
        .snake(&state.our_snake_id)
        .is_some_and(|snake| snake.alive);
    let living_enemies = state
        .snakes
        .iter()
        .any(|snake| snake.alive && snake.id != state.our_snake_id);

    !ours_alive || !living_enemies
}

fn build_node(state: SimulatedGameState, profile: AnalysisProfile) -> SearchNode {
    let key = StateKey::from_state(&state);
    build_node_with_key(state, key, profile)
}

fn build_node_with_key(
    state: SimulatedGameState,
    key: StateKey,
    profile: AnalysisProfile,
) -> SearchNode {
    let analysis = if is_terminal_state(&state) {
        None
    } else {
        let spatial = Arc::new(SpatialOccupancy::from_state(&state));
        let mobility = Arc::new(MobilityAnalysis::from_spatial(Arc::clone(&spatial)));

        let (state_analysis, territory) = rayon::join(
            || match profile {
                AnalysisProfile::Full => StateAnalysis::from_simulated(&state),
                AnalysisProfile::BeamLean => StateAnalysis::from_simulated_routes_only(&state),
            },
            || TerritoryAnalysis::from_spatial(&state, &spatial),
        );
        let state_analysis = Arc::new(state_analysis);
        let territory = Arc::new(territory);

        let tracing = Arc::new(trace_with_mobility(&state, &state_analysis, &mobility));
        let tactical = Arc::new(TacticalStateAnalysis::from_parts(
            &state, &tracing, &mobility,
        ));
        let border = Arc::new(match profile {
            AnalysisProfile::Full => {
                BorderFobicAnalysis::from_parts_with_territory(&state, &tactical, &territory)
            }
            AnalysisProfile::BeamLean => {
                BorderFobicAnalysis::from_parts_with_territory_actor_relative(
                    &state, &tactical, &territory,
                )
            }
        });
        let posture = Arc::new(match profile {
            AnalysisProfile::Full => StrategicPosture::from_state(&state),
            AnalysisProfile::BeamLean => StrategicPosture::default(),
        });
        let enclosure = Arc::new(match profile {
            AnalysisProfile::Full => EnclosureAnalysis::from_parts(&state, &territory, &tactical),
            AnalysisProfile::BeamLean => {
                EnclosureAnalysis::from_parts_actor_relative(&state, &territory, &tactical)
            }
        });

        let (survival, hunting) = match profile {
            AnalysisProfile::Full => rayon::join(
                || survival::analyze_with_border(&state, &tactical, &border),
                || {
                    hunting::analyze(
                        &state, &tactical, &tracing, &territory, &enclosure, &posture,
                    )
                },
            ),
            AnalysisProfile::BeamLean => {
                (SurvivalModeOutput::default(), HuntingModeOutput::default())
            }
        };
        let survival = Arc::new(survival);
        let hunting = Arc::new(hunting);

        let actor_snapshots = state
            .snakes
            .par_iter()
            .map(|snake| {
                if !snake.alive {
                    return None;
                }

                let context = ActorContext::from_state(&state, &snake.id)?;
                let metrics = ActorUtilityMetrics::from_parts(
                    &state,
                    &snake.id,
                    &state_analysis,
                    &mobility,
                    &territory,
                    &enclosure,
                    &border,
                )?;
                Some(ActorSnapshot::new(context, metrics))
            })
            .collect::<Vec<_>>();
        Some(Arc::new(NodeAnalysis {
            state: state_analysis,
            mobility,
            tracing,
            tactical,
            territory,
            border,
            posture,
            enclosure,
            survival,
            hunting,
            actor_snapshots,
        }))
    };

    SearchNode {
        state,
        key,
        analysis,
        children: Vec::new(),
        expansion_complete: false,
        pending_actions: None,
    }
}

#[cfg(test)]
mod tests {
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

    fn state() -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![Coord { x: 3, y: 3 }],
            hazards: vec![],
            snakes: vec![
                snake("ours", &[(1, 1), (1, 0)]),
                snake("enemy", &[(5, 5), (5, 4)]),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        }
    }

    #[test]
    fn node_analysis_scores_every_living_actor() {
        let graph = FutureGraph::new(state());
        let analysis = graph
            .node(graph.root())
            .active_analysis()
            .expect("running root must have analysis");

        let root = graph.node(graph.root());
        let ours = analysis
            .actor_snapshot(&root.state, "ours")
            .expect("our actor evaluation must exist");
        let enemy = analysis
            .actor_snapshot(&root.state, "enemy")
            .expect("enemy actor evaluation must exist");

        assert_eq!(ours.weights.total(), 1000);
        assert_eq!(enemy.weights.total(), 1000);
        assert_eq!(
            analysis
                .actor_snapshots
                .iter()
                .filter(|snapshot| snapshot.is_some())
                .count(),
            2
        );
    }

    #[test]
    fn independent_beam_graph_starts_clean_and_lean_from_current_root() {
        let mut legacy = FutureGraph::new(state());
        legacy.expand_to_depth(1).unwrap();
        assert!(!legacy.node(legacy.root()).children.is_empty());

        let beam = legacy.independent_beam_graph();
        let root = beam.node(beam.root());
        let analysis = root
            .active_analysis()
            .expect("beam root must be analyzable");

        assert_eq!(beam.node_count(), 1);
        assert_eq!(beam.edge_count(), 0);
        assert!(root.children.is_empty());
        assert!(analysis.survival.candidates.is_empty());
        assert!(analysis.hunting.candidates.is_empty());
        assert!(!analysis.actor_snapshots.is_empty());
    }

    #[test]
    fn beam_lean_nodes_skip_legacy_modes_but_keep_actor_snapshots() {
        let mut graph = FutureGraph::new(state());
        graph.use_beam_lean_analysis();
        graph.expand_to_depth(1).unwrap();

        let child = graph.node(graph.node(graph.root()).children[0].child);
        let analysis = child.active_analysis().expect("child must be analyzable");

        assert!(analysis.survival.candidates.is_empty());
        assert!(analysis.hunting.candidates.is_empty());
        assert!(analysis.hunting.plans.is_empty());
        assert_eq!(
            analysis.actor_snapshots.len(),
            child
                .state
                .snakes
                .iter()
                .filter(|snake| snake.alive)
                .count()
        );
    }

    #[test]
    fn expands_simultaneous_turns_to_requested_depth() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(2).unwrap();

        assert!(!graph.node(graph.root()).children.is_empty());
        assert!(graph.node_count() > 1);
        assert!(graph.edge_count() > 0);

        let first_child = graph.node(graph.root()).children[0].child;
        assert_eq!(graph.node(first_child).state.turn, 2);
        assert!(!graph.node(first_child).children.is_empty());
    }

    #[test]
    fn parallel_batch_preserves_deterministic_edge_order() {
        let mut first = FutureGraph::new(state());
        first.use_beam_lean_analysis();
        first.expand_to_depth(1).unwrap();

        let mut second = FutureGraph::new(state());
        second.use_beam_lean_analysis();
        second.expand_to_depth(1).unwrap();

        let signature = |graph: &FutureGraph| {
            graph
                .node(graph.root())
                .children
                .iter()
                .map(|edge| {
                    (
                        edge.joint_action.clone(),
                        graph.node(edge.child).key.clone(),
                        edge.transition.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(signature(&first), signature(&second));
    }

    #[test]
    fn parallel_batch_deduplicates_equivalent_resolved_states() {
        let mut initial = state();
        initial.food.clear();
        initial
            .snakes
            .iter_mut()
            .find(|snake| snake.id == "enemy")
            .unwrap()
            .health = 1;

        let mut graph = FutureGraph::new(initial);
        graph.use_beam_lean_analysis();
        graph.expand_to_depth(1).unwrap();

        let root = graph.root();
        let our_id = graph.node(root).state.our_snake_id.clone();
        let mut found_shared_child = false;

        for direction in crate::direction::Direction::ALL {
            let children = graph
                .node(root)
                .children
                .iter()
                .filter(|edge| edge.joint_action.direction_for(&our_id) == Some(direction))
                .map(|edge| edge.child)
                .collect::<Vec<_>>();
            if children.len() >= 2 && children.windows(2).any(|pair| pair[0] == pair[1]) {
                found_shared_child = true;
                break;
            }
        }

        assert!(found_shared_child);
        assert!(graph.transposition_hits() > 0);
        assert!(graph.edge_count() as usize > graph.node_count().saturating_sub(1));
    }

    #[test]
    fn edge_events_are_kept_outside_child_node() {
        let mut initial = state();
        initial.food = vec![Coord { x: 2, y: 1 }];

        let mut graph = FutureGraph::new(initial);
        graph.expand_to_depth(1).unwrap();

        assert!(graph.node(graph.root()).children.iter().any(|edge| {
            edge.events.iter().any(|event| {
                matches!(
                    event,
                    InstantEvent::AteFood { snake, .. } if snake == "ours"
                )
            })
        }));
    }

    #[test]
    fn entering_border_emits_exposure_event() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let root = graph.root();
        let our_id = graph.node(root).state.our_snake_id.clone();
        let border_edges = graph
            .node(root)
            .children
            .iter()
            .filter(|edge| {
                edge.joint_action.direction_for(&our_id) == Some(crate::direction::Direction::Left)
            })
            .collect::<Vec<_>>();

        assert!(!border_edges.is_empty());
        assert!(border_edges.iter().all(|edge| {
            edge.events
                .iter()
                .any(|event| matches!(event, InstantEvent::SelfBorderExposure { .. }))
        }));
    }

    #[test]
    fn chosen_direction_prunes_other_root_moves_but_keeps_enemy_responses() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(2).unwrap();

        let root = graph.root();
        let our_id = graph.node(root).state.our_snake_id.clone();
        let direction = graph.node(root).children[0]
            .joint_action
            .direction_for(&our_id)
            .unwrap();
        let expected_responses = graph
            .node(root)
            .children
            .iter()
            .filter(|edge| edge.joint_action.direction_for(&our_id) == Some(direction))
            .count();

        graph.retain_chosen_direction(direction);

        assert_eq!(graph.root(), 0);
        assert_eq!(graph.node(0).children.len(), expected_responses);
        assert!(graph
            .node(0)
            .children
            .iter()
            .all(|edge| { edge.joint_action.direction_for(&our_id) == Some(direction) }));
    }

    #[test]
    fn reroot_preserves_node_ids_until_direction_retention_compacts() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(2).unwrap();

        let old_count = graph.node_count();
        let child = graph.node(graph.root()).children[0].child;
        let child_key = graph.node(child).key.clone();

        graph.reroot(child);

        assert_eq!(graph.root(), child);
        assert_eq!(graph.node(graph.root()).key, child_key);
        assert_eq!(graph.node_count(), old_count);

        let our_id = graph.node(graph.root()).state.our_snake_id.clone();
        let direction = graph.node(graph.root()).children[0]
            .joint_action
            .direction_for(&our_id)
            .unwrap();

        graph.retain_chosen_direction(direction);

        assert_eq!(graph.root(), 0);
        assert_eq!(graph.node(graph.root()).key, child_key);
        assert!(graph.node_count() < old_count);
    }

    #[test]
    fn evaluation_and_expansion_support_non_zero_root() {
        use crate::decision::evaluation::evaluate_graph;

        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(2).unwrap();

        let child = graph.node(graph.root()).children[0].child;
        graph.reroot(child);

        assert_ne!(graph.root(), 0);

        let evaluations = evaluate_graph(&graph, 1);
        assert!(!evaluations.is_empty());

        let budget = SearchBudget::for_duration(Duration::from_secs(1));
        let report = graph.expand_depth(1, &budget).unwrap();
        assert!(report.completed);
    }

    #[test]
    fn budgeted_search_reports_only_completed_depths() {
        let mut graph = FutureGraph::new(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(1));

        let report = graph.expand_iteratively(2, 2, &budget).unwrap();

        assert_eq!(report.completed_depth, 2);
        assert!(report.nodes > 1);
        assert!(report.edges > 0);
    }

    #[test]
    fn graph_accepts_updated_opponent_profiles_without_rebuilding_nodes() {
        use crate::enemy::profile::OpponentProfile;

        let mut graph = FutureGraph::new(state());
        let node_count = graph.node_count();
        graph.set_opponent_profiles(OpponentProfiles::from([(
            "enemy".to_string(),
            OpponentProfile {
                hunting_bias_milli: 1200,
                ..OpponentProfile::default()
            },
        )]));

        assert_eq!(graph.node_count(), node_count);
        assert_eq!(graph.opponent_profiles["enemy"].hunting_bias_milli, 1200);
    }

    #[test]
    fn subtree_expansion_only_deepens_from_requested_tip() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let root = graph.root();
        let tip = graph.node(root).children[0].child;
        let root_children_before = graph.node(root).children.len();
        let budget = SearchBudget::for_duration(Duration::from_secs(5));

        let expansion = graph.expand_subtree(tip, 2, &budget).unwrap();

        assert!(expansion.completed);
        assert_eq!(expansion.start_node, tip);
        assert_eq!(expansion.requested_depth, 2);
        assert_eq!(graph.node(root).children.len(), root_children_before);
        assert!(graph.node(tip).expansion_complete());
    }

    #[test]
    fn interrupted_expansion_preserves_generator_and_can_resume() {
        let mut graph = FutureGraph::new(state());
        let root = graph.root();
        let expired = SearchBudget::for_duration(Duration::ZERO);

        let completed = graph.expand_node_budgeted(root, Some(&expired)).unwrap();

        assert!(!completed);
        assert!(graph.nodes[root].pending_actions.is_some());
        assert!(!graph.nodes[root].expansion_complete);

        graph.expand_node(root).unwrap();

        assert!(graph.nodes[root].pending_actions.is_none());
        assert!(graph.nodes[root].expansion_complete);
        assert!(!graph.nodes[root].children.is_empty());
    }

    #[test]
    fn incomplete_root_cache_is_dropped_before_direction_retention() {
        let mut graph = FutureGraph::new(state());
        let root = graph.root();
        let analysis = graph.nodes[root].analysis.as_ref().unwrap();
        let our_moves = analysis.tactical.ours.deterministic_moves;
        graph.nodes[root].pending_actions = Some(JointActionGenerator::new(
            &graph.nodes[root].state,
            our_moves,
            &analysis.tracing,
        ));

        graph.retain_chosen_direction(crate::direction::Direction::Up);

        assert!(graph.nodes[graph.root()].pending_actions.is_none());
        assert!(graph.nodes[graph.root()].children.is_empty());
        assert!(!graph.nodes[graph.root()].expansion_complete);
    }

    #[test]
    fn terminal_nodes_skip_expensive_analysis() {
        let mut terminal = state();
        terminal
            .snakes
            .iter_mut()
            .find(|snake| snake.id == "enemy")
            .unwrap()
            .alive = false;

        let graph = FutureGraph::new(terminal);

        assert!(graph.node(graph.root()).is_terminal());
        assert!(graph.node(graph.root()).analysis.is_none());
    }

    #[test]
    fn food_validation_accepts_any_retained_enemy_response() {
        let mut initial = state();
        initial.food = vec![Coord { x: 2, y: 1 }, Coord { x: 5, y: 6 }];
        let mut graph = FutureGraph::new(initial);
        graph.expand_to_depth(1).unwrap();

        let child_state = graph
            .node(graph.node(graph.root()).children[0].child)
            .state
            .clone();
        let child_key = StateKey::from_state(&child_state);

        assert!(graph.root_children_match_food(&child_key));

        let mut unexpected = child_state;
        unexpected.food = vec![Coord { x: 0, y: 0 }];
        assert!(!graph.root_children_match_food(&StateKey::from_state(&unexpected)));
    }

    #[test]
    fn build_node_reuses_precomputed_state_key() {
        let initial = state();
        let key = StateKey::from_state(&initial);

        let node = build_node_with_key(initial, key.clone(), AnalysisProfile::Full);

        assert_eq!(node.key, key);
    }

    #[test]
    fn depth_expansion_reports_frontier_size_used_for_expansion() {
        let mut graph = FutureGraph::new(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(1));

        let report = graph.expand_depth(1, &budget).unwrap();

        assert!(report.completed);
        assert_eq!(report.frontier_nodes, 1);
    }
}
