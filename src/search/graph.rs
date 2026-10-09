//! Single-purpose persistent FutureGraph for Hobbs evaluation.
//! Physical Battlesnake turn resolution is shared with the standard simulator.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use rayon::prelude::*;

use crate::decision::joint_actions::JointActionGenerator;
use crate::decision::state_key::StateKey;
use crate::simulation::joint_action::JointAction;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::resolver::{resolve_turn, ResolveError};
use crate::simulation::state::SimulatedGameState;

use super::budget::SearchBudget;
use super::forecast::{FoodForecastPolicy, ForecastDelta};

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
}

#[derive(Debug, Clone)]
pub(crate) struct SearchEdge {
    pub(crate) joint_action: JointAction,
    pub(crate) forecast_delta: ForecastDelta,
    pub(crate) child: NodeId,
}

struct ResolvedCandidate {
    joint_action: JointAction,
    state: Option<SimulatedGameState>,
    key: Arc<StateKey>,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchNode {
    pub(crate) state: SimulatedGameState,
    pub(crate) key: Arc<StateKey>,
    pub(crate) children: Vec<SearchEdge>,
    expansion_complete: bool,
    pending_actions: Option<JointActionGenerator>,
}

impl SearchNode {
    pub(crate) fn is_terminal(&self) -> bool {
        is_terminal_state(&self.state)
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
    forecast_policy: FoodForecastPolicy,
    action_batch_estimate: Duration,
    perf_stats: GraphPerfStats,
}

impl FutureGraph {
    #[cfg(test)]
    pub(crate) fn new_beam(root_state: SimulatedGameState) -> Self {
        Self::new_beam_with_forecast(root_state, FoodForecastPolicy::default())
    }

    pub(crate) fn new_beam_with_forecast(
        root_state: SimulatedGameState,
        forecast_policy: FoodForecastPolicy,
    ) -> Self {
        let node = build_node(root_state);
        let key = Arc::clone(&node.key);
        Self {
            root: 0,
            nodes: vec![node],
            transpositions: HashMap::from([(key, 0)]),
            transposition_hits: 0,
            edge_count: 0,
            forecast_policy,
            action_batch_estimate: INITIAL_BATCH_ESTIMATE,
            perf_stats: GraphPerfStats::default(),
        }
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

    /// Expand all legal joint responses for a node. Incomplete expansion is
    /// never reported as an exact minimax depth.
    pub(crate) fn expand_one(
        &mut self,
        node_id: NodeId,
        budget: &SearchBudget,
    ) -> Result<bool, SearchError> {
        self.expand_node_budgeted(node_id, Some(budget))
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
        let mobility = MobilityAnalysis::from_state(&state);
        let deterministic_moves = mobility.deterministic_moves_for(&state, &state.our_snake_id);
        let our_moves = if deterministic_moves.is_empty() {
            mobility.in_bounds_moves_for(&state, &state.our_snake_id)
        } else {
            deterministic_moves
        };

        let mut actions = self.nodes[node_id]
            .pending_actions
            .take()
            .unwrap_or_else(|| JointActionGenerator::new(&state, our_moves, &mobility));

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
                        ResolvedCandidate {
                            joint_action,
                            state: Some(resolution.state),
                            key,
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

            let nodes = &self.nodes;
            let forecast = self.forecast_policy;
            let prepared_edges = resolved
                .into_par_iter()
                .zip(child_ids.into_par_iter())
                .map(|(candidate, child)| SearchEdge {
                    joint_action: candidate.joint_action,
                    forecast_delta: forecast.delta_after(&nodes[child].state),
                    child,
                })
                .collect::<Vec<_>>();

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
    SearchNode {
        state,
        key,
        children: Vec::new(),
        expansion_complete: false,
        pending_actions: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedSnake};
    use crate::Coord;
    use std::time::Duration;

    fn state() -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![Coord { x: 3, y: 3 }],
            hazards: vec![],
            snakes: vec![
                SimulatedSnake {
                    id: "ours".into(),
                    health: 100,
                    body: vec![Coord { x: 1, y: 2 }, Coord { x: 1, y: 1 }],
                    alive: true,
                },
                SimulatedSnake {
                    id: "enemy".into(),
                    health: 100,
                    body: vec![Coord { x: 5, y: 5 }, Coord { x: 5, y: 4 }],
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

    #[test]
    fn expands_joint_responses_without_legacy_scores() {
        let mut graph = FutureGraph::new_beam(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(4));
        assert!(graph.expand_one(graph.root(), &budget).unwrap());
        assert!(graph.edge_count() >= 3);
        assert!(graph.node_count() >= 2);
        assert!(graph
            .node(graph.root())
            .children
            .iter()
            .all(|edge| edge.child != graph.root()));
    }

    #[test]
    fn chosen_move_keeps_opponent_responses_and_enables_reroot() {
        let mut graph = FutureGraph::new_beam(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(4));
        assert!(graph.expand_one(graph.root(), &budget).unwrap());
        let actor = graph.node(graph.root()).state.actor_index("ours").unwrap();
        let selected = graph.node(graph.root()).children[0]
            .joint_action
            .direction_for(actor)
            .unwrap();
        graph.retain_chosen_direction(selected);
        assert!(graph
            .node(graph.root())
            .children
            .iter()
            .all(|edge| edge.joint_action.direction_for(actor) == Some(selected)));
        let child = graph.node(graph.root()).children[0].child;
        let key = graph.node(child).key.clone();
        let found = graph.find_node_by_key(&key).unwrap();
        graph.reroot(found);
        assert_eq!(graph.node(graph.root()).key.as_ref(), key.as_ref());
    }

    #[test]
    fn empty_deadline_leaves_expansion_uncommitted() {
        let mut graph = FutureGraph::new_beam(state());
        let budget = SearchBudget::for_duration(Duration::ZERO);
        assert!(!graph.expand_one(graph.root(), &budget).unwrap());
    }
}
