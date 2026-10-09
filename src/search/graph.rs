//! Single-purpose persistent FutureGraph for Hobbs evaluation.
//! Physical Battlesnake turn resolution is shared with the standard simulator.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::decision::joint_actions::JointActionGenerator;
use crate::decision::state_key::StateKey;
use crate::direction::{Direction, MoveMask};
use crate::simulation::joint_action::JointAction;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::resolver::{resolve_turn, ResolveError};
use crate::simulation::state::SimulatedGameState;

use super::budget::SearchBudget;
use super::forecast::{FoodForecastPolicy, ForecastDelta};

pub(crate) type NodeId = usize;

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

#[derive(Debug, Clone)]
pub(crate) enum ResponseLookup {
    Edge(SearchEdge),
    Exhausted,
    Deadline,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchNode {
    pub(crate) state: SimulatedGameState,
    pub(crate) key: Arc<StateKey>,
    pub(crate) children: Vec<SearchEdge>,
    generators_initialized: bool,
    pending_by_direction: HashMap<Direction, JointActionGenerator>,
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

    pub(crate) fn retain_chosen_direction(&mut self, direction: Direction) {
        // Alpha-Beta can stop with incomplete opponent alternatives.
        // Preserve matching edges without retaining generators for unchosen moves.
        let root = self.root;
        let state = &self.nodes[root].state;
        if let Some(actor) = state.actor_index(&state.our_snake_id) {
            self.nodes[root]
                .children
                .retain(|edge| edge.joint_action.direction_for(actor) == Some(direction));
        } else {
            self.nodes[root].children.clear();
        }
        self.nodes[root].pending_by_direction.clear();
        self.nodes[root].generators_initialized = true;
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

    fn initialize_generators(&mut self, node_id: NodeId) {
        if self.nodes[node_id].generators_initialized || self.is_terminal(node_id) {
            return;
        }
        let state = &self.nodes[node_id].state;
        let mobility = MobilityAnalysis::from_state(state);
        let deterministic = mobility.deterministic_moves_for(state, &state.our_snake_id);
        let our_moves = if deterministic.is_empty() {
            mobility.in_bounds_moves_for(state, &state.our_snake_id)
        } else {
            deterministic
        };
        let mut generators = HashMap::new();
        for direction in our_moves.iter() {
            generators.insert(
                direction,
                JointActionGenerator::new(state, MoveMask::single(direction), &mobility),
            );
        }
        self.nodes[node_id].pending_by_direction = generators;
        self.nodes[node_id].generators_initialized = true;
    }

    /// Directions generated for this state, including previously cached edges.
    pub(crate) fn available_directions(&mut self, node_id: NodeId) -> Vec<Direction> {
        self.initialize_generators(node_id);
        let node = &self.nodes[node_id];
        let actor = node.state.actor_index(&node.state.our_snake_id);
        Direction::ALL
            .into_iter()
            .filter(|direction| {
                node.pending_by_direction.contains_key(direction)
                    || actor.is_some_and(|actor| {
                        node.children
                            .iter()
                            .any(|edge| edge.joint_action.direction_for(actor) == Some(*direction))
                    })
            })
            .collect()
    }

    /// Return already resolved responses for BestFirst ordering.
    pub(crate) fn known_responses(&self, node_id: NodeId, direction: Direction) -> Vec<SearchEdge> {
        let node = &self.nodes[node_id];
        let Some(actor) = node.state.actor_index(&node.state.our_snake_id) else {
            return Vec::new();
        };
        node.children
            .iter()
            .filter(|edge| edge.joint_action.direction_for(actor) == Some(direction))
            .cloned()
            .collect()
    }

    /// Resolve at most one new simultaneous action, leaving other replies
    /// pending when Alpha-Beta cuts off a direction.
    pub(crate) fn next_response(
        &mut self,
        node_id: NodeId,
        direction: Direction,
        response_index: usize,
        budget: &SearchBudget,
    ) -> Result<ResponseLookup, SearchError> {
        self.initialize_generators(node_id);
        if let Some(edge) = self
            .known_responses(node_id, direction)
            .into_iter()
            .nth(response_index)
        {
            return Ok(ResponseLookup::Edge(edge));
        }
        if !self.nodes[node_id]
            .pending_by_direction
            .contains_key(&direction)
        {
            return Ok(ResponseLookup::Exhausted);
        }
        if budget.expired() {
            return Ok(ResponseLookup::Deadline);
        }
        let action = self.nodes[node_id]
            .pending_by_direction
            .get_mut(&direction)
            .and_then(Iterator::next);
        let Some(action) = action else {
            self.nodes[node_id].pending_by_direction.remove(&direction);
            return Ok(ResponseLookup::Exhausted);
        };

        let started = std::time::Instant::now();
        let resolved = resolve_turn(&self.nodes[node_id].state, &action)?;
        let resolve_us = duration_us(started.elapsed());
        let merge_started = std::time::Instant::now();
        let key = Arc::new(StateKey::from_beam_state(&resolved.state));
        let child = if let Some(&existing) = self.transpositions.get(key.as_ref()) {
            self.transposition_hits = self.transposition_hits.saturating_add(1);
            existing
        } else {
            let child = self.nodes.len();
            self.transpositions.insert(Arc::clone(&key), child);
            self.nodes.push(build_node_with_key(resolved.state, key));
            self.perf_stats.new_nodes_built = self.perf_stats.new_nodes_built.saturating_add(1);
            child
        };
        let edge = SearchEdge {
            joint_action: action,
            forecast_delta: self.forecast_policy.delta_after(&self.nodes[child].state),
            child,
        };
        self.nodes[node_id].children.push(edge.clone());
        self.edge_count = self.edge_count.saturating_add(1);
        self.perf_stats.action_batches = self.perf_stats.action_batches.saturating_add(1);
        self.perf_stats.resolved_actions = self.perf_stats.resolved_actions.saturating_add(1);
        self.perf_stats.resolve_us = self.perf_stats.resolve_us.saturating_add(resolve_us);
        self.perf_stats.merge_us = self
            .perf_stats
            .merge_us
            .saturating_add(duration_us(merge_started.elapsed()));
        Ok(ResponseLookup::Edge(edge))
    }

    #[cfg(test)]
    pub(crate) fn expand_one(
        &mut self,
        node_id: NodeId,
        budget: &SearchBudget,
    ) -> Result<bool, SearchError> {
        for direction in self.available_directions(node_id) {
            let mut index = 0;
            loop {
                match self.next_response(node_id, direction, index, budget)? {
                    ResponseLookup::Edge(_) => index += 1,
                    ResponseLookup::Exhausted => break,
                    ResponseLookup::Deadline => return Ok(false),
                }
            }
        }
        Ok(true)
    }

    fn is_terminal(&self, node_id: NodeId) -> bool {
        self.nodes[node_id].is_terminal()
    }
}

fn duration_us(duration: std::time::Duration) -> u64 {
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
        generators_initialized: false,
        pending_by_direction: HashMap::new(),
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
