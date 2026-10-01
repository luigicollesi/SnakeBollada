#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use crate::analysis::transition::analyze_transition;
use crate::analysis::{
    BorderFobicAnalysis, EnclosureAnalysis, StateAnalysis, StrategicPosture, TacticalStateAnalysis,
    TerritoryAnalysis,
};
use crate::decision::joint_actions::JointActionGenerator;
use crate::decision::state_key::StateKey;
use crate::direction::MoveMask;
use crate::enemy::profile::OpponentProfiles;
use crate::enemy::tracing::{trace_with_mobility, EnemyTracingOutput};
use crate::modes::hunting::{self, HuntingModeOutput};
use crate::modes::survival::{self, SurvivalModeOutput};
use crate::simulation::joint_action::JointAction;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::resolver::{resolve_turn, ForecastDelta, InstantEvent, ResolveError};
use crate::simulation::state::SimulatedGameState;

use super::budget::SearchBudget;

pub(crate) type NodeId = usize;

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

#[derive(Debug, Clone)]
pub(crate) struct SearchEdge {
    pub(crate) joint_action: JointAction,
    pub(crate) events: Vec<InstantEvent>,
    pub(crate) forecast_delta: ForecastDelta,
    pub(crate) child: NodeId,
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
        }
    }

    pub(crate) fn set_opponent_profiles(&mut self, opponent_profiles: OpponentProfiles) {
        self.opponent_profiles = opponent_profiles;
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

    pub(crate) fn root_children_match_food(&self, actual_food: &[crate::Coord]) -> bool {
        let mut actual = actual_food.to_vec();
        actual.sort_unstable();

        self.nodes[self.root].children.iter().any(|edge| {
            let mut expected = self.nodes[edge.child].state.food.clone();
            expected.sort_unstable();
            expected == actual
        })
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
        self.garbage_collect();
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

        let mut nodes = Vec::with_capacity(order.len());
        for old_id in order {
            let mut node = self.nodes[old_id].clone();
            node.children = node
                .children
                .into_iter()
                .filter_map(|mut edge| {
                    let child = remap.get(&edge.child).copied()?;
                    edge.child = child;
                    Some(edge)
                })
                .collect();
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
        self.edge_count = self
            .nodes
            .iter()
            .map(|node| node.children.len() as u32)
            .sum();
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
            let complete = self.expand_depth(depth, budget)?;
            if !complete {
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
    ) -> Result<bool, SearchError> {
        let parents = self.nodes_at_depth(depth.saturating_sub(1));

        for node_id in parents {
            if budget.expired() {
                self.garbage_collect();
                return Ok(false);
            }

            if !self.expand_node_budgeted(node_id, Some(budget))? {
                self.garbage_collect();
                return Ok(false);
            }
        }

        Ok(true)
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

        let mut actions = self.nodes[node_id]
            .pending_actions
            .take()
            .unwrap_or_else(|| {
                JointActionGenerator::new_with_profiles(
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

            let Some(joint_action) = actions.next() else {
                self.nodes[node_id].pending_actions = None;
                self.nodes[node_id].expansion_complete = true;
                return Ok(true);
            };

            let resolution = resolve_turn(&state, &joint_action)?;
            let child_key = StateKey::from_state(&resolution.state);

            let child = if let Some(existing) = self.transpositions.get(&child_key).copied() {
                self.transposition_hits = self.transposition_hits.saturating_add(1);
                existing
            } else {
                let child = self.nodes.len();
                let node = build_node(resolution.state.clone());
                self.transpositions.insert(child_key, child);
                self.nodes.push(node);
                child
            };

            let events = if let Some(child_analysis) = self.nodes[child].analysis.as_ref() {
                let after_tactical = Arc::clone(&child_analysis.tactical);
                analyze_transition(&before_tactical, &resolution, &after_tactical).events
            } else {
                resolution.events.clone()
            };

            self.nodes[node_id].children.push(SearchEdge {
                joint_action,
                events,
                forecast_delta: resolution.forecast_delta,
                child,
            });
            self.edge_count = self.edge_count.saturating_add(1);
        }
    }

    fn is_terminal(&self, node_id: NodeId) -> bool {
        self.nodes[node_id].is_terminal()
    }
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
    let key = StateKey::from_state(&state);
    let analysis = if is_terminal_state(&state) {
        None
    } else {
        let state_analysis = Arc::new(StateAnalysis::from_simulated(&state));
        let mobility = Arc::new(MobilityAnalysis::from_state(&state));
        let tracing = Arc::new(trace_with_mobility(&state, &state_analysis, &mobility));
        let tactical = Arc::new(TacticalStateAnalysis::from_parts(
            &state, &tracing, &mobility,
        ));
        let territory = Arc::new(TerritoryAnalysis::from_state(&state));
        let border = Arc::new(BorderFobicAnalysis::from_parts_with_territory(
            &state,
            &tactical,
            &territory,
        ));
        let posture = Arc::new(StrategicPosture::from_state(&state));
        let enclosure = Arc::new(EnclosureAnalysis::from_parts(&state, &territory, &tactical));
        let survival = Arc::new(survival::analyze_with_border(&state, &tactical, &border));
        let hunting = Arc::new(hunting::analyze(
            &state, &tactical, &tracing, &territory, &enclosure,
        ));
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
    fn reroot_discards_unreachable_past() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(2).unwrap();

        let old_count = graph.node_count();
        let child = graph.node(graph.root()).children[0].child;
        let child_key = graph.node(child).key.clone();

        graph.reroot(child);

        assert_eq!(graph.root(), 0);
        assert_eq!(graph.node(0).key, child_key);
        assert!(graph.node_count() < old_count);
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

        let child_food = graph
            .node(graph.node(graph.root()).children[0].child)
            .state
            .food
            .clone();

        assert!(graph.root_children_match_food(&child_food));
        assert!(!graph.root_children_match_food(&[Coord { x: 0, y: 0 }]));
    }
}
