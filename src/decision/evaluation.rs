use std::collections::HashMap;

use crate::direction::Direction;
use crate::forecast::ForecastCertainty;
use crate::modes::survival::{SurvivalRouteAssessment, SurvivalStateSnapshot};
use crate::search::graph::{FutureGraph, NodeId};
use crate::simulation::resolver::{ForecastDelta, InstantEvent};

#[derive(Debug, Clone)]
pub(crate) struct RouteEvaluation {
    pub(crate) initial_move: Direction,
    pub(crate) events: Vec<InstantEvent>,
    pub(crate) survival: SurvivalRouteAssessment,
    pub(crate) died: bool,
    pub(crate) food_value: f32,
    pub(crate) hunting_value: f32,
    pub(crate) strategic_utility: f32,
    pub(crate) certainty: ForecastCertainty,
    pub(crate) final_aggression: f32,
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
}

pub(crate) fn evaluate_graph(graph: &FutureGraph, target_depth: u8) -> Vec<DirectionEvaluation> {
    let root = graph.node(graph.root());
    let root_snapshot = SurvivalStateSnapshot::from_tactical(&root.state, &root.tactical);

    let context = RouteContext {
        initial_move: None,
        events: Vec::new(),
        snapshots: vec![root_snapshot],
        certainty: ForecastCertainty::Deterministic,
    };

    let mut routes = Vec::new();
    walk_routes(graph, graph.root(), 0, target_depth, &context, &mut routes);

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
        .collect()
}

#[derive(Debug, Clone)]
struct RouteContext {
    initial_move: Option<Direction>,
    events: Vec<InstantEvent>,
    snapshots: Vec<SurvivalStateSnapshot>,
    certainty: ForecastCertainty,
}

fn walk_routes(
    graph: &FutureGraph,
    node_id: NodeId,
    depth: u8,
    target_depth: u8,
    context: &RouteContext,
    routes: &mut Vec<RouteEvaluation>,
) {
    let node = graph.node(node_id);

    if depth >= target_depth || node.children.is_empty() {
        if let Some(initial_move) = context.initial_move {
            if let Some(survival) = SurvivalRouteAssessment::from_snapshots(
                &context.snapshots,
                second_order_mobility(&context.snapshots),
            ) {
                routes.push(finalize_route(
                    initial_move,
                    &context.events,
                    survival,
                    context.certainty,
                    node.state.aggression.value,
                    &node.state.our_snake_id,
                ));
            }
        }
        return;
    }

    for edge in &node.children {
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
            &child.tactical,
        ));

        let certainty = match (context.certainty, edge.forecast_delta) {
            (_, ForecastDelta::FoodUncertainty) | (ForecastCertainty::FoodProvisional, _) => {
                ForecastCertainty::FoodProvisional
            }
            _ => ForecastCertainty::Deterministic,
        };

        walk_routes(
            graph,
            edge.child,
            depth.saturating_add(1),
            target_depth,
            &RouteContext {
                initial_move: Some(initial_move),
                events,
                snapshots,
                certainty,
            },
            routes,
        );
    }
}

fn finalize_route(
    initial_move: Direction,
    events: &[InstantEvent],
    survival: SurvivalRouteAssessment,
    certainty: ForecastCertainty,
    final_aggression: f32,
    our_snake_id: &str,
) -> RouteEvaluation {
    let food_value = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                InstantEvent::AteFood { snake, .. } if snake == our_snake_id
            )
        })
        .count() as f32;

    let hunting_value = hunting_value(events);
    let (food_discount, hunting_discount) = match certainty {
        ForecastCertainty::Deterministic => (1.0, 1.0),
        ForecastCertainty::FoodProvisional => (0.50, 0.80),
    };

    let aggression = final_aggression.clamp(0.0, 1.0);
    let strategic_utility = food_value * food_discount * (1.0 - aggression)
        + hunting_value * hunting_discount * aggression;

    RouteEvaluation {
        initial_move,
        events: events.to_vec(),
        survival,
        died: survival.died,
        food_value,
        hunting_value,
        strategic_utility,
        certainty,
        final_aggression: aggression,
    }
}

fn hunting_value(events: &[InstantEvent]) -> f32 {
    let mut per_enemy: HashMap<&str, f32> = HashMap::new();

    for event in events {
        let (enemy, value) = match event {
            InstantEvent::EnemyForced { enemy, .. } => (enemy.as_str(), 0.25),
            InstantEvent::EnemyTrapped { enemy } => (enemy.as_str(), 0.60),
            InstantEvent::EnemyKilled { enemy, .. } => (enemy.as_str(), 1.0),
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

fn second_order_mobility(snapshots: &[SurvivalStateSnapshot]) -> u32 {
    snapshots
        .iter()
        .skip(1)
        .take(2)
        .map(|snapshot| u32::from(snapshot.safe_moves))
        .sum()
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
        let average_strategic_utility = if routes.is_empty() {
            0.0
        } else {
            routes
                .iter()
                .map(|route| route.strategic_utility)
                .sum::<f32>()
                / routes.len() as f32
        };

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
    fn kill_supersedes_forced_and_trapped_reward_for_same_enemy() {
        let events = vec![
            InstantEvent::EnemyForced {
                enemy: "enemy".to_string(),
                remaining_moves: 1,
            },
            InstantEvent::EnemyTrapped {
                enemy: "enemy".to_string(),
            },
            InstantEvent::EnemyKilled {
                enemy: "enemy".to_string(),
                cause: crate::simulation::resolver::EliminationCause::HeadToHead,
            },
        ];

        assert!((hunting_value(&events) - 1.0).abs() < f32::EPSILON);
    }
}
