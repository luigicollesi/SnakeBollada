#![allow(dead_code)]

use crate::search::graph::{SearchEdge, SearchNode};
use crate::simulation::resolver::{EliminationAttribution, InstantEvent};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TransitionScore {
    pub(crate) instant_benefit: i64,
    pub(crate) instant_harm: i64,
    pub(crate) structural_delta: i64,
    pub(crate) net: i64,
}

impl TransitionScore {
    pub(crate) fn from_edge(parent: &SearchNode, edge: &SearchEdge, child: &SearchNode) -> Self {
        let actor_id = &parent.state.our_snake_id;
        let parent_eval = parent
            .active_analysis()
            .and_then(|analysis| analysis.actor_evaluations.get(actor_id));
        let child_eval = child
            .active_analysis()
            .and_then(|analysis| analysis.actor_evaluations.get(actor_id));

        let structural_delta = match (parent_eval, child_eval) {
            (Some(before), Some(after)) => after.net.saturating_sub(before.net),
            _ => 0,
        };

        let mut instant_benefit = 0_i64;
        let mut instant_harm = 0_i64;

        let weights = child_eval
            .or(parent_eval)
            .map(|evaluation| evaluation.weights);

        for event in &edge.events {
            match event {
                InstantEvent::AteFood { snake, .. } if snake == actor_id => {
                    if let Some(weights) = weights {
                        let food = weighted(1000, weights.food);
                        let growth = weighted(650, weights.growth);
                        instant_benefit = instant_benefit.saturating_add(food.saturating_add(growth));
                    }
                }
                InstantEvent::EnemyForced {
                    caused_by_ours: true,
                    ..
                } => {
                    if let Some(weights) = weights {
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(350, weights.pressure));
                    }
                }
                InstantEvent::EnemyTrapped {
                    caused_by_ours: true,
                    ..
                } => {
                    if let Some(weights) = weights {
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(700, weights.hunting))
                            .saturating_add(weighted(350, weights.pressure));
                    }
                }
                InstantEvent::EnemyKilled {
                    attribution: EliminationAttribution::OurSnake,
                    ..
                } => {
                    if let Some(weights) = weights {
                        let terminal_pressure = kill_importance(parent);
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(1000, weights.hunting))
                            .saturating_add(weighted(terminal_pressure, weights.dominance));
                    }
                }
                InstantEvent::HeadToHeadWon { .. } => {
                    if let Some(weights) = weights {
                        instant_benefit = instant_benefit
                            .saturating_add(weighted(1000, weights.hunting))
                            .saturating_add(weighted(700, weights.dominance));
                    }
                }
                InstantEvent::SelfConstrained { remaining_moves } => {
                    if let Some(weights) = weights {
                        let raw = match remaining_moves {
                            0 | 1 => 850,
                            2 => 400,
                            _ => 0,
                        };
                        instant_harm = instant_harm
                            .saturating_add(weighted(raw, weights.survival))
                            .saturating_add(weighted(raw / 2, weights.mobility));
                    }
                }
                InstantEvent::SelfDeadEnd => {
                    if let Some(weights) = weights {
                        instant_harm = instant_harm
                            .saturating_add(weighted(1000, weights.survival))
                            .saturating_add(weighted(700, weights.mobility));
                    }
                }
                InstantEvent::SelfBorderExposure { fear_milli, corner } => {
                    if let Some(weights) = weights {
                        let mut raw = i64::from(*fear_milli);
                        if *corner {
                            raw = raw.saturating_mul(135).saturating_div(100);
                        }
                        instant_harm =
                            instant_harm.saturating_add(weighted(raw.min(1000), weights.border));
                    }
                }
                InstantEvent::HeadToHeadLost { .. } | InstantEvent::Died { .. } => {
                    if let Some(weights) = weights {
                        instant_harm = instant_harm
                            .saturating_add(weighted(1000, weights.survival))
                            .saturating_add(weighted(1000, weights.mobility));
                    }
                }
                _ => {}
            }
        }

        let net = instant_benefit
            .saturating_sub(instant_harm)
            .saturating_add(structural_delta);

        Self {
            instant_benefit,
            instant_harm,
            structural_delta,
            net,
        }
    }
}

fn weighted(raw_milli: i64, weight_milli: u16) -> i64 {
    raw_milli
        .saturating_mul(i64::from(weight_milli))
        .saturating_div(1000)
}

fn kill_importance(parent: &SearchNode) -> i64 {
    let living_enemies = parent
        .state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != parent.state.our_snake_id)
        .count();

    match living_enemies {
        0 => 0,
        1 => 1000,
        2 => 800,
        3 => 650,
        _ => 500,
    }
}

#[cfg(test)]
mod tests {
    use crate::direction::Direction;
    use crate::search::graph::FutureGraph;
    use crate::simulation::resolver::InstantEvent;
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedGameState, SimulatedSnake,
    };
    use crate::Coord;

    use super::*;

    fn snake(id: &str, health: i32, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(health: i32, food: Vec<Coord>, head_x: i32) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food,
            hazards: vec![],
            snakes: vec![
                snake("ours", health, &[(head_x, 1), (head_x, 0)]),
                snake("enemy", 100, &[(5, 5), (5, 4)]),
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

    fn first_edge_for(
        graph: &FutureGraph,
        direction: Direction,
        predicate: impl Fn(&SearchEdge) -> bool,
    ) -> &SearchEdge {
        let root = graph.node(graph.root());
        root.children
            .iter()
            .find(|edge| {
                edge.joint_action.direction_for("ours") == Some(direction) && predicate(edge)
            })
            .expect("expected matching edge")
    }

    #[test]
    fn structural_delta_matches_actor_evaluation_difference() {
        let mut graph = FutureGraph::new(state(80, vec![], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |_| true);
        let child = graph.node(edge.child);
        let before = root
            .active_analysis()
            .unwrap()
            .actor_evaluations
            .get("ours")
            .unwrap()
            .net;
        let after = child
            .active_analysis()
            .unwrap()
            .actor_evaluations
            .get("ours")
            .unwrap()
            .net;

        let score = TransitionScore::from_edge(root, edge, child);

        assert_eq!(score.structural_delta, after - before);
    }

    #[test]
    fn food_is_an_instant_benefit_in_addition_to_state_delta() {
        let mut graph = FutureGraph::new(state(20, vec![Coord { x: 3, y: 1 }], 2));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Right, |edge| {
            edge.events.iter().any(|event| {
                matches!(
                    event,
                    InstantEvent::AteFood { snake, .. } if snake == "ours"
                )
            })
        });
        let child = graph.node(edge.child);
        let score = TransitionScore::from_edge(root, edge, child);

        assert!(score.instant_benefit > 0);
        assert!(score.net > score.structural_delta);
    }

    #[test]
    fn border_exposure_is_an_instant_harm() {
        let mut graph = FutureGraph::new(state(90, vec![], 1));
        graph.expand_to_depth(1).unwrap();

        let root = graph.node(graph.root());
        let edge = first_edge_for(&graph, Direction::Left, |edge| {
            edge.events
                .iter()
                .any(|event| matches!(event, InstantEvent::SelfBorderExposure { .. }))
        });
        let child = graph.node(edge.child);
        let score = TransitionScore::from_edge(root, edge, child);

        assert!(score.instant_harm > 0);
        assert!(score.net < score.structural_delta);
    }
}
