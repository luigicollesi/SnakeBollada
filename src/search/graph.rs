#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::analysis::transition::analyze_transition;
use crate::analysis::{StateAnalysis, TacticalStateAnalysis};
use crate::decision::joint_actions::JointActionGenerator;
use crate::decision::state_key::StateKey;
use crate::direction::MoveMask;
use crate::enemy::tracing::{trace, EnemyTracingOutput};
use crate::simulation::joint_action::JointAction;
use crate::simulation::resolver::{resolve_turn, ForecastDelta, InstantEvent, ResolveError};
use crate::simulation::state::SimulatedGameState;

pub(crate) type NodeId = usize;

#[derive(Debug, Clone)]
pub(crate) struct SearchEdge {
    pub(crate) joint_action: JointAction,
    pub(crate) events: Vec<InstantEvent>,
    pub(crate) forecast_delta: ForecastDelta,
    pub(crate) child: NodeId,
}

#[derive(Debug, Clone)]
pub(crate) struct SearchNode {
    pub(crate) state: SimulatedGameState,
    pub(crate) key: StateKey,
    pub(crate) analysis: Arc<StateAnalysis>,
    pub(crate) tracing: Arc<EnemyTracingOutput>,
    pub(crate) tactical: Arc<TacticalStateAnalysis>,
    pub(crate) children: Vec<SearchEdge>,
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
}

impl FutureGraph {
    pub(crate) fn new(root_state: SimulatedGameState) -> Self {
        let root_node = build_node(root_state);
        let root_key = root_node.key.clone();

        Self {
            root: 0,
            nodes: vec![root_node],
            transpositions: HashMap::from([(root_key, 0)]),
            transposition_hits: 0,
            edge_count: 0,
        }
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
        if !self.nodes[node_id].children.is_empty() || self.is_terminal(node_id) {
            return Ok(());
        }

        let state = self.nodes[node_id].state.clone();
        let tracing = Arc::clone(&self.nodes[node_id].tracing);
        let before_tactical = Arc::clone(&self.nodes[node_id].tactical);

        let our_moves = if before_tactical.ours.deterministic_moves.is_empty() {
            MoveMask::all()
        } else {
            before_tactical.ours.deterministic_moves
        };

        let actions = JointActionGenerator::new(&state, our_moves, &tracing);
        let mut edges = Vec::with_capacity(actions.estimated_count());

        for joint_action in actions {
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

            let after_tactical = Arc::clone(&self.nodes[child].tactical);
            let transition = analyze_transition(&before_tactical, &resolution, &after_tactical);

            edges.push(SearchEdge {
                joint_action,
                events: transition.events,
                forecast_delta: resolution.forecast_delta,
                child,
            });
            self.edge_count = self.edge_count.saturating_add(1);
        }

        self.nodes[node_id].children = edges;
        Ok(())
    }

    fn is_terminal(&self, node_id: NodeId) -> bool {
        let state = &self.nodes[node_id].state;
        let ours_alive = state
            .snake(&state.our_snake_id)
            .is_some_and(|snake| snake.alive);
        let living_enemies = state
            .snakes
            .iter()
            .any(|snake| snake.alive && snake.id != state.our_snake_id);

        !ours_alive || !living_enemies
    }
}

fn build_node(state: SimulatedGameState) -> SearchNode {
    let key = StateKey::from_state(&state);
    let analysis = Arc::new(StateAnalysis::from_simulated(&state));
    let tracing = Arc::new(trace(&state, &analysis));
    let tactical = Arc::new(TacticalStateAnalysis::from_state(&state, &tracing));

    SearchNode {
        state,
        key,
        analysis,
        tracing,
        tactical,
        children: Vec::new(),
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
}
