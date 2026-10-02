#![allow(dead_code)]

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet, VecDeque};
use std::time::Duration;

use crate::decision::evaluation::{compare_direction, DirectionEvaluation};
use crate::decision::intent::DecisionIntent;
use crate::decision::policy::ReservedCellPolicy;
use crate::direction::Direction;
use crate::search::budget::SearchBudget;
use crate::search::graph::{FutureGraph, NodeId, SearchError};
use crate::search::priority::{FrontierPriority, PrioritySignals};
use crate::search::trend::SearchTrend;

const MAX_SELECTIVE_DEPTH: u8 = 20;
const MIN_EXPANSION_SLICE: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SelectiveSearchStats {
    pub(crate) expansions: u32,
    pub(crate) frontier_peak: u32,
    pub(crate) max_selective_depth: u8,
    pub(crate) reused_expansions: u32,
    pub(crate) interrupted_expansions: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrontierEntry {
    node_id: NodeId,
    parent_id: Option<NodeId>,
    root_direction: Direction,
    depth: u8,
    priority: FrontierPriority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnqueueContext {
    node_id: NodeId,
    parent_id: Option<NodeId>,
    root_direction: Direction,
    depth: u8,
    base_depth: u8,
    root_relevance: u16,
}

impl Ord for FrontierEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority.cmp(&other.priority)
    }
}

impl PartialOrd for FrontierEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SelectiveSearchScheduler;

impl SelectiveSearchScheduler {
    pub(crate) fn run(
        &self,
        graph: &mut FutureGraph,
        base_evaluations: &[DirectionEvaluation],
        budget: &SearchBudget,
        base_depth: u8,
        intent: Option<&DecisionIntent>,
        reevaluation_reserve: Duration,
    ) -> Result<SelectiveSearchStats, SearchError> {
        if base_evaluations.is_empty() || budget.soft_expired() {
            return Ok(SelectiveSearchStats::default());
        }

        let relevance = root_relevance(graph, base_evaluations);
        let mut frontier = BinaryHeap::new();
        let mut enqueued = HashSet::new();

        for (node_id, parent_id, root_direction, depth) in base_frontier(graph, base_depth) {
            enqueue(
                graph,
                &mut frontier,
                &mut enqueued,
                EnqueueContext {
                    node_id,
                    parent_id: Some(parent_id),
                    root_direction,
                    depth,
                    base_depth,
                    root_relevance: relevance[usize::from(root_direction.rank())],
                },
                intent,
            );
        }

        let mut stats = SelectiveSearchStats {
            frontier_peak: frontier.len().try_into().unwrap_or(u32::MAX),
            ..SelectiveSearchStats::default()
        };

        while budget.can_afford_soft(MIN_EXPANSION_SLICE.saturating_add(reevaluation_reserve)) {
            let Some(entry) = frontier.pop() else {
                break;
            };

            if entry.depth >= MAX_SELECTIVE_DEPTH || graph.node(entry.node_id).is_terminal() {
                continue;
            }

            let expansion = graph.expand_frontier(entry.node_id, budget)?;
            if !expansion.completed {
                stats.interrupted_expansions = stats.interrupted_expansions.saturating_add(1);
                break;
            }

            if expansion.expanded {
                stats.expansions = stats.expansions.saturating_add(1);
            } else {
                stats.reused_expansions = stats.reused_expansions.saturating_add(1);
            }
            let children = graph
                .node(entry.node_id)
                .children
                .iter()
                .map(|edge| edge.child)
                .collect::<Vec<_>>();

            let next_depth = entry.depth.saturating_add(1);
            stats.max_selective_depth = stats.max_selective_depth.max(next_depth);
            let root_relevance = relevance[usize::from(entry.root_direction.rank())];
            for child in children {
                enqueue(
                    graph,
                    &mut frontier,
                    &mut enqueued,
                    EnqueueContext {
                        node_id: child,
                        parent_id: Some(entry.node_id),
                        root_direction: entry.root_direction,
                        depth: next_depth,
                        base_depth,
                        root_relevance,
                    },
                    intent,
                );
            }

            stats.frontier_peak = stats
                .frontier_peak
                .max(frontier.len().try_into().unwrap_or(u32::MAX));
        }

        Ok(stats)
    }
}

fn base_frontier(graph: &FutureGraph, base_depth: u8) -> Vec<(NodeId, NodeId, Direction, u8)> {
    let root = graph.root();
    let root_state = &graph.node(root).state;
    let Some(our_actor) = root_state.actor_index(&root_state.our_snake_id) else {
        return Vec::new();
    };
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    let mut frontier = Vec::new();

    for edge in &graph.node(root).children {
        let Some(direction) = edge.joint_action.direction_for(our_actor) else {
            continue;
        };
        queue.push_back((edge.child, root, direction, 1_u8));
    }

    while let Some((node_id, parent_id, root_direction, depth)) = queue.pop_front() {
        let key = (node_id, root_direction.rank(), depth);
        if !seen.insert(key) {
            continue;
        }

        if depth >= base_depth {
            frontier.push((node_id, parent_id, root_direction, depth));
            continue;
        }

        for edge in &graph.node(node_id).children {
            queue.push_back((edge.child, node_id, root_direction, depth.saturating_add(1)));
        }
    }

    frontier
}

fn enqueue(
    graph: &FutureGraph,
    frontier: &mut BinaryHeap<FrontierEntry>,
    enqueued: &mut HashSet<(NodeId, u8, u8)>,
    context: EnqueueContext,
    intent: Option<&DecisionIntent>,
) {
    if graph.node(context.node_id).is_terminal() {
        return;
    }

    let key = (
        context.node_id,
        context.root_direction.rank(),
        context.depth,
    );
    if !enqueued.insert(key) {
        return;
    }

    let signals = priority_signals(
        graph,
        context.node_id,
        context.parent_id,
        context.root_relevance,
        context.depth.saturating_sub(context.base_depth),
        intent,
    );
    frontier.push(FrontierEntry {
        node_id: context.node_id,
        parent_id: context.parent_id,
        root_direction: context.root_direction,
        depth: context.depth,
        priority: FrontierPriority::new(
            signals,
            context.root_direction,
            context.depth,
            context.node_id,
        ),
    });
}

fn priority_signals(
    graph: &FutureGraph,
    node_id: NodeId,
    parent_id: Option<NodeId>,
    root_relevance: u16,
    depth_beyond_base: u8,
    intent: Option<&DecisionIntent>,
) -> PrioritySignals {
    let node = graph.node(node_id);
    let Some(analysis) = node.active_analysis() else {
        return PrioritySignals::default();
    };

    let hunt_target = match intent {
        Some(DecisionIntent::Hunt(hunt)) => Some(hunt.target.as_str()),
        _ => None,
    };
    let trend = parent_id
        .map(|parent_id| SearchTrend::between(graph.node(parent_id), node, hunt_target))
        .unwrap_or_default();

    let ours = analysis.enclosure.ours(&node.state);
    let safe_moves = analysis.tactical.ours.safe_moves.len();
    let state_danger = ours.map_or(0, |snapshot| {
        let risk = u16::from(snapshot.risk.rank()).saturating_mul(300);
        let mobility = match safe_moves {
            0 => 1000,
            1 => 850,
            2 => 450,
            _ => 0,
        };
        let space = match snapshot.space_to_length_milli {
            0..=1250 => 1000,
            1251..=1600 => 800,
            1601..=2200 => 500,
            2201..=2800 => 250,
            _ => 0,
        };
        let border = analysis.border.ours().map_or(0, |snapshot| {
            snapshot
                .structural_risk_milli
                .max(snapshot.enemy_pin_risk_milli)
        });
        risk.max(mobility).max(space).max(border).min(1000)
    });
    let danger = state_danger.max(trend.danger_priority_milli());

    let intent_focus = intent_focus(node, intent);
    let trend_tactical = trend.tactical_priority_milli();
    let raw_hunting_tactical = (analysis.hunting.best_plan_score() * 1000.0)
        .round()
        .clamp(0.0, 1000.0) as u16;
    let posture_hunting_tactical = u32::from(raw_hunting_tactical)
        .saturating_mul(u32::from(analysis.posture.hunt_drive_milli))
        .saturating_div(1000)
        .try_into()
        .unwrap_or(u16::MAX);
    let tactical = posture_hunting_tactical
        .max(intent_focus)
        .max(trend_tactical);

    let forcing = forcing_score(node);
    let relevance_penalty = u16::from(depth_beyond_base).saturating_mul(45);
    let relevance = root_relevance
        .saturating_sub(relevance_penalty)
        .max(intent_focus.saturating_sub(relevance_penalty / 2))
        .max(trend_tactical.saturating_sub(relevance_penalty / 2));
    let refutation = u32::from(danger)
        .saturating_mul(u32::from(root_relevance))
        .saturating_div(1000)
        .try_into()
        .unwrap_or(u16::MAX);

    PrioritySignals {
        refutation,
        danger,
        relevance,
        tactical,
        forcing,
        uncertainty: 0,
    }
}

fn intent_focus(node: &crate::search::graph::SearchNode, intent: Option<&DecisionIntent>) -> u16 {
    let Some(intent) = intent else {
        return 0;
    };
    let Some(analysis) = node.active_analysis() else {
        return 0;
    };

    match intent {
        DecisionIntent::Food(food) => {
            if !node.state.food.contains(&food.target) {
                return 1000;
            }

            let Some(route) = analysis
                .state
                .route_for(&node.state.our_snake_id, food.target)
            else {
                return 0;
            };
            let Some(distance) = route.distance else {
                return 0;
            };

            1000_u32
                .saturating_div(u32::from(distance).saturating_add(1))
                .try_into()
                .unwrap_or(0)
        }
        DecisionIntent::Hunt(hunt) => {
            let enclosure = analysis.enclosure.for_snake(&hunt.target);
            let risk = enclosure.map_or(0, |snapshot| {
                u16::from(snapshot.risk.rank()).saturating_mul(180)
            });
            let escape = enclosure.map_or(0, |snapshot| {
                4_u16
                    .saturating_sub(u16::from(snapshot.escape_frontier.min(4)))
                    .saturating_mul(90)
            });
            let boundary = enclosure.map_or(0, |snapshot| {
                u16::from(snapshot.boundary_support).saturating_mul(70)
            });
            let plan = analysis
                .hunting
                .plans
                .iter()
                .find(|plan| plan.target == hunt.target && plan.kind == hunt.kind)
                .map_or(0, |plan| plan.score_milli);
            let competitive = analysis
                .territory
                .competitive_for_snake(&hunt.target)
                .map_or(0, |snapshot| {
                    1000_u16.saturating_sub(snapshot.control_ratio_milli)
                });

            risk.saturating_add(escape)
                .saturating_add(boundary)
                .max(plan)
                .max(competitive)
                .min(1000)
        }
        DecisionIntent::Escape(escape) => {
            let current = analysis.enclosure.ours(&node.state);
            let enclosure = current.map_or(0, |snapshot| {
                u16::from(snapshot.risk.rank()).saturating_mul(250)
            });
            let mobility = match analysis.tactical.ours.safe_moves.len() {
                0 => 1000,
                1 => 850,
                2 => 450,
                _ => 0,
            };
            let pin = analysis
                .border
                .ours()
                .map_or(0, |snapshot| snapshot.enemy_pin_risk_milli);
            enclosure
                .max(mobility)
                .max(pin)
                .max(escape.last_pressure_milli)
                .min(1000)
        }
    }
}

fn forcing_score(node: &crate::search::graph::SearchNode) -> u16 {
    let Some(analysis) = node.active_analysis() else {
        return 0;
    };

    let our_moves = u32::from(analysis.tactical.ours.safe_moves.len().max(1));
    let enemy_branching = analysis
        .tactical
        .enemies
        .values()
        .fold(1_u32, |product, enemy| {
            let moves = if enemy.plausible_moves.is_empty() {
                enemy.legal_moves.len()
            } else {
                enemy.plausible_moves.len()
            };
            product.saturating_mul(u32::from(moves.max(1))).min(64)
        });

    let branching = our_moves.saturating_mul(enemy_branching).max(1);
    (1000_u32.saturating_div(branching).min(1000))
        .try_into()
        .unwrap_or(0)
}

fn root_relevance(graph: &FutureGraph, evaluations: &[DirectionEvaluation]) -> [u16; 4] {
    let root = graph.node(graph.root());
    let policy = ReservedCellPolicy::default();

    let mut ranked = evaluations.iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| compare_direction(left, right, &root.state, policy));

    let mut relevance = [0_u16; 4];
    let rank_values = [1000_u16, 825, 600, 350];

    for (index, evaluation) in ranked.into_iter().enumerate() {
        relevance[usize::from(evaluation.direction.rank())] =
            rank_values.get(index).copied().unwrap_or(200);
    }

    relevance
}
