#![allow(dead_code)]

use std::collections::HashMap;

use crate::direction::Direction;
use crate::forecast::ForecastCertainty;
use crate::modes::survival::{SurvivalRouteAssessment, SurvivalStateSnapshot};
use crate::search::budget::SearchBudget;
use crate::search::graph::{FutureGraph, NodeId, SearchNode};
use crate::simulation::resolver::{EliminationAttribution, ForecastDelta, InstantEvent};

#[derive(Debug, Clone)]
pub(crate) struct RouteEvaluation {
    pub(crate) initial_move: Direction,
    pub(crate) events: Vec<InstantEvent>,
    pub(crate) survival: SurvivalRouteAssessment,
    pub(crate) died: bool,
    pub(crate) food_value: f32,
    pub(crate) hunting_value: f32,
    pub(crate) leaf_food_potential: f32,
    pub(crate) leaf_hunting_potential: f32,
    pub(crate) strategic_utility: f32,
    pub(crate) certainty: ForecastCertainty,
    pub(crate) final_aggression: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct LeafPotential {
    food: f32,
    hunting: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectionSurvivalSummary {
    pub(crate) total_routes: u32,
    pub(crate) death_routes: u32,
    pub(crate) dead_end_routes: u32,
    pub(crate) forced_routes: u32,
    pub(crate) constrained_routes: u32,
    pub(crate) min_future_mobility: u8,
    pub(crate) min_reachable_space: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectionEvaluation {
    pub(crate) direction: Direction,
    pub(crate) routes: Vec<RouteEvaluation>,
    pub(crate) survival: DirectionSurvivalSummary,
    pub(crate) worst_strategic_utility: f32,
    pub(crate) average_strategic_utility: f32,
    pub(crate) average_food_value: f32,
    pub(crate) average_hunting_value: f32,
    pub(crate) average_leaf_food_potential: f32,
    pub(crate) average_leaf_hunting_potential: f32,
    pub(crate) reserved_override: bool,
}

pub(crate) fn evaluate_graph(graph: &FutureGraph, target_depth: u8) -> Vec<DirectionEvaluation> {
    evaluate_graph_inner(graph, target_depth, None).unwrap_or_default()
}

pub(crate) fn evaluate_graph_budgeted(
    graph: &FutureGraph,
    target_depth: u8,
    budget: &SearchBudget,
) -> Option<Vec<DirectionEvaluation>> {
    evaluate_graph_inner(graph, target_depth, Some(budget))
}

fn evaluate_graph_inner(
    graph: &FutureGraph,
    target_depth: u8,
    budget: Option<&SearchBudget>,
) -> Option<Vec<DirectionEvaluation>> {
    let root = graph.node(graph.root());
    let root_snapshot = SurvivalStateSnapshot::from_tactical(&root.state, &root.analysis.tactical);

    let context = RouteContext {
        initial_move: None,
        events: Vec::new(),
        snapshots: vec![root_snapshot],
        second_order_mobility: node_second_order_mobility(graph, graph.root()),
        certainty: ForecastCertainty::Deterministic,
    };

    let mut routes = Vec::new();
    if !walk_routes(
        graph,
        graph.root(),
        0,
        target_depth,
        &context,
        &mut routes,
        budget,
    ) {
        return None;
    }

    Some(
        Direction::ALL
            .into_iter()
            .filter_map(|direction| {
                let direction_routes = routes
                    .iter()
                    .filter(|route| route.initial_move == direction)
                    .cloned()
                    .collect::<Vec<_>>();

                (!direction_routes.is_empty())
                    .then(|| DirectionEvaluation::from_routes(direction, direction_routes))
            })
            .collect(),
    )
}

#[derive(Debug, Clone)]
struct RouteContext {
    initial_move: Option<Direction>,
    events: Vec<InstantEvent>,
    snapshots: Vec<SurvivalStateSnapshot>,
    second_order_mobility: u32,
    certainty: ForecastCertainty,
}

fn walk_routes(
    graph: &FutureGraph,
    node_id: NodeId,
    depth: u8,
    target_depth: u8,
    context: &RouteContext,
    routes: &mut Vec<RouteEvaluation>,
    budget: Option<&SearchBudget>,
) -> bool {
    if budget.is_some_and(SearchBudget::expired) {
        return false;
    }

    let node = graph.node(node_id);

    if depth >= target_depth || node.children.is_empty() {
        if let Some(initial_move) = context.initial_move {
            if let Some(survival) = SurvivalRouteAssessment::from_snapshots(
                &context.snapshots,
                context.second_order_mobility,
            ) {
                routes.push(finalize_route(
                    initial_move,
                    &context.events,
                    survival,
                    context.certainty,
                    node,
                ));
            }
        }
        return true;
    }

    for edge in &node.children {
        if budget.is_some_and(SearchBudget::expired) {
            return false;
        }
        let child = graph.node(edge.child);
        let initial_move = context
            .initial_move
            .or_else(|| edge.joint_action.direction_for(&node.state.our_snake_id));

        let Some(initial_move) = initial_move else {
            continue;
        };

        let mut events = context.events.clone();
        events.extend(edge.events.iter().cloned());

        let mut snapshots = context.snapshots.clone();
        snapshots.push(SurvivalStateSnapshot::from_tactical(
            &child.state,
            &child.analysis.tactical,
        ));

        let second_order_mobility = context
            .second_order_mobility
            .min(node_second_order_mobility(graph, edge.child));

        let certainty = match (context.certainty, edge.forecast_delta) {
            (_, ForecastDelta::FoodUncertainty) | (ForecastCertainty::FoodProvisional, _) => {
                ForecastCertainty::FoodProvisional
            }
            _ => ForecastCertainty::Deterministic,
        };

        if !walk_routes(
            graph,
            edge.child,
            depth.saturating_add(1),
            target_depth,
            &RouteContext {
                initial_move: Some(initial_move),
                events,
                snapshots,
                second_order_mobility,
                certainty,
            },
            routes,
            budget,
        ) {
            return false;
        }
    }

    true
}

fn finalize_route(
    initial_move: Direction,
    events: &[InstantEvent],
    survival: SurvivalRouteAssessment,
    certainty: ForecastCertainty,
    leaf: &SearchNode,
) -> RouteEvaluation {
    let realized_food = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                InstantEvent::AteFood { snake, .. } if snake == &leaf.state.our_snake_id
            )
        })
        .count() as f32;
    let realized_hunting = hunting_value(events);
    let potential = leaf_potential(leaf);

    let food_value = realized_food + potential.food;
    let hunting_value = realized_hunting + potential.hunting;
    let (food_discount, hunting_discount) = match certainty {
        ForecastCertainty::Deterministic => (1.0, 1.0),
        ForecastCertainty::FoodProvisional => (0.50, 0.80),
    };

    let aggression = leaf.state.aggression.value.clamp(0.0, 1.0);
    let strategic_utility = food_value * food_discount * (1.0 - aggression)
        + hunting_value * hunting_discount * aggression;

    RouteEvaluation {
        initial_move,
        events: events.to_vec(),
        survival,
        died: survival.died,
        food_value,
        hunting_value,
        leaf_food_potential: potential.food,
        leaf_hunting_potential: potential.hunting,
        strategic_utility,
        certainty,
        final_aggression: aggression,
    }
}

fn leaf_potential(node: &SearchNode) -> LeafPotential {
    if !node
        .state
        .snake(&node.state.our_snake_id)
        .is_some_and(|snake| snake.alive)
    {
        return LeafPotential::default();
    }

    LeafPotential {
        food: food_leaf_potential(node),
        hunting: hunting_leaf_potential(node),
    }
}

fn food_leaf_potential(node: &SearchNode) -> f32 {
    let Some(ours) = node.state.snake(&node.state.our_snake_id) else {
        return 0.0;
    };

    let health_pressure = if ours.health <= 20 {
        1.5
    } else if ours.health <= 40 {
        1.25
    } else {
        1.0
    };

    node.state
        .food
        .iter()
        .filter_map(|food| {
            let route = node
                .analysis
                .state
                .route_for(&node.state.our_snake_id, *food)?;
            let distance = route.distance?;
            let claim_factor = node
                .analysis
                .state
                .nearest_competitor_for(&node.state.our_snake_id, *food)
                .map(|competitor| {
                    if distance < competitor.eta {
                        1.0
                    } else if distance == competitor.eta {
                        0.5
                    } else {
                        0.15
                    }
                })
                .unwrap_or(1.0);

            Some(0.5 * health_pressure * claim_factor / f32::from(distance.saturating_add(1)))
        })
        .reduce(f32::max)
        .unwrap_or(0.0)
        .min(0.5)
}

fn hunting_leaf_potential(node: &SearchNode) -> f32 {
    let Some(ours) = node.state.snake(&node.state.our_snake_id) else {
        return 0.0;
    };

    node.analysis
        .tactical
        .enemies
        .values()
        .filter_map(|enemy| {
            let snake = node.state.snake(&enemy.snake_id)?;
            let length_advantage = ours.length() as i32 - snake.length() as i32;
            if length_advantage < 0 {
                return Some(0.0);
            }

            let plausible = f32::from(enemy.plausible_moves.len().max(1));
            let mobility_pressure = (4.0 - plausible).max(0.0) / 4.0;
            let space_threshold = (snake.length() as u32).saturating_mul(2).max(1);
            let space_pressure = if enemy.best_reachable_space < space_threshold {
                1.0 - enemy.best_reachable_space as f32 / space_threshold as f32
            } else {
                0.0
            };
            let length_factor = if length_advantage > 0 { 1.0 } else { 0.5 };

            Some((0.35 * mobility_pressure + 0.15 * space_pressure) * length_factor)
        })
        .reduce(f32::max)
        .unwrap_or(0.0)
        .min(0.5)
}

fn hunting_value(events: &[InstantEvent]) -> f32 {
    let mut per_enemy: HashMap<&str, f32> = HashMap::new();

    for event in events {
        let (enemy, value) = match event {
            InstantEvent::EnemyForced {
                enemy,
                caused_by_ours: true,
                ..
            } => (enemy.as_str(), 0.25),
            InstantEvent::EnemyTrapped {
                enemy,
                caused_by_ours: true,
            } => (enemy.as_str(), 0.60),
            InstantEvent::EnemyForced { .. } | InstantEvent::EnemyTrapped { .. } => continue,
            InstantEvent::EnemyKilled {
                enemy,
                attribution: EliminationAttribution::OurSnake,
                ..
            } => (enemy.as_str(), 1.0),
            InstantEvent::EnemyKilled { .. } => continue,
            InstantEvent::HeadToHeadWon { enemy } => (enemy.as_str(), 1.0),
            _ => continue,
        };

        per_enemy
            .entry(enemy)
            .and_modify(|current| *current = current.max(value))
            .or_insert(value);
    }

    per_enemy.values().sum()
}

fn node_second_order_mobility(graph: &FutureGraph, node_id: NodeId) -> u32 {
    let node = graph.node(node_id);
    if node.children.is_empty() {
        return u32::from(node.analysis.tactical.ours.safe_moves.len());
    }

    node.children
        .iter()
        .map(|edge| {
            u32::from(
                graph
                    .node(edge.child)
                    .analysis
                    .tactical
                    .ours
                    .safe_moves
                    .len(),
            )
        })
        .min()
        .unwrap_or(0)
}

fn confirms_reserved_override(routes: &[RouteEvaluation]) -> bool {
    let mut surviving_routes = routes.iter().filter(|route| !route.died).peekable();
    if surviving_routes.peek().is_none() {
        return false;
    }

    surviving_routes.all(|route| {
        route.events.iter().any(|event| {
            matches!(
                event,
                InstantEvent::EnemyTrapped {
                    caused_by_ours: true,
                    ..
                } | InstantEvent::EnemyKilled {
                    attribution: EliminationAttribution::OurSnake,
                    ..
                } | InstantEvent::HeadToHeadWon { .. }
            )
        })
    })
}

impl DirectionEvaluation {
    fn from_routes(direction: Direction, routes: Vec<RouteEvaluation>) -> Self {
        let total_routes = routes.len() as u32;
        let death_routes = routes.iter().filter(|route| route.died).count() as u32;
        let dead_end_routes = routes
            .iter()
            .filter(|route| route.survival.dead_end)
            .count() as u32;
        let forced_routes = routes
            .iter()
            .filter(|route| {
                route.events.iter().any(|event| {
                    matches!(event, InstantEvent::SelfConstrained { remaining_moves: 1 })
                })
            })
            .count() as u32;
        let constrained_routes = routes
            .iter()
            .filter(|route| {
                route.events.iter().any(|event| {
                    matches!(event, InstantEvent::SelfConstrained { remaining_moves: 2 })
                })
            })
            .count() as u32;

        let min_future_mobility = routes
            .iter()
            .map(|route| route.survival.min_safe_moves)
            .min()
            .unwrap_or(0);
        let min_reachable_space = routes
            .iter()
            .map(|route| route.survival.min_reachable_space)
            .min()
            .unwrap_or(0);
        let worst_strategic_utility = routes
            .iter()
            .map(|route| route.strategic_utility)
            .reduce(f32::min)
            .unwrap_or(0.0);
        let reserved_override = confirms_reserved_override(&routes);
        let route_count = routes.len().max(1) as f32;
        let average_strategic_utility =
            routes.iter().map(|route| route.strategic_utility).sum::<f32>() / route_count;
        let average_food_value =
            routes.iter().map(|route| route.food_value).sum::<f32>() / route_count;
        let average_hunting_value =
            routes.iter().map(|route| route.hunting_value).sum::<f32>() / route_count;
        let average_leaf_food_potential = routes
            .iter()
            .map(|route| route.leaf_food_potential)
            .sum::<f32>()
            / route_count;
        let average_leaf_hunting_potential = routes
            .iter()
            .map(|route| route.leaf_hunting_potential)
            .sum::<f32>()
            / route_count;

        Self {
            direction,
            routes,
            survival: DirectionSurvivalSummary {
                total_routes,
                death_routes,
                dead_end_routes,
                forced_routes,
                constrained_routes,
                min_future_mobility,
                min_reachable_space,
            },
            worst_strategic_utility,
            average_strategic_utility,
            average_food_value,
            average_hunting_value,
            average_leaf_food_potential,
            average_leaf_hunting_potential,
            reserved_override,
        }
    }
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

    fn state(food: Vec<Coord>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food,
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
    fn graph_routes_are_grouped_by_first_direction() {
        let mut graph = FutureGraph::new(state(vec![]));
        graph.expand_to_depth(1).unwrap();

        let directions = evaluate_graph(&graph, 1);

        assert!(!directions.is_empty());
        assert!(directions
            .iter()
            .all(|evaluation| evaluation.survival.total_routes > 0));
    }

    #[test]
    fn food_reward_is_branch_local() {
        let mut graph = FutureGraph::new(state(vec![Coord { x: 2, y: 1 }]));
        graph.expand_to_depth(1).unwrap();

        let directions = evaluate_graph(&graph, 1);
        let right = directions
            .iter()
            .find(|evaluation| evaluation.direction == Direction::Right)
            .unwrap();
        let up = directions
            .iter()
            .find(|evaluation| evaluation.direction == Direction::Up)
            .unwrap();

        assert!(right.routes.iter().any(|route| route.food_value > 0.0));
        assert!(up.routes.iter().all(|route| route.food_value == 0.0));
    }

    #[test]
    fn food_beyond_horizon_receives_leaf_potential_without_fake_event() {
        let graph = FutureGraph::new(state(vec![Coord { x: 4, y: 1 }]));
        let node = graph.node(graph.root());

        let potential = food_leaf_potential(node);

        assert!(potential > 0.0);
    }

    #[test]
    fn hunting_pressure_can_exist_without_realized_hunting_event() {
        let graph = FutureGraph::new(state(vec![]));
        let node = graph.node(graph.root());

        let potential = hunting_leaf_potential(node);

        assert!(potential >= 0.0);
    }

    #[test]
    fn reserved_override_requires_confirmed_tactical_result_on_every_surviving_route() {
        let survival = SurvivalRouteAssessment {
            died: false,
            dead_end: false,
            final_safe_moves: 2,
            min_safe_moves: 2,
            final_reachable_space: 10,
            min_reachable_space: 10,
            second_order_mobility: 4,
        };
        let tactical = RouteEvaluation {
            initial_move: Direction::Left,
            events: vec![InstantEvent::EnemyTrapped {
                enemy: "enemy".to_string(),
                caused_by_ours: true,
            }],
            survival,
            died: false,
            food_value: 0.0,
            hunting_value: 0.6,
            leaf_food_potential: 0.0,
            leaf_hunting_potential: 0.0,
            strategic_utility: 0.1,
            certainty: ForecastCertainty::Deterministic,
            final_aggression: 0.2,
        };
        let food_only = RouteEvaluation {
            events: vec![InstantEvent::AteFood {
                snake: "ours".to_string(),
                food: Coord { x: 0, y: 0 },
            }],
            ..tactical.clone()
        };

        assert!(confirms_reserved_override(std::slice::from_ref(&tactical)));
        assert!(!confirms_reserved_override(&[tactical, food_only]));
    }

    #[test]
    fn noncausal_forcing_does_not_receive_hunting_credit() {
        let events = vec![InstantEvent::EnemyForced {
            enemy: "enemy".to_string(),
            remaining_moves: 1,
            caused_by_ours: false,
        }];

        assert_eq!(hunting_value(&events), 0.0);
    }

    #[test]
    fn environment_kill_does_not_receive_hunting_credit() {
        let events = vec![InstantEvent::EnemyKilled {
            enemy: "enemy".to_string(),
            cause: crate::simulation::resolver::EliminationCause::Hazard,
            attribution: EliminationAttribution::Environment,
        }];

        assert_eq!(hunting_value(&events), 0.0);
    }

    #[test]
    fn kill_supersedes_forced_and_trapped_reward_for_same_enemy() {
        let events = vec![
            InstantEvent::EnemyForced {
                enemy: "enemy".to_string(),
                remaining_moves: 1,
                caused_by_ours: true,
            },
            InstantEvent::EnemyTrapped {
                enemy: "enemy".to_string(),
                caused_by_ours: true,
            },
            InstantEvent::EnemyKilled {
                enemy: "enemy".to_string(),
                cause: crate::simulation::resolver::EliminationCause::HeadToHead,
                attribution: EliminationAttribution::OurSnake,
            },
        ];

        assert!((hunting_value(&events) - 1.0).abs() < f32::EPSILON);
    }
}
