#![allow(dead_code)]

use std::collections::HashMap;

use crate::direction::Direction;
use crate::simulation::resolver::{EliminationAttribution, InstantEvent};

use super::graph::{FutureGraph, NodeId, SearchEdge, SearchNode};

const FORCING_SCALE: i64 = 4_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActorEdgePriority {
    pub(crate) our_opportunity: i64,
    pub(crate) our_harm: i64,
    pub(crate) strongest_enemy_opportunity: i64,
    pub(crate) strongest_enemy_harm: i64,
    pub(crate) forcing: i64,
}

impl ActorEdgePriority {
    pub(crate) fn our_search_score(self) -> i64 {
        self.our_opportunity
            .saturating_add(self.strongest_enemy_harm)
            .saturating_add(self.forcing.saturating_div(2))
    }

    pub(crate) fn opponent_search_score(self) -> i64 {
        self.our_harm
            .saturating_mul(2)
            .saturating_add(self.strongest_enemy_opportunity)
            .saturating_add(self.forcing)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RankedEdge {
    edge_index: usize,
    direction: Direction,
    direction_score: i64,
    opponent_score: i64,
    child: NodeId,
}

pub(crate) fn edge_priority(
    parent: &SearchNode,
    edge: &SearchEdge,
    child: &SearchNode,
) -> ActorEdgePriority {
    let our_id = parent.state.our_snake_id.as_str();
    let our_before = actor_net(parent, our_id).unwrap_or(0);
    let our_after = actor_net(child, our_id).unwrap_or_else(|| {
        if child
            .state
            .snake(our_id)
            .is_some_and(|snake| !snake.alive)
        {
            -1_000_000_000
        } else {
            0
        }
    });
    let our_delta = our_after.saturating_sub(our_before);

    let mut strongest_enemy_opportunity = 0_i64;
    let mut strongest_enemy_harm = 0_i64;

    for enemy in parent
        .state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != our_id)
    {
        let Some(before) = actor_net(parent, &enemy.id) else {
            continue;
        };
        let after = actor_net(child, &enemy.id).unwrap_or_else(|| {
            if child
                .state
                .snake(&enemy.id)
                .is_some_and(|snake| !snake.alive)
            {
                -1_000_000_000
            } else {
                before
            }
        });
        let delta = after.saturating_sub(before);
        strongest_enemy_opportunity = strongest_enemy_opportunity.max(delta.max(0));
        strongest_enemy_harm = strongest_enemy_harm.max(delta.saturating_neg().max(0));
    }

    ActorEdgePriority {
        our_opportunity: our_delta.max(0),
        our_harm: our_delta.saturating_neg().max(0),
        strongest_enemy_opportunity,
        strongest_enemy_harm,
        forcing: forcing_score(parent, edge),
    }
}

pub(crate) fn ordered_child_ids_for_search(
    graph: &FutureGraph,
    node_id: NodeId,
) -> Vec<NodeId> {
    let node = graph.node(node_id);
    let our_id = node.state.our_snake_id.as_str();
    let mut direction_scores = HashMap::<Direction, i64>::new();
    let mut ranked = Vec::with_capacity(node.children.len());

    for (edge_index, edge) in node.children.iter().enumerate() {
        let Some(direction) = edge.joint_action.direction_for(our_id) else {
            continue;
        };
        let priority = edge_priority(node, edge, graph.node(edge.child));
        direction_scores
            .entry(direction)
            .and_modify(|score| *score = (*score).max(priority.our_search_score()))
            .or_insert_with(|| priority.our_search_score());

        ranked.push((
            edge_index,
            direction,
            priority.opponent_search_score(),
            edge.child,
        ));
    }

    let mut ranked = ranked
        .into_iter()
        .map(|(edge_index, direction, opponent_score, child)| RankedEdge {
            edge_index,
            direction,
            direction_score: direction_scores.get(&direction).copied().unwrap_or(0),
            opponent_score,
            child,
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .direction_score
            .cmp(&left.direction_score)
            .then_with(|| right.opponent_score.cmp(&left.opponent_score))
            .then_with(|| left.direction.rank().cmp(&right.direction.rank()))
            .then_with(|| left.edge_index.cmp(&right.edge_index))
            .then_with(|| left.child.cmp(&right.child))
    });

    ranked.into_iter().map(|edge| edge.child).collect()
}

fn actor_net(node: &SearchNode, actor_id: &str) -> Option<i64> {
    node.active_analysis()?
        .actor_evaluations
        .get(actor_id)
        .map(|evaluation| evaluation.net)
}

fn forcing_score(parent: &SearchNode, edge: &SearchEdge) -> i64 {
    let _ = parent;
    let mut forcing = 0_i64;

    for event in &edge.events {
        let value = match event {
            InstantEvent::Died { .. } => FORCING_SCALE,
            InstantEvent::HeadToHeadLost { .. } => FORCING_SCALE,
            InstantEvent::SelfDeadEnd => FORCING_SCALE,
            InstantEvent::SelfConstrained { remaining_moves } => match remaining_moves {
                0 | 1 => 3_000,
                2 => 1_500,
                _ => 0,
            },
            InstantEvent::EnemyKilled {
                attribution: EliminationAttribution::OurSnake,
                ..
            }
            | InstantEvent::HeadToHeadWon { .. } => 3_000,
            InstantEvent::EnemyTrapped {
                caused_by_ours: true,
                ..
            } => 2_500,
            InstantEvent::EnemyForced {
                caused_by_ours: true,
                ..
            } => 1_500,
            _ => 0,
        };
        forcing = forcing.max(value);
    }

    forcing
}

#[cfg(test)]
mod tests {
    use crate::search::graph::FutureGraph;
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedGameState, SimulatedSnake,
    };
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
            food: vec![Coord { x: 2, y: 4 }],
            hazards: vec![],
            snakes: vec![
                snake("ours", &[(2, 2), (2, 1)]),
                snake("enemy", &[(4, 2), (4, 1)]),
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
    fn lethal_or_forcing_responses_receive_high_opponent_priority() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();
        let root = graph.node(graph.root());

        let lethal = root
            .children
            .iter()
            .filter_map(|edge| {
                let child = graph.node(edge.child);
                let priority = edge_priority(root, edge, child);
                (!child
                    .state
                    .snake("ours")
                    .is_some_and(|snake| snake.alive))
                .then_some(priority)
            })
            .max_by_key(|priority| priority.opponent_search_score())
            .expect("scenario must contain a lethal response");

        assert!(lethal.opponent_search_score() >= FORCING_SCALE);
    }

    #[test]
    fn ordering_is_deterministic_and_covers_every_generated_child() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let first = ordered_child_ids_for_search(&graph, graph.root());
        let second = ordered_child_ids_for_search(&graph, graph.root());

        assert_eq!(first, second);
        assert_eq!(first.len(), graph.node(graph.root()).children.len());
    }

    #[test]
    fn direction_opportunity_is_considered_before_response_order() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();
        let root = graph.node(graph.root());
        let ordered = ordered_child_ids_for_search(&graph, graph.root());

        let first_direction = root
            .children
            .iter()
            .find(|edge| edge.child == ordered[0])
            .and_then(|edge| edge.joint_action.direction_for("ours"))
            .expect("first child must have our direction");

        let best_direction = crate::direction::Direction::ALL
            .into_iter()
            .filter_map(|direction| {
                let score = root
                    .children
                    .iter()
                    .filter(|edge| edge.joint_action.direction_for("ours") == Some(direction))
                    .map(|edge| {
                        edge_priority(root, edge, graph.node(edge.child)).our_search_score()
                    })
                    .max()?;
                Some((direction, score))
            })
            .max_by_key(|(_, score)| *score)
            .map(|(direction, _)| direction)
            .unwrap();

        assert_eq!(first_direction, best_direction);
    }
}
