#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use crate::decision::policy::ReservedCellPolicy;
use crate::direction::{Direction, MoveMask};
use crate::forecast::ForecastCertainty;
use crate::search::budget::SearchBudget;
use crate::search::graph::{FutureGraph, NodeId, SearchEdge, SearchNode};
use crate::simulation::resolver::{EliminationAttribution, ForecastDelta, InstantEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalAssessment {
    Lost,
    Running,
    Won,
}

impl TerminalAssessment {
    const fn rank(self) -> u8 {
        match self {
            Self::Lost => 0,
            Self::Running => 1,
            Self::Won => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirectionSurvivalSummary {
    pub(crate) total_routes: u64,
    pub(crate) death_routes: u64,
    pub(crate) dead_end_routes: u64,
    pub(crate) forced_routes: u64,
    pub(crate) constrained_routes: u64,
    pub(crate) min_future_mobility: u8,
    pub(crate) min_reachable_space: u32,
    pub(crate) min_second_order_mobility: u32,
}

impl DirectionSurvivalSummary {
    fn leaf(node: &SearchNode, terminal: TerminalAssessment) -> Self {
        let safe_moves = node
            .active_analysis()
            .map_or(0, |analysis| analysis.tactical.ours.safe_moves.len());
        let reachable_space = node
            .active_analysis()
            .map_or(0, |analysis| analysis.tactical.ours.best_reachable_space);

        Self {
            total_routes: 1,
            death_routes: u64::from(terminal == TerminalAssessment::Lost),
            dead_end_routes: u64::from(terminal != TerminalAssessment::Lost && safe_moves == 0),
            forced_routes: u64::from(terminal != TerminalAssessment::Lost && safe_moves == 1),
            constrained_routes: u64::from(terminal != TerminalAssessment::Lost && safe_moves == 2),
            min_future_mobility: safe_moves,
            min_reachable_space: reachable_space,
            min_second_order_mobility: u32::from(safe_moves),
        }
    }

    fn with_parent_snapshot(mut self, parent: &SearchNode) -> Self {
        let analysis = parent
            .active_analysis()
            .expect("running parent must have analysis");
        let safe_moves = analysis.tactical.ours.safe_moves.len();
        self.min_future_mobility = self.min_future_mobility.min(safe_moves);
        self.min_reachable_space = self
            .min_reachable_space
            .min(analysis.tactical.ours.best_reachable_space);
        self
    }

    pub(crate) fn has_death_response(&self) -> bool {
        self.death_routes > 0
    }

    pub(crate) fn has_dead_end_response(&self) -> bool {
        self.dead_end_routes > 0
    }

    pub(crate) fn has_forced_response(&self) -> bool {
        self.forced_routes > 0
    }

    pub(crate) fn has_constrained_response(&self) -> bool {
        self.constrained_routes > 0
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StrategicEnvelope {
    worst_utility: f32,
    average_utility: f32,
    average_realized_food: f32,
    average_realized_hunting: f32,
    average_leaf_food_potential: f32,
    average_leaf_hunting_potential: f32,
}

#[derive(Debug, Clone)]
struct NodeEvaluation {
    terminal: TerminalAssessment,
    survival: DirectionSurvivalSummary,
    strategic: StrategicEnvelope,
    guaranteed_enemy_kills: u16,
    reserved_override_all: bool,
    chosen_move: Option<Direction>,
}

#[derive(Debug, Clone)]
struct EdgeOutcome {
    terminal: TerminalAssessment,
    survival: DirectionSurvivalSummary,
    strategic: StrategicEnvelope,
    guaranteed_enemy_kills: u16,
    reserved_override_all: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DirectionEvaluation {
    pub(crate) direction: Direction,
    pub(crate) terminal: TerminalAssessment,
    pub(crate) survival: DirectionSurvivalSummary,
    pub(crate) worst_strategic_utility: f32,
    pub(crate) average_strategic_utility: f32,
    pub(crate) average_food_value: f32,
    pub(crate) average_hunting_value: f32,
    pub(crate) average_leaf_food_potential: f32,
    pub(crate) average_leaf_hunting_potential: f32,
    pub(crate) guaranteed_enemy_kills: u16,
    pub(crate) reserved_override: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DagEvaluationStats {
    pub(crate) nodes_evaluated: u32,
    pub(crate) memo_hits: u32,
    pub(crate) deterministic_evaluations: u32,
    pub(crate) provisional_evaluations: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct DagEvaluationResult {
    pub(crate) directions: Vec<DirectionEvaluation>,
    pub(crate) stats: DagEvaluationStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    node: NodeId,
    remaining_depth: u8,
    provisional: bool,
}

struct DagEvaluator<'a> {
    graph: &'a FutureGraph,
    budget: Option<&'a SearchBudget>,
    memo: HashMap<MemoKey, NodeEvaluation>,
    stats: DagEvaluationStats,
    policy: ReservedCellPolicy,
}

pub(crate) fn evaluate_graph(graph: &FutureGraph, target_depth: u8) -> Vec<DirectionEvaluation> {
    evaluate_graph_inner(graph, target_depth, None)
        .map(|result| result.directions)
        .unwrap_or_default()
}

pub(crate) fn evaluate_graph_budgeted(
    graph: &FutureGraph,
    target_depth: u8,
    budget: &SearchBudget,
) -> Option<DagEvaluationResult> {
    evaluate_graph_inner(graph, target_depth, Some(budget))
}

fn evaluate_graph_inner(
    graph: &FutureGraph,
    target_depth: u8,
    budget: Option<&SearchBudget>,
) -> Option<DagEvaluationResult> {
    let mut evaluator = DagEvaluator {
        graph,
        budget,
        memo: HashMap::new(),
        stats: DagEvaluationStats::default(),
        policy: ReservedCellPolicy::default(),
    };
    let directions = evaluator.evaluate_directions(
        graph.root(),
        target_depth,
        ForecastCertainty::Deterministic,
    )?;

    Some(DagEvaluationResult {
        directions,
        stats: evaluator.stats,
    })
}

impl DagEvaluator<'_> {
    fn evaluate_node(
        &mut self,
        node_id: NodeId,
        remaining_depth: u8,
        certainty: ForecastCertainty,
    ) -> Option<NodeEvaluation> {
        if self.budget.is_some_and(SearchBudget::expired) {
            return None;
        }

        let key = MemoKey {
            node: node_id,
            remaining_depth,
            provisional: certainty == ForecastCertainty::FoodProvisional,
        };
        if let Some(cached) = self.memo.get(&key) {
            self.stats.memo_hits = self.stats.memo_hits.saturating_add(1);
            return Some(cached.clone());
        }

        self.stats.nodes_evaluated = self.stats.nodes_evaluated.saturating_add(1);
        match certainty {
            ForecastCertainty::Deterministic => {
                self.stats.deterministic_evaluations =
                    self.stats.deterministic_evaluations.saturating_add(1);
            }
            ForecastCertainty::FoodProvisional => {
                self.stats.provisional_evaluations =
                    self.stats.provisional_evaluations.saturating_add(1);
            }
        }

        let node = self.graph.node(node_id);
        let terminal = terminal_assessment(node);
        let evaluation = if remaining_depth == 0
            || terminal != TerminalAssessment::Running
            || node.children.is_empty()
        {
            evaluate_frontier(node, certainty, terminal)
        } else {
            let directions = self.evaluate_directions(node_id, remaining_depth, certainty)?;
            let best = choose_best_direction(
                &directions,
                &node.state,
                node.active_analysis()
                    .expect("running search node must have analysis")
                    .tactical
                    .ours
                    .safe_moves,
                self.policy,
            )?;

            NodeEvaluation {
                terminal: best.terminal,
                survival: best.survival.with_parent_snapshot(node),
                strategic: StrategicEnvelope {
                    worst_utility: best.worst_strategic_utility,
                    average_utility: best.average_strategic_utility,
                    average_realized_food: best.average_food_value,
                    average_realized_hunting: best.average_hunting_value,
                    average_leaf_food_potential: best.average_leaf_food_potential,
                    average_leaf_hunting_potential: best.average_leaf_hunting_potential,
                },
                guaranteed_enemy_kills: best.guaranteed_enemy_kills,
                reserved_override_all: best.reserved_override,
                chosen_move: Some(best.direction),
            }
        };

        self.memo.insert(key, evaluation.clone());
        Some(evaluation)
    }

    fn evaluate_directions(
        &mut self,
        node_id: NodeId,
        remaining_depth: u8,
        certainty: ForecastCertainty,
    ) -> Option<Vec<DirectionEvaluation>> {
        if self.budget.is_some_and(SearchBudget::expired) {
            return None;
        }

        let node = self.graph.node(node_id);
        if remaining_depth == 0 || node.children.is_empty() {
            return Some(Vec::new());
        }

        let mut grouped: [Vec<&SearchEdge>; 4] = std::array::from_fn(|_| Vec::new());
        for edge in &node.children {
            let Some(direction) = edge.joint_action.direction_for(&node.state.our_snake_id) else {
                continue;
            };
            grouped[usize::from(direction.rank())].push(edge);
        }

        let mut directions = Vec::new();
        for direction in Direction::ALL {
            let edges = &grouped[usize::from(direction.rank())];
            if edges.is_empty() {
                continue;
            }

            let mut outcomes = Vec::with_capacity(edges.len());
            for edge in edges {
                if self.budget.is_some_and(SearchBudget::expired) {
                    return None;
                }

                let child_certainty = next_certainty(certainty, edge.forecast_delta);
                let child = self.evaluate_node(
                    edge.child,
                    remaining_depth.saturating_sub(1),
                    child_certainty,
                )?;
                outcomes.push(apply_edge(node, edge, child, certainty, self.graph));
            }

            directions.push(aggregate_direction(direction, node, &outcomes));
        }

        Some(directions)
    }
}

fn terminal_assessment(node: &SearchNode) -> TerminalAssessment {
    let ours_alive = node
        .state
        .snake(&node.state.our_snake_id)
        .is_some_and(|snake| snake.alive);
    if !ours_alive {
        return TerminalAssessment::Lost;
    }

    let enemy_alive = node
        .state
        .snakes
        .iter()
        .any(|snake| snake.alive && snake.id != node.state.our_snake_id);

    if enemy_alive {
        TerminalAssessment::Running
    } else {
        TerminalAssessment::Won
    }
}

fn evaluate_frontier(
    node: &SearchNode,
    certainty: ForecastCertainty,
    terminal: TerminalAssessment,
) -> NodeEvaluation {
    let potential = if terminal == TerminalAssessment::Running {
        leaf_potential(node)
    } else {
        LeafPotential::default()
    };
    let aggression = node.state.aggression.value.clamp(0.0, 1.0);
    let (food_discount, hunting_discount) = certainty_discounts(certainty);
    let utility = potential.food * food_discount * (1.0 - aggression)
        + potential.hunting * hunting_discount * aggression;

    NodeEvaluation {
        terminal,
        survival: DirectionSurvivalSummary::leaf(node, terminal),
        strategic: StrategicEnvelope {
            worst_utility: utility,
            average_utility: utility,
            average_realized_food: 0.0,
            average_realized_hunting: 0.0,
            average_leaf_food_potential: potential.food,
            average_leaf_hunting_potential: potential.hunting,
        },
        guaranteed_enemy_kills: 0,
        reserved_override_all: false,
        chosen_move: None,
    }
}

fn apply_edge(
    parent: &SearchNode,
    edge: &SearchEdge,
    child: NodeEvaluation,
    certainty: ForecastCertainty,
    graph: &FutureGraph,
) -> EdgeOutcome {
    let child_node = graph.node(edge.child);
    let route_count = child.survival.total_routes.max(1);
    let realized_food = edge
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                InstantEvent::AteFood { snake, .. } if snake == &parent.state.our_snake_id
            )
        })
        .count() as f32;
    let realized_hunting = edge_hunting_delta(parent, edge);
    let guaranteed_enemy_kills = child
        .guaranteed_enemy_kills
        .saturating_add(edge_enemy_kills(edge));

    let (food_discount, hunting_discount) = certainty_discounts(certainty);
    let aggression = child_node.state.aggression.value.clamp(0.0, 1.0);
    let local_utility = realized_food * food_discount * (1.0 - aggression)
        + realized_hunting * hunting_discount * aggression;

    let death_now = edge
        .events
        .iter()
        .any(|event| matches!(event, InstantEvent::Died { .. }));
    let dead_end_now = edge
        .events
        .iter()
        .any(|event| matches!(event, InstantEvent::SelfDeadEnd));
    let forced_now = edge
        .events
        .iter()
        .any(|event| matches!(event, InstantEvent::SelfConstrained { remaining_moves: 1 }));
    let constrained_now = edge
        .events
        .iter()
        .any(|event| matches!(event, InstantEvent::SelfConstrained { remaining_moves: 2 }));

    let child_safe_moves = child_node
        .active_analysis()
        .map_or(0, |analysis| analysis.tactical.ours.safe_moves.len());
    let mut survival = child.survival;
    if death_now {
        survival.death_routes = route_count;
    }
    if dead_end_now {
        survival.dead_end_routes = route_count;
    }
    if forced_now {
        survival.forced_routes = route_count;
    }
    if constrained_now {
        survival.constrained_routes = route_count;
    }
    survival.min_future_mobility = survival.min_future_mobility.min(child_safe_moves);
    survival.min_reachable_space = survival.min_reachable_space.min(
        child_node
            .active_analysis()
            .map_or(0, |analysis| analysis.tactical.ours.best_reachable_space),
    );
    survival.min_second_order_mobility = survival
        .min_second_order_mobility
        .min(u32::from(child_safe_moves));

    EdgeOutcome {
        terminal: if death_now {
            TerminalAssessment::Lost
        } else {
            child.terminal
        },
        survival,
        strategic: StrategicEnvelope {
            worst_utility: child.strategic.worst_utility + local_utility,
            average_utility: child.strategic.average_utility + local_utility,
            average_realized_food: child.strategic.average_realized_food + realized_food,
            average_realized_hunting: child.strategic.average_realized_hunting + realized_hunting,
            average_leaf_food_potential: child.strategic.average_leaf_food_potential,
            average_leaf_hunting_potential: child.strategic.average_leaf_hunting_potential,
        },
        guaranteed_enemy_kills,
        reserved_override_all: causal_reserved_event(&edge.events) || child.reserved_override_all,
    }
}

fn aggregate_direction(
    direction: Direction,
    parent: &SearchNode,
    outcomes: &[EdgeOutcome],
) -> DirectionEvaluation {
    let total_routes = outcomes.iter().fold(0_u64, |sum, outcome| {
        sum.saturating_add(outcome.survival.total_routes)
    });

    let terminal = outcomes
        .iter()
        .map(|outcome| outcome.terminal)
        .min_by_key(|terminal| terminal.rank())
        .unwrap_or(TerminalAssessment::Lost);

    let survival = DirectionSurvivalSummary {
        total_routes,
        death_routes: saturating_sum(outcomes, |outcome| outcome.survival.death_routes),
        dead_end_routes: saturating_sum(outcomes, |outcome| outcome.survival.dead_end_routes),
        forced_routes: saturating_sum(outcomes, |outcome| outcome.survival.forced_routes),
        constrained_routes: saturating_sum(outcomes, |outcome| outcome.survival.constrained_routes),
        min_future_mobility: outcomes
            .iter()
            .map(|outcome| outcome.survival.min_future_mobility)
            .min()
            .unwrap_or(0)
            .min(
                parent
                    .active_analysis()
                    .expect("expanded parent must have analysis")
                    .tactical
                    .ours
                    .safe_moves
                    .len(),
            ),
        min_reachable_space: outcomes
            .iter()
            .map(|outcome| outcome.survival.min_reachable_space)
            .min()
            .unwrap_or(0)
            .min(
                parent
                    .active_analysis()
                    .expect("expanded parent must have analysis")
                    .tactical
                    .ours
                    .best_reachable_space,
            ),
        min_second_order_mobility: outcomes
            .iter()
            .map(|outcome| outcome.survival.min_second_order_mobility)
            .min()
            .unwrap_or(0),
    };

    let worst_strategic_utility = outcomes
        .iter()
        .map(|outcome| outcome.strategic.worst_utility)
        .reduce(f32::min)
        .unwrap_or(0.0);

    let denominator = total_routes.max(1) as f64;
    let average_strategic_utility = weighted_average(outcomes, denominator, |outcome| {
        outcome.strategic.average_utility
    });
    let average_food_value = weighted_average(outcomes, denominator, |outcome| {
        outcome.strategic.average_realized_food
    });
    let average_hunting_value = weighted_average(outcomes, denominator, |outcome| {
        outcome.strategic.average_realized_hunting
    });
    let average_leaf_food_potential = weighted_average(outcomes, denominator, |outcome| {
        outcome.strategic.average_leaf_food_potential
    });
    let average_leaf_hunting_potential = weighted_average(outcomes, denominator, |outcome| {
        outcome.strategic.average_leaf_hunting_potential
    });

    let guaranteed_enemy_kills = outcomes
        .iter()
        .map(|outcome| outcome.guaranteed_enemy_kills)
        .min()
        .unwrap_or(0);

    let mut surviving = outcomes
        .iter()
        .filter(|outcome| outcome.terminal != TerminalAssessment::Lost)
        .peekable();
    let reserved_override =
        surviving.peek().is_some() && surviving.all(|outcome| outcome.reserved_override_all);

    DirectionEvaluation {
        direction,
        terminal,
        survival,
        worst_strategic_utility,
        average_strategic_utility,
        average_food_value,
        average_hunting_value,
        average_leaf_food_potential,
        average_leaf_hunting_potential,
        guaranteed_enemy_kills,
        reserved_override,
    }
}

fn saturating_sum(outcomes: &[EdgeOutcome], value: impl Fn(&EdgeOutcome) -> u64) -> u64 {
    outcomes
        .iter()
        .fold(0_u64, |sum, outcome| sum.saturating_add(value(outcome)))
}

fn weighted_average(
    outcomes: &[EdgeOutcome],
    denominator: f64,
    value: impl Fn(&EdgeOutcome) -> f32,
) -> f32 {
    let weighted = outcomes.iter().fold(0.0_f64, |sum, outcome| {
        sum + f64::from(value(outcome)) * outcome.survival.total_routes.max(1) as f64
    });
    (weighted / denominator) as f32
}

fn next_certainty(current: ForecastCertainty, delta: ForecastDelta) -> ForecastCertainty {
    match (current, delta) {
        (ForecastCertainty::FoodProvisional, _) | (_, ForecastDelta::FoodUncertainty) => {
            ForecastCertainty::FoodProvisional
        }
        _ => ForecastCertainty::Deterministic,
    }
}

fn certainty_discounts(certainty: ForecastCertainty) -> (f32, f32) {
    match certainty {
        ForecastCertainty::Deterministic => (1.0, 1.0),
        ForecastCertainty::FoodProvisional => (0.50, 0.80),
    }
}

fn edge_hunting_delta(parent: &SearchNode, edge: &SearchEdge) -> f32 {
    let mut target_levels: HashMap<&str, f32> = HashMap::new();

    for event in &edge.events {
        let target = match event {
            InstantEvent::EnemyForced {
                enemy,
                caused_by_ours: true,
                ..
            } => Some((enemy.as_str(), 0.25)),
            InstantEvent::EnemyTrapped {
                enemy,
                caused_by_ours: true,
            } => Some((enemy.as_str(), 0.60)),
            InstantEvent::EnemyKilled {
                enemy,
                attribution: EliminationAttribution::OurSnake,
                ..
            }
            | InstantEvent::HeadToHeadWon { enemy } => Some((enemy.as_str(), 1.0)),
            _ => None,
        };

        if let Some((enemy, level)) = target {
            target_levels
                .entry(enemy)
                .and_modify(|current| *current = current.max(level))
                .or_insert(level);
        }
    }

    target_levels
        .into_iter()
        .map(|(enemy, target)| {
            let before = enemy_pressure_level(parent, enemy);
            (target - before).max(0.0)
        })
        .sum()
}

fn enemy_pressure_level(node: &SearchNode, enemy_id: &str) -> f32 {
    let Some(enemy) = node
        .active_analysis()
        .and_then(|analysis| analysis.tactical.enemies.get(enemy_id))
    else {
        return 0.0;
    };

    if enemy.legal_moves.is_empty() {
        0.60
    } else if enemy.plausible_moves.len() == 1 {
        0.25
    } else {
        0.0
    }
}

fn edge_enemy_kills(edge: &SearchEdge) -> u16 {
    let mut killed = HashSet::new();

    for event in &edge.events {
        match event {
            InstantEvent::EnemyKilled {
                enemy,
                attribution: EliminationAttribution::OurSnake,
                ..
            }
            | InstantEvent::HeadToHeadWon { enemy } => {
                killed.insert(enemy.as_str());
            }
            _ => {}
        }
    }

    killed.len().try_into().unwrap_or(u16::MAX)
}

fn causal_reserved_event(events: &[InstantEvent]) -> bool {
    events.iter().any(|event| {
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
}

#[derive(Debug, Clone, Copy, Default)]
struct LeafPotential {
    food: f32,
    hunting: f32,
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
                .active_analysis()
                .expect("running leaf must have analysis")
                .state
                .route_for(&node.state.our_snake_id, *food)?;
            let distance = route.distance?;
            let claim_factor = node
                .active_analysis()
                .expect("running leaf must have analysis")
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

    node.active_analysis()
        .expect("running leaf must have analysis")
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

pub(crate) fn choose_best_direction<'a>(
    evaluations: &'a [DirectionEvaluation],
    state: &crate::simulation::state::SimulatedGameState,
    robust_safe_moves: MoveMask,
    policy: ReservedCellPolicy,
) -> Option<&'a DirectionEvaluation> {
    evaluations
        .iter()
        .filter(|evaluation| {
            robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction)
        })
        .min_by(|left, right| compare_direction(left, right, state, policy))
}

pub(crate) fn compare_direction(
    left: &DirectionEvaluation,
    right: &DirectionEvaluation,
    state: &crate::simulation::state::SimulatedGameState,
    policy: ReservedCellPolicy,
) -> Ordering {
    terminal_compare(left.terminal, right.terminal)
        .then_with(|| {
            left.survival
                .has_death_response()
                .cmp(&right.survival.has_death_response())
        })
        .then_with(|| {
            left.survival
                .has_dead_end_response()
                .cmp(&right.survival.has_dead_end_response())
        })
        .then_with(|| {
            left.survival
                .has_forced_response()
                .cmp(&right.survival.has_forced_response())
        })
        .then_with(|| {
            left.survival
                .has_constrained_response()
                .cmp(&right.survival.has_constrained_response())
        })
        .then_with(|| {
            effective_reserved_penalty(policy, state, left)
                .total_cmp(&effective_reserved_penalty(policy, state, right))
        })
        .then_with(|| {
            right
                .worst_strategic_utility
                .total_cmp(&left.worst_strategic_utility)
        })
        .then_with(|| {
            right
                .average_strategic_utility
                .total_cmp(&left.average_strategic_utility)
        })
        .then_with(|| {
            right
                .survival
                .min_future_mobility
                .cmp(&left.survival.min_future_mobility)
        })
        .then_with(|| {
            right
                .survival
                .min_reachable_space
                .cmp(&left.survival.min_reachable_space)
        })
        .then_with(|| {
            right
                .survival
                .min_second_order_mobility
                .cmp(&left.survival.min_second_order_mobility)
        })
        .then_with(|| left.direction.rank().cmp(&right.direction.rank()))
}

fn terminal_compare(left: TerminalAssessment, right: TerminalAssessment) -> Ordering {
    right.rank().cmp(&left.rank())
}

fn effective_reserved_penalty(
    policy: ReservedCellPolicy,
    state: &crate::simulation::state::SimulatedGameState,
    evaluation: &DirectionEvaluation,
) -> f32 {
    if evaluation.reserved_override {
        0.0
    } else {
        policy.penalty(state, evaluation.direction)
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

    fn summary(total: u64, deaths: u64, mobility: u8) -> DirectionSurvivalSummary {
        DirectionSurvivalSummary {
            total_routes: total,
            death_routes: deaths,
            dead_end_routes: 0,
            forced_routes: 0,
            constrained_routes: 0,
            min_future_mobility: mobility,
            min_reachable_space: 20,
            min_second_order_mobility: u32::from(mobility),
        }
    }

    fn evaluation(direction: Direction, survival: DirectionSurvivalSummary) -> DirectionEvaluation {
        DirectionEvaluation {
            direction,
            terminal: if survival.death_routes > 0 {
                TerminalAssessment::Lost
            } else {
                TerminalAssessment::Running
            },
            survival,
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        }
    }

    #[test]
    fn graph_is_evaluated_without_materializing_routes() {
        let mut graph = FutureGraph::new(state(vec![]));
        graph.expand_to_depth(2).unwrap();

        let result = evaluate_graph_inner(&graph, 2, None).unwrap();

        assert!(!result.directions.is_empty());
        assert!(result.stats.nodes_evaluated <= graph.node_count() as u32 * 2);
    }

    #[test]
    fn realized_food_stays_separate_from_leaf_potential() {
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

        assert!(right.average_food_value > 0.0);
        assert_eq!(up.average_food_value, 0.0);
        assert!(up.average_leaf_food_potential >= 0.0);
    }

    #[test]
    fn adversarial_death_response_beats_safe_route_ratio() {
        let safe = evaluation(Direction::Up, summary(10, 0, 2));
        let dangerous = evaluation(Direction::Right, summary(100, 1, 4));
        let state = state(vec![]);

        assert_eq!(
            compare_direction(&safe, &dangerous, &state, ReservedCellPolicy::default(),),
            Ordering::Less
        );
    }

    #[test]
    fn terminal_order_is_won_running_lost() {
        assert_eq!(
            terminal_compare(TerminalAssessment::Won, TerminalAssessment::Running),
            Ordering::Less
        );
        assert_eq!(
            terminal_compare(TerminalAssessment::Running, TerminalAssessment::Lost),
            Ordering::Less
        );
    }

    #[test]
    fn food_uncertainty_uses_separate_memo_state() {
        let mut graph = FutureGraph::new(state(vec![Coord { x: 2, y: 1 }]));
        graph.expand_to_depth(2).unwrap();

        let result = evaluate_graph_inner(&graph, 2, None).unwrap();

        assert!(result.stats.provisional_evaluations > 0);
    }

    #[test]
    fn hunting_delta_is_incremental() {
        let mut graph = FutureGraph::new(state(vec![]));
        graph.expand_to_depth(1).unwrap();
        let root = graph.node(graph.root());

        for edge in &root.children {
            let delta = edge_hunting_delta(root, edge);
            assert!(delta >= 0.0);
            assert!(delta <= 1.0);
        }
    }
}
