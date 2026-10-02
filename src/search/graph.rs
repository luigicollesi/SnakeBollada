use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use rayon::prelude::*;

use crate::analysis::{BorderFobicAnalysis, EnclosureAnalysis, TerritoryAnalysis};
use crate::decision::joint_actions::JointActionGenerator;
use crate::decision::state_key::StateKey;
use crate::direction::MoveMask;
use crate::enemy::profile::OpponentProfiles;
use crate::enemy::tracing::{trace_actor_relative_with_mobility, EnemyTracingOutput};
use crate::evaluation::{
    ActorSnapshot, ActorUtilityMetrics, ActorVec, StrategicWeights, TransitionScore,
};
use crate::simulation::joint_action::JointAction;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::resolver::{resolve_turn, InstantEvent, ResolveError, TurnResolution};
use crate::simulation::state::{ActorIndex, SimulatedGameState};
use crate::spatial::SpatialOccupancy;

use super::actor_priority::ordered_child_ids_for_search;
use super::budget::SearchBudget;

pub(crate) type NodeId = usize;

const MAX_PARALLEL_ACTION_BATCH: usize = 8;
const INITIAL_BATCH_ESTIMATE: Duration = Duration::from_millis(2);
const BATCH_DEADLINE_RESERVE: Duration = Duration::from_millis(1);

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

#[cfg(test)]
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
    pub(crate) transition: TransitionScore,
    pub(crate) child: NodeId,
}

struct ResolvedCandidate {
    joint_action: JointAction,
    state: Option<SimulatedGameState>,
    key: Arc<StateKey>,
    resolution_events: Vec<InstantEvent>,
}

#[derive(Debug, Clone)]
pub(crate) struct NodeAnalysis {
    pub(crate) mobility: Arc<MobilityAnalysis>,
    pub(crate) tracing: Arc<EnemyTracingOutput>,
    pub(crate) territory: Arc<TerritoryAnalysis>,
    pub(crate) actor_snapshots: ActorVec<ActorSnapshot>,
}

impl NodeAnalysis {
    pub(crate) fn actor_snapshot(&self, actor: ActorIndex) -> Option<&ActorSnapshot> {
        self.actor_snapshots.get(actor)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SearchNode {
    pub(crate) state: SimulatedGameState,
    pub(crate) key: Arc<StateKey>,
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
    transpositions: HashMap<Arc<StateKey>, NodeId>,
    transposition_hits: u32,
    edge_count: u32,
    opponent_profiles: OpponentProfiles,
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
        let root_node = build_node(root_state);
        let root_key = root_node.key.clone();

        Self {
            root: 0,
            nodes: vec![root_node],
            transpositions: HashMap::from([(root_key, 0)]),
            transposition_hits: 0,
            edge_count: 0,
            opponent_profiles,
            action_batch_estimate: INITIAL_BATCH_ESTIMATE,
            perf_stats: GraphPerfStats::default(),
        }
    }

    pub(crate) fn new_beam(root_state: SimulatedGameState) -> Self {
        Self::new(root_state)
    }

    pub(crate) fn new_beam_with_opponent_profiles(
        root_state: SimulatedGameState,
        opponent_profiles: OpponentProfiles,
    ) -> Self {
        Self::new_with_opponent_profiles(root_state, opponent_profiles)
    }

    pub(crate) fn set_opponent_profiles(&mut self, opponent_profiles: OpponentProfiles) {
        self.opponent_profiles = opponent_profiles;
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

        let root_state = &self.nodes[self.root].state;
        let Some(our_actor) = root_state.actor_index(&root_state.our_snake_id) else {
            self.nodes[self.root].children.clear();
            self.garbage_collect();
            return;
        };
        self.nodes[self.root]
            .children
            .retain(|edge| edge.joint_action.direction_for(our_actor) == Some(direction));
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

    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
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
        let deterministic_moves = parent_analysis
            .mobility
            .deterministic_moves_for(&state, &state.our_snake_id);
        let our_moves = if deterministic_moves.is_empty() {
            MoveMask::all()
        } else {
            deterministic_moves
        };

        let mut actions = self.nodes[node_id]
            .pending_actions
            .take()
            .unwrap_or_else(|| {
                JointActionGenerator::new_actor_relative_with_profiles(
                    &state,
                    our_moves,
                    &tracing,
                    &self.opponent_profiles,
                )
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
                        let key = Arc::new(StateKey::from_beam_state(&resolution.state));
                        let TurnResolution { state, events } = resolution;
                        ResolvedCandidate {
                            joint_action,
                            state: Some(state),
                            key,
                            resolution_events: events,
                        }
                    })
                })
                .collect::<Result<Vec<_>, ResolveError>>()?;
            let resolve_elapsed = resolve_started.elapsed();

            let mut unique_indices = Vec::new();
            let mut seen_new = HashSet::<&StateKey>::new();
            let mut batch_transposition_hits = 0_u32;

            for (index, candidate) in resolved.iter().enumerate() {
                if self.transpositions.contains_key(&candidate.key) {
                    batch_transposition_hits = batch_transposition_hits.saturating_add(1);
                } else if seen_new.insert(candidate.key.as_ref()) {
                    unique_indices.push(index);
                } else {
                    batch_transposition_hits = batch_transposition_hits.saturating_add(1);
                }
            }
            drop(seen_new);

            let mut resolved = resolved;
            let unique_new = unique_indices
                .into_iter()
                .map(|index| {
                    let candidate = &mut resolved[index];
                    let state = candidate
                        .state
                        .take()
                        .expect("new resolved state must still be owned by candidate");
                    (Arc::clone(&candidate.key), state)
                })
                .collect::<Vec<_>>();

            for candidate in &mut resolved {
                candidate.state = None;
            }

            let node_build_started = std::time::Instant::now();
            let built_nodes = unique_new
                .into_par_iter()
                .map(|(key, state)| build_node_with_key(state, key))
                .collect::<Vec<_>>();
            let node_build_elapsed = node_build_started.elapsed();
            let built_node_count = built_nodes.len().try_into().unwrap_or(u32::MAX);

            let merge_started = std::time::Instant::now();
            for node in built_nodes {
                if self.transpositions.contains_key(node.key.as_ref()) {
                    batch_transposition_hits = batch_transposition_hits.saturating_add(1);
                    continue;
                }
                let child = self.nodes.len();
                self.transpositions.insert(Arc::clone(&node.key), child);
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
                    let transition = TransitionScore::from_parts(
                        parent,
                        &candidate.resolution_events,
                        &nodes[child],
                    );

                    SearchEdge {
                        joint_action: candidate.joint_action,
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

fn build_node(state: SimulatedGameState) -> SearchNode {
    let key = Arc::new(StateKey::from_beam_state(&state));
    build_node_with_key(state, key)
}

fn build_node_with_key(state: SimulatedGameState, key: Arc<StateKey>) -> SearchNode {
    let analysis = if is_terminal_state(&state) {
        None
    } else {
        let spatial = Arc::new(SpatialOccupancy::from_state(&state));
        let mobility = Arc::new(MobilityAnalysis::from_spatial(Arc::clone(&spatial)));
        let territory = Arc::new(TerritoryAnalysis::from_spatial_actor_relative(
            &state, &spatial,
        ));
        let tracing = Arc::new(trace_actor_relative_with_mobility(&state, &mobility));
        let border = BorderFobicAnalysis::from_parts_with_territory_actor_relative(
            &state, &mobility, &territory,
        );
        let enclosure = EnclosureAnalysis::from_parts_actor_relative(&state, &territory, &mobility);

        let actor_snapshots = state
            .snakes
            .par_iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive)
            .filter_map(|(index, snake)| {
                let actor = ActorIndex::new(index)?;
                let metrics = ActorUtilityMetrics::from_parts(
                    &state, actor, &mobility, &territory, &enclosure, &border,
                )?;
                let weights = StrategicWeights::for_actor(
                    &state,
                    &snake.id,
                    metrics.space_capacity_milli,
                    metrics.territory_control_milli,
                )?;
                Some((actor, ActorSnapshot::new(metrics, weights)))
            })
            .collect::<Vec<_>>()
            .into_iter()
            .collect::<ActorVec<_>>();

        Some(Arc::new(NodeAnalysis {
            mobility,
            tracing,
            territory,
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
    use crate::simulation::state::{RulesContext, SimulatedSnake};
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
        }
    }

    fn actor(state: &SimulatedGameState, actor_id: &str) -> ActorIndex {
        state.actor_index(actor_id).expect("actor must exist")
    }

    #[test]
    fn node_analysis_scores_every_living_actor() {
        let graph = FutureGraph::new(state());
        let analysis = graph
            .node(graph.root())
            .active_analysis()
            .expect("running root must have analysis");

        let ours = analysis
            .actor_snapshot(actor(&graph.node(graph.root()).state, "ours"))
            .expect("our actor evaluation must exist");
        let enemy = analysis
            .actor_snapshot(actor(&graph.node(graph.root()).state, "enemy"))
            .expect("enemy actor evaluation must exist");

        assert_eq!(ours.weights.total(), 1000);
        assert_eq!(enemy.weights.total(), 1000);
        assert_eq!(analysis.actor_snapshots.len(), 2);
    }

    #[test]
    fn beam_food_utility_reuses_territory_distance_without_route_analysis() {
        let mut initial = state();
        initial.food = vec![Coord { x: 3, y: 1 }];

        let graph = FutureGraph::new(initial);
        let root = graph.node(graph.root());
        let analysis = root
            .active_analysis()
            .expect("beam root must have analysis");
        let ours = analysis
            .actor_snapshot(actor(&root.state, "ours"))
            .expect("our actor snapshot must exist");

        assert!(ours.metrics.food_potential_milli > 0);
        assert_eq!(
            analysis
                .territory
                .distance_for("ours", Coord { x: 3, y: 1 }),
            Some(2)
        );
    }

    #[test]
    fn actor_relative_nodes_keep_actor_snapshots() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let child = graph.node(graph.node(graph.root()).children[0].child);
        let analysis = child.active_analysis().expect("child must be analyzable");

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
        first.expand_to_depth(1).unwrap();

        let mut second = FutureGraph::new(state());
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
        graph.expand_to_depth(1).unwrap();

        let root = graph.root();
        let our_actor = graph
            .node(root)
            .state
            .actor_index(&graph.node(root).state.our_snake_id)
            .unwrap();
        let mut found_shared_child = false;

        for direction in crate::direction::Direction::ALL {
            let children = graph
                .node(root)
                .children
                .iter()
                .filter(|edge| edge.joint_action.direction_for(our_actor) == Some(direction))
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
    fn food_events_are_folded_into_cached_transition_scores() {
        let mut initial = state();
        initial.food = vec![Coord { x: 2, y: 1 }];

        let mut graph = FutureGraph::new(initial);
        graph.expand_to_depth(1).unwrap();

        let root = graph.root();
        let our_actor = graph
            .node(root)
            .state
            .actor_index(&graph.node(root).state.our_snake_id)
            .unwrap();

        assert!(graph.node(root).children.iter().any(|edge| {
            edge.transition
                .for_actor(our_actor)
                .is_some_and(|score| score.food_benefit > 0)
        }));
    }

    #[test]
    fn entering_border_is_penalized_by_transition_utility() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let root = graph.root();
        let our_actor = graph
            .node(root)
            .state
            .actor_index(&graph.node(root).state.our_snake_id)
            .unwrap();
        let border_edges = graph
            .node(root)
            .children
            .iter()
            .filter(|edge| {
                edge.joint_action.direction_for(our_actor)
                    == Some(crate::direction::Direction::Left)
            })
            .collect::<Vec<_>>();

        assert!(!border_edges.is_empty());
        assert!(border_edges.iter().all(|edge| {
            edge.transition
                .for_actor(our_actor)
                .is_some_and(|score| score.survival_harm > 0)
        }));
    }

    #[test]
    fn chosen_direction_prunes_other_root_moves_but_keeps_enemy_responses() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(2).unwrap();

        let root = graph.root();
        let our_actor = graph
            .node(root)
            .state
            .actor_index(&graph.node(root).state.our_snake_id)
            .unwrap();
        let direction = graph.node(root).children[0]
            .joint_action
            .direction_for(our_actor)
            .unwrap();
        let expected_responses = graph
            .node(root)
            .children
            .iter()
            .filter(|edge| edge.joint_action.direction_for(our_actor) == Some(direction))
            .count();

        graph.retain_chosen_direction(direction);

        assert_eq!(graph.root(), 0);
        assert_eq!(graph.node(0).children.len(), expected_responses);
        assert!(graph
            .node(0)
            .children
            .iter()
            .all(|edge| { edge.joint_action.direction_for(our_actor) == Some(direction) }));
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

        let our_actor = graph
            .node(graph.root())
            .state
            .actor_index(&graph.node(graph.root()).state.our_snake_id)
            .unwrap();
        let direction = graph.node(graph.root()).children[0]
            .joint_action
            .direction_for(our_actor)
            .unwrap();

        graph.retain_chosen_direction(direction);

        assert_eq!(graph.root(), 0);
        assert_eq!(graph.node(graph.root()).key, child_key);
        assert!(graph.node_count() < old_count);
    }

    #[test]
    fn expansion_supports_non_zero_root() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(2).unwrap();

        let child = graph.node(graph.root()).children[0].child;
        graph.reroot(child);

        assert_ne!(graph.root(), 0);

        let budget = SearchBudget::for_duration(Duration::from_secs(1));
        let report = graph.expand_depth(1, &budget).unwrap();
        assert!(report.completed);
        assert!(!graph.node(graph.root()).children.is_empty());
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
        let root_state = &graph.nodes[root].state;
        let our_moves = analysis
            .mobility
            .deterministic_moves_for(root_state, &root_state.our_snake_id);
        graph.nodes[root].pending_actions =
            Some(JointActionGenerator::new_actor_relative_with_profiles(
                root_state,
                our_moves,
                &analysis.tracing,
                &OpponentProfiles::default(),
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
        let child_key = StateKey::from_beam_state(&child_state);

        assert!(graph.root_children_match_food(&child_key));

        let mut unexpected = child_state;
        unexpected.food = vec![Coord { x: 0, y: 0 }];
        assert!(!graph.root_children_match_food(&StateKey::from_beam_state(&unexpected)));
    }

    #[test]
    fn build_node_reuses_precomputed_state_key() {
        let initial = state();
        let key = Arc::new(StateKey::from_beam_state(&initial));

        let node = build_node_with_key(initial, Arc::clone(&key));

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
