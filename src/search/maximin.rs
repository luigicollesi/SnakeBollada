#![allow(dead_code)]

use std::collections::HashMap;

use crate::direction::Direction;
use crate::evaluation::{ActorVec, TransitionScore};
use crate::simulation::state::ActorIndex;

use super::beam::{select_seed_beam, BeamLine, BeamPath, BeamStep, LineId, LineTerminal};
use super::bounds::ValueBound;
use super::forecast::{ForecastCertainty, PROVISIONAL_TERMINAL_VALUE};
use super::graph::{FutureGraph, NodeId, SearchEdge, SearchNode};

const TERMINAL_VALUE: i64 = 1_000_000_000;
const INCOMPLETE_MARGIN: i64 = 20_000;
const MAX_VARIANTS_PER_NODE: usize = 3;
const OPPONENT_RESPONSE_UTILITY_SLACK: i64 = 100;
const MIN_NEAR_BEST_PLAUSIBILITY_MILLI: u16 = 250;
const OPPONENT_RESPONSE_PLAUSIBILITY_SLACK_MILLI: u16 = 150;
const GROWTH_FRONTIER_PRESSURE_NUMERATOR: i64 = 3;
const GROWTH_FRONTIER_PRESSURE_DENOMINATOR: i64 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MaximinStats {
    pub(crate) nodes_evaluated: u32,
    pub(crate) memo_hits: u32,
    pub(crate) exact_nodes: u32,
    pub(crate) bounded_nodes: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct SeedEvaluation {
    pub(crate) lines: Vec<BeamLine>,
    pub(crate) stats: MaximinStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContinuationEvaluation {
    pub(crate) depth: u8,
    pub(crate) benefit_total: i64,
    pub(crate) harm_total: i64,
    pub(crate) our_utility_total: i64,
    pub(crate) opponent_utility_total: i64,
    pub(crate) actor_utility_totals: ActorVec<i64>,
    pub(crate) value: i64,
    pub(crate) terminal: LineTerminal,
    pub(crate) certainty: ForecastCertainty,
    pub(crate) bound: ValueBound,
    pub(crate) path: BeamPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EvaluatedLine {
    value: i64,
    benefit_total: i64,
    harm_total: i64,
    our_utility_total: i64,
    opponent_utility_total: i64,
    actor_utility_totals: ActorVec<i64>,
    terminal: LineTerminal,
    certainty: ForecastCertainty,
    bound: ValueBound,
    path: BeamPath,
}

impl EvaluatedLine {
    fn shifted_by_edge(
        mut self,
        node: NodeId,
        edge: &SearchEdge,
        transition: &TransitionScore,
    ) -> Self {
        for (actor, score) in transition.actors.iter() {
            // Forecast uncertainty belongs to the route value, not to the actor's
            // willingness to die. Keeping the raw actor terminal here prevents a
            // long survivable line from looking worse to the opponent than death.
            self.actor_utility_totals.add(actor, score.actor_choice_net);
        }

        if self.terminal == LineTerminal::Running || self.certainty.is_provisional() {
            self.our_utility_total = self.our_utility_total.saturating_add(transition.net);
            self.opponent_utility_total = self
                .opponent_utility_total
                .saturating_add(transition.opponent_net_total);
            self.value = route_value(self.our_utility_total, self.opponent_utility_total);
            self.bound = shift_bound(self.bound, transition.route_delta());
        }

        self.benefit_total = self
            .benefit_total
            .saturating_add(transition.effective_benefit);
        self.harm_total = self.harm_total.saturating_add(transition.effective_harm);
        self.path = self.path.prepend(BeamStep {
            node,
            joint_action: edge.joint_action.clone(),
            child: edge.child,
        });
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    node: NodeId,
    remaining_depth: u8,
    certainty: ForecastCertainty,
}

struct MaximinEvaluator<'a> {
    graph: &'a FutureGraph,
    memo: HashMap<MemoKey, Vec<EvaluatedLine>>,
    stats: MaximinStats,
}

pub(crate) fn evaluate_seed_lines(graph: &FutureGraph, target_depth: u8) -> SeedEvaluation {
    let mut evaluator = MaximinEvaluator {
        graph,
        memo: HashMap::new(),
        stats: MaximinStats::default(),
    };

    let root = graph.root();
    let node = graph.node(root);
    let mut lines = Vec::new();
    let mut next_id = 1_u32;

    if target_depth == 0 || node.is_terminal() {
        return SeedEvaluation {
            lines,
            stats: evaluator.stats,
        };
    }

    for direction in Direction::ALL {
        let variants = evaluator.evaluate_direction_variants(
            root,
            direction,
            target_depth,
            ForecastCertainty::Deterministic,
        );
        for line in variants {
            let depth = line.path.len().try_into().unwrap_or(u8::MAX);
            lines.push(BeamLine {
                id: LineId(next_id),
                root_direction: direction,
                depth,
                benefit_total: line.benefit_total,
                harm_total: line.harm_total,
                our_utility_total: line.our_utility_total,
                opponent_utility_total: line.opponent_utility_total,
                actor_utility_totals: line.actor_utility_totals,
                value: line.value,
                terminal: line.terminal,
                certainty: line.certainty,
                bound: line.bound,
                path: line.path,
            });
            next_id = next_id.saturating_add(1);
        }
    }

    sort_beam_lines(&mut lines);

    SeedEvaluation {
        lines,
        stats: evaluator.stats,
    }
}

pub(crate) fn evaluate_seed_beam(graph: &FutureGraph, target_depth: u8) -> SeedEvaluation {
    let mut evaluation = evaluate_seed_lines(graph, target_depth);
    evaluation.lines = select_seed_beam(&evaluation.lines);
    evaluation
}

pub(crate) fn evaluate_continuations(
    graph: &FutureGraph,
    start_node: NodeId,
    target_depth: u8,
    certainty: ForecastCertainty,
) -> Vec<ContinuationEvaluation> {
    let mut evaluator = MaximinEvaluator {
        graph,
        memo: HashMap::new(),
        stats: MaximinStats::default(),
    };

    evaluator
        .evaluate_node_variants(start_node, target_depth, certainty)
        .into_iter()
        .map(|line| ContinuationEvaluation {
            depth: line.path.len().try_into().unwrap_or(u8::MAX),
            benefit_total: line.benefit_total,
            harm_total: line.harm_total,
            our_utility_total: line.our_utility_total,
            opponent_utility_total: line.opponent_utility_total,
            actor_utility_totals: line.actor_utility_totals,
            value: line.value,
            terminal: line.terminal,
            certainty: line.certainty,
            bound: line.bound,
            path: line.path,
        })
        .collect()
}

impl MaximinEvaluator<'_> {
    fn evaluate_node(
        &mut self,
        node_id: NodeId,
        remaining_depth: u8,
        certainty: ForecastCertainty,
    ) -> EvaluatedLine {
        self.evaluate_node_variants(node_id, remaining_depth, certainty)
            .into_iter()
            .next()
            .unwrap_or_else(|| frontier_line(self.graph.node(node_id), false, certainty))
    }

    fn evaluate_node_variants(
        &mut self,
        node_id: NodeId,
        remaining_depth: u8,
        certainty: ForecastCertainty,
    ) -> Vec<EvaluatedLine> {
        let key = MemoKey {
            node: node_id,
            remaining_depth,
            certainty,
        };
        if let Some(cached) = self.memo.get(&key) {
            self.stats.memo_hits = self.stats.memo_hits.saturating_add(1);
            return cached.clone();
        }

        self.stats.nodes_evaluated = self.stats.nodes_evaluated.saturating_add(1);
        let node = self.graph.node(node_id);

        let mut result = if let Some(terminal) = terminal_line(node, certainty) {
            self.stats.exact_nodes = self.stats.exact_nodes.saturating_add(1);
            vec![terminal]
        } else if remaining_depth == 0 {
            self.stats.exact_nodes = self.stats.exact_nodes.saturating_add(1);
            vec![frontier_line(node, true, certainty)]
        } else if !node.expansion_complete() {
            self.stats.bounded_nodes = self.stats.bounded_nodes.saturating_add(1);
            vec![frontier_line(node, false, certainty)]
        } else {
            let mut variants = Vec::new();
            for direction in Direction::ALL {
                variants.extend(self.evaluate_direction_variants(
                    node_id,
                    direction,
                    remaining_depth,
                    certainty,
                ));
            }

            if variants.is_empty() {
                self.stats.bounded_nodes = self.stats.bounded_nodes.saturating_add(1);
                vec![frontier_line(node, false, certainty)]
            } else {
                variants
            }
        };

        rank_and_dedup_variants(&mut result, MAX_VARIANTS_PER_NODE);
        self.memo.insert(key, result.clone());
        result
    }

    fn evaluate_direction_variants(
        &mut self,
        node_id: NodeId,
        direction: Direction,
        remaining_depth: u8,
        certainty: ForecastCertainty,
    ) -> Vec<EvaluatedLine> {
        let node = self.graph.node(node_id);
        let Some(our_actor) = node.state.actor_index(&node.state.our_snake_id) else {
            return Vec::new();
        };
        let edges = node
            .children
            .iter()
            .filter(|edge| edge.joint_action.direction_for(our_actor) == Some(direction))
            .collect::<Vec<_>>();

        if edges.is_empty() {
            return Vec::new();
        }

        let mut edge_variants = Vec::with_capacity(edges.len());
        for edge in edges {
            let child = self.graph.node(edge.child);
            let child_certainty = if child.is_terminal() {
                certainty
            } else {
                certainty.after(edge.forecast_delta)
            };
            let variants = self
                .evaluate_node_variants(
                    edge.child,
                    remaining_depth.saturating_sub(1),
                    child_certainty,
                )
                .into_iter()
                .map(|line| line.shifted_by_edge(node_id, edge, &edge.transition))
                .collect::<Vec<_>>();

            if variants.is_empty() {
                continue;
            }
            edge_variants.push(variants);
        }

        if edge_variants.is_empty() {
            return Vec::new();
        }

        let mut policies = Vec::new();
        policies.push(select_selfish_opponent_response(
            self.graph,
            node,
            edge_variants
                .iter()
                .filter_map(|variants| variants.first().cloned())
                .collect(),
        ));

        for edge_index in 0..edge_variants.len() {
            for variant_index in 1..edge_variants[edge_index].len() {
                let selected = edge_variants
                    .iter()
                    .enumerate()
                    .filter_map(|(index, variants)| {
                        let chosen = if index == edge_index {
                            variants.get(variant_index)
                        } else {
                            variants.first()
                        };
                        chosen.cloned()
                    })
                    .collect::<Vec<_>>();

                if selected.len() == edge_variants.len() {
                    policies.push(select_selfish_opponent_response(self.graph, node, selected));
                }
            }
        }

        if !node.expansion_complete() {
            for policy in &mut policies {
                policy.bound = widen_bound(policy.bound, policy.value);
            }
        }

        // The variants above are alternative continuations used to evaluate which
        // current opponent response is rational. They are not additional choices
        // available to us. Returning several policies here lets the upper MAX layer
        // pick the most optimistic opponent continuation, which can model a rational
        // opponent as voluntarily taking a losing head-to-head. Collapse them back
        // to the single selfish response before exposing this direction upward.
        vec![select_selfish_opponent_response(self.graph, node, policies)]
    }
}

fn frontier_line(node: &SearchNode, exact: bool, certainty: ForecastCertainty) -> EvaluatedLine {
    let mut actor_utility_totals = ActorVec::with_capacity(node.state.snakes.len());
    let mut our_utility_total = 0_i64;
    let mut opponent_utility_total = 0_i64;

    if let Some(analysis) = node.active_analysis() {
        for (index, snake) in node
            .state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive)
        {
            let Some(actor) = ActorIndex::new(index) else {
                continue;
            };
            let pressure = analysis
                .actor_snapshot(actor)
                .map(|snapshot| snapshot.metrics.growth_pressure_milli)
                .unwrap_or(0);
            let utility = i64::from(pressure)
                .saturating_mul(GROWTH_FRONTIER_PRESSURE_NUMERATOR)
                .saturating_div(GROWTH_FRONTIER_PRESSURE_DENOMINATOR)
                .saturating_neg();
            actor_utility_totals.insert(actor, utility);

            if snake.id == node.state.our_snake_id {
                our_utility_total = utility;
            } else {
                opponent_utility_total = opponent_utility_total.saturating_add(utility);
            }
        }
    }

    let value = route_value(our_utility_total, opponent_utility_total);
    EvaluatedLine {
        value,
        benefit_total: value.max(0),
        harm_total: value.saturating_neg().max(0),
        our_utility_total,
        opponent_utility_total,
        actor_utility_totals,
        terminal: LineTerminal::Running,
        certainty,
        bound: if exact {
            ValueBound::Exact(value)
        } else {
            incomplete_bound(value)
        },
        path: BeamPath::empty(),
    }
}

fn terminal_line(node: &SearchNode, certainty: ForecastCertainty) -> Option<EvaluatedLine> {
    let ours_alive = node
        .state
        .snake(&node.state.our_snake_id)
        .is_some_and(|snake| snake.alive);
    if !ours_alive {
        let value = if certainty.is_provisional() {
            -PROVISIONAL_TERMINAL_VALUE
        } else {
            -TERMINAL_VALUE
        };
        return Some(EvaluatedLine {
            value,
            benefit_total: 0,
            harm_total: value.saturating_abs(),
            our_utility_total: value,
            opponent_utility_total: 0,
            actor_utility_totals: ActorVec::new(),
            terminal: LineTerminal::Lost,
            certainty,
            bound: ValueBound::Exact(value),
            path: BeamPath::empty(),
        });
    }

    let enemy_alive = node
        .state
        .snakes
        .iter()
        .any(|snake| snake.alive && snake.id != node.state.our_snake_id);
    if enemy_alive {
        return None;
    }

    let value = if certainty.is_provisional() {
        PROVISIONAL_TERMINAL_VALUE
    } else {
        TERMINAL_VALUE
    };
    Some(EvaluatedLine {
        value,
        benefit_total: value,
        harm_total: 0,
        our_utility_total: value,
        opponent_utility_total: 0,
        actor_utility_totals: ActorVec::new(),
        terminal: LineTerminal::Won,
        certainty,
        bound: ValueBound::Exact(value),
        path: BeamPath::empty(),
    })
}

fn max_our_choices(lines: Vec<EvaluatedLine>) -> EvaluatedLine {
    lines
        .into_iter()
        .max_by(compare_our_lines)
        .expect("MAX requires at least one line")
}

fn select_selfish_opponent_response(
    graph: &FutureGraph,
    node: &SearchNode,
    lines: Vec<EvaluatedLine>,
) -> EvaluatedLine {
    let lower = lines
        .iter()
        .map(|line| line.bound.lower())
        .min()
        .unwrap_or(i64::MIN);
    let upper = lines
        .iter()
        .map(|line| line.bound.upper())
        .max()
        .unwrap_or(i64::MAX);
    let all_exact = lines.iter().all(|line| line.bound.is_exact());

    let enemies = node
        .state
        .snakes
        .iter()
        .enumerate()
        .filter(|(_, snake)| snake.alive && snake.id != node.state.our_snake_id)
        .filter_map(|(index, _)| ActorIndex::new(index))
        .collect::<Vec<_>>();

    let near_best_responses = lines
        .iter()
        .filter(|candidate| is_near_best_response(graph, node, candidate, &lines, &enemies))
        .cloned()
        .collect::<Vec<_>>();

    // A kill is only proven when the opponent has no strategically plausible
    // escape. Actor utility is an estimate; it must not turn one preferred
    // response into a forced win when another plausible legal response survives.
    let plausible_terminal_escapes = if lines.iter().any(|line| line.terminal == LineTerminal::Won)
    {
        lines
            .iter()
            .filter(|candidate| {
                candidate.terminal != LineTerminal::Won
                    && is_plausibility_supported_response(graph, node, candidate, &lines, &enemies)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let response_pool = if plausible_terminal_escapes.is_empty() {
        near_best_responses
    } else {
        plausible_terminal_escapes
    };

    let mut chosen = if response_pool.is_empty() {
        lines
            .iter()
            .min_by(|left, right| {
                let left_regret = unilateral_regret(node, left, &enemies, &lines);
                let right_regret = unilateral_regret(node, right, &enemies, &lines);

                left_regret
                    .cmp(&right_regret)
                    .then_with(|| left.value.cmp(&right.value))
                    .then_with(|| {
                        right
                            .opponent_utility_total
                            .cmp(&left.opponent_utility_total)
                    })
            })
            .cloned()
            .expect("opponent response selection requires at least one line")
    } else {
        response_pool
            .into_iter()
            .min_by(|left, right| {
                left.value.cmp(&right.value).then_with(|| {
                    right
                        .opponent_utility_total
                        .cmp(&left.opponent_utility_total)
                })
            })
            .expect("pure best-response set cannot be empty")
    };

    chosen.bound = if all_exact {
        ValueBound::Exact(chosen.value)
    } else {
        ValueBound::Interval { lower, upper }
    };
    chosen
}

fn is_near_best_response(
    graph: &FutureGraph,
    node: &SearchNode,
    candidate: &EvaluatedLine,
    lines: &[EvaluatedLine],
    enemies: &[ActorIndex],
) -> bool {
    enemies.iter().all(|enemy_id| {
        let current = actor_utility(candidate, *enemy_id);
        let alternatives = lines
            .iter()
            .filter(|alternative| {
                same_joint_context_except_actor(node, candidate, alternative, *enemy_id)
            })
            .collect::<Vec<_>>();
        let best = alternatives
            .iter()
            .map(|alternative| actor_utility(alternative, *enemy_id))
            .max()
            .unwrap_or(current);
        let regret = best.saturating_sub(current);

        if regret > OPPONENT_RESPONSE_UTILITY_SLACK {
            return false;
        }
        if regret == 0 {
            return true;
        }

        response_plausibility_supported(graph, node, candidate, &alternatives, *enemy_id)
    })
}

fn is_plausibility_supported_response(
    graph: &FutureGraph,
    node: &SearchNode,
    candidate: &EvaluatedLine,
    lines: &[EvaluatedLine],
    enemies: &[ActorIndex],
) -> bool {
    enemies.iter().all(|enemy_id| {
        let alternatives = lines
            .iter()
            .filter(|alternative| {
                same_joint_context_except_actor(node, candidate, alternative, *enemy_id)
            })
            .collect::<Vec<_>>();
        response_plausibility_supported(graph, node, candidate, &alternatives, *enemy_id)
    })
}

fn response_plausibility_supported(
    graph: &FutureGraph,
    node: &SearchNode,
    candidate: &EvaluatedLine,
    alternatives: &[&EvaluatedLine],
    enemy_id: ActorIndex,
) -> bool {
    let Some(current_plausibility) = response_plausibility_milli(graph, node, candidate, enemy_id)
    else {
        return false;
    };
    if current_plausibility < MIN_NEAR_BEST_PLAUSIBILITY_MILLI {
        return false;
    }

    let best_plausibility = alternatives
        .iter()
        .filter_map(|alternative| response_plausibility_milli(graph, node, alternative, enemy_id))
        .max()
        .unwrap_or(current_plausibility);

    best_plausibility.saturating_sub(current_plausibility)
        <= OPPONENT_RESPONSE_PLAUSIBILITY_SLACK_MILLI
}

fn response_plausibility_milli(
    graph: &FutureGraph,
    node: &SearchNode,
    line: &EvaluatedLine,
    enemy_id: ActorIndex,
) -> Option<u16> {
    let direction = line.path.first()?.joint_action.direction_for(enemy_id)?;
    let move_set = node.active_analysis()?.tracing.for_actor(enemy_id)?;
    let hypothesis = move_set.hypothesis(direction)?;
    let enemy = node.state.snake_at(enemy_id)?;
    Some(
        graph
            .opponent_profile(&enemy.id)
            .map_or(hypothesis.plausibility_milli, |profile| {
                profile.adjusted_plausibility(hypothesis)
            }),
    )
}

fn unilateral_regret(
    node: &SearchNode,
    candidate: &EvaluatedLine,
    enemies: &[ActorIndex],
    lines: &[EvaluatedLine],
) -> (i64, i64) {
    enemies
        .iter()
        .fold((0_i64, 0_i64), |(worst, total), enemy_id| {
            let current = actor_utility(candidate, *enemy_id);
            let best = lines
                .iter()
                .filter(|alternative| {
                    same_joint_context_except_actor(node, candidate, alternative, *enemy_id)
                })
                .map(|alternative| actor_utility(alternative, *enemy_id))
                .max()
                .unwrap_or(current);
            let regret = best.saturating_sub(current).max(0);
            (worst.max(regret), total.saturating_add(regret))
        })
}

fn same_joint_context_except_actor(
    node: &SearchNode,
    left: &EvaluatedLine,
    right: &EvaluatedLine,
    deviating_actor: ActorIndex,
) -> bool {
    let (Some(left_action), Some(right_action)) = (
        left.path.first().map(|step| &step.joint_action),
        right.path.first().map(|step| &step.joint_action),
    ) else {
        return false;
    };

    node.state
        .snakes
        .iter()
        .enumerate()
        .filter(|(index, snake)| snake.alive && ActorIndex::new(*index) != Some(deviating_actor))
        .all(|(index, _)| {
            let actor = ActorIndex::new(index).expect("simulated actor index must fit");
            left_action.direction_for(actor) == right_action.direction_for(actor)
        })
}

fn actor_utility(line: &EvaluatedLine, actor: ActorIndex) -> i64 {
    line.actor_utility_totals.get(actor).copied().unwrap_or(0)
}

fn rank_and_dedup_variants(lines: &mut Vec<EvaluatedLine>, limit: usize) {
    lines.sort_by(|left, right| compare_our_lines(right, left));
    let mut unique = Vec::with_capacity(lines.len().min(limit));

    for line in lines.drain(..) {
        if unique.iter().any(|existing: &EvaluatedLine| {
            existing.path == line.path && existing.terminal == line.terminal
        }) {
            continue;
        }
        unique.push(line);
        if unique.len() == limit {
            break;
        }
    }

    *lines = unique;
}

fn compare_our_lines(left: &EvaluatedLine, right: &EvaluatedLine) -> std::cmp::Ordering {
    left.our_utility_total
        .cmp(&right.our_utility_total)
        .then_with(|| left.value.cmp(&right.value))
        .then_with(|| left.bound.lower().cmp(&right.bound.lower()))
        .then_with(|| left.bound.upper().cmp(&right.bound.upper()))
}

fn sort_beam_lines(lines: &mut [BeamLine]) {
    lines.sort_by(|left, right| {
        right
            .bound
            .lower()
            .cmp(&left.bound.lower())
            .then_with(|| right.value.cmp(&left.value))
            .then_with(|| left.root_direction.rank().cmp(&right.root_direction.rank()))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn route_value(ours: i64, opponents: i64) -> i64 {
    ours.saturating_sub(opponents)
}

fn incomplete_bound(value: i64) -> ValueBound {
    ValueBound::Interval {
        lower: value.saturating_sub(INCOMPLETE_MARGIN),
        upper: value.saturating_add(INCOMPLETE_MARGIN),
    }
}

fn widen_bound(bound: ValueBound, value: i64) -> ValueBound {
    ValueBound::Interval {
        lower: bound.lower().min(value.saturating_sub(INCOMPLETE_MARGIN)),
        upper: bound.upper().max(value.saturating_add(INCOMPLETE_MARGIN)),
    }
}

fn shift_bound(bound: ValueBound, delta: i64) -> ValueBound {
    match bound {
        ValueBound::Exact(value) => ValueBound::Exact(value.saturating_add(delta)),
        ValueBound::LowerBound(value) => ValueBound::LowerBound(value.saturating_add(delta)),
        ValueBound::UpperBound(value) => ValueBound::UpperBound(value.saturating_add(delta)),
        ValueBound::Interval { lower, upper } => ValueBound::Interval {
            lower: lower.saturating_add(delta),
            upper: upper.saturating_add(delta),
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::search::beam::SEED_DEPTH;
    use crate::simulation::joint_action::JointAction;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
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
        }
    }

    fn multi_enemy_state() -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes: vec![
                snake("ours", &[(2, 2), (2, 1)]),
                snake("enemy-a", &[(5, 2), (5, 1)]),
                snake("enemy-b", &[(5, 5), (5, 4)]),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    fn synthetic_multi_enemy_line(
        ours: i64,
        enemy_a: i64,
        enemy_b: i64,
        child: NodeId,
    ) -> EvaluatedLine {
        let opponents = enemy_a.saturating_add(enemy_b);
        let value = route_value(ours, opponents);
        EvaluatedLine {
            value,
            benefit_total: ours.max(0),
            harm_total: ours.max(0).saturating_sub(ours),
            our_utility_total: ours,
            opponent_utility_total: opponents,
            actor_utility_totals: ActorVec::from_iter([
                (ActorIndex::new(0).unwrap(), ours),
                (ActorIndex::new(1).unwrap(), enemy_a),
                (ActorIndex::new(2).unwrap(), enemy_b),
            ]),
            terminal: LineTerminal::Running,
            certainty: ForecastCertainty::Deterministic,
            bound: ValueBound::Exact(value),
            path: BeamPath::single(BeamStep {
                node: 0,
                joint_action: JointAction::new()
                    .with_move(ActorIndex::new(0).unwrap(), Direction::Up),
                child,
            }),
        }
    }

    fn synthetic_line(ours: i64, opponents: i64, child: NodeId) -> EvaluatedLine {
        let value = route_value(ours, opponents);
        EvaluatedLine {
            value,
            benefit_total: ours.max(0),
            harm_total: ours.max(0).saturating_sub(ours),
            our_utility_total: ours,
            opponent_utility_total: opponents,
            actor_utility_totals: ActorVec::from_iter([
                (ActorIndex::new(0).unwrap(), ours),
                (ActorIndex::new(1).unwrap(), opponents),
            ]),
            terminal: LineTerminal::Running,
            certainty: ForecastCertainty::Deterministic,
            bound: ValueBound::Exact(value),
            path: BeamPath::single(BeamStep {
                node: 0,
                joint_action: JointAction::new()
                    .with_move(ActorIndex::new(0).unwrap(), Direction::Up),
                child,
            }),
        }
    }

    fn synthetic_response_line(
        our_direction: Direction,
        enemy_direction: Direction,
        ours: i64,
        enemy: i64,
        child: NodeId,
    ) -> EvaluatedLine {
        let value = route_value(ours, enemy);
        EvaluatedLine {
            value,
            benefit_total: ours.max(0),
            harm_total: ours.max(0).saturating_sub(ours),
            our_utility_total: ours,
            opponent_utility_total: enemy,
            actor_utility_totals: ActorVec::from_iter([
                (ActorIndex::new(0).unwrap(), ours),
                (ActorIndex::new(1).unwrap(), enemy),
            ]),
            terminal: LineTerminal::Running,
            certainty: ForecastCertainty::Deterministic,
            bound: ValueBound::Exact(value),
            path: BeamPath::single(BeamStep {
                node: 0,
                joint_action: JointAction::new()
                    .with_move(ActorIndex::new(0).unwrap(), our_direction)
                    .with_move(ActorIndex::new(1).unwrap(), enemy_direction),
                child,
            }),
        }
    }

    #[test]
    fn near_best_enemy_response_can_be_selected_when_it_is_worse_for_us() {
        let graph = FutureGraph::new(state());
        let root = graph.node(graph.root());

        let exact_best = synthetic_response_line(Direction::Down, Direction::Right, -501, 57, 1);
        let near_best_trap = synthetic_response_line(Direction::Down, Direction::Up, -556, 48, 2);

        let chosen = select_selfish_opponent_response(
            &graph,
            root,
            vec![exact_best, near_best_trap.clone()],
        );

        assert_eq!(
            chosen
                .path
                .first()
                .unwrap()
                .joint_action
                .direction_for(ActorIndex::new(1).unwrap()),
            Some(Direction::Up)
        );
        assert_eq!(chosen.value, near_best_trap.value);
    }

    #[test]
    fn unsupported_near_best_enemy_response_is_not_promoted_by_slack_alone() {
        let graph = FutureGraph::new(state());
        let root = graph.node(graph.root());

        let supported_best = synthetic_response_line(Direction::Down, Direction::Up, -501, 57, 1);
        let unsupported_near_best =
            synthetic_response_line(Direction::Down, Direction::Right, -10_000, 48, 2);

        let chosen = select_selfish_opponent_response(
            &graph,
            root,
            vec![supported_best.clone(), unsupported_near_best],
        );

        assert_eq!(
            chosen
                .path
                .first()
                .unwrap()
                .joint_action
                .direction_for(ActorIndex::new(1).unwrap()),
            Some(Direction::Up)
        );
        assert_eq!(chosen.value, supported_best.value);
    }

    #[test]
    fn plausible_escape_invalidates_modeled_terminal_win() {
        let graph = FutureGraph::new(state());
        let root = graph.node(graph.root());

        let mut modeled_win =
            synthetic_response_line(Direction::Down, Direction::Right, 10_000, 500, 1);
        modeled_win.terminal = LineTerminal::Won;
        modeled_win.certainty = ForecastCertainty::FoodProvisional;

        let escape = synthetic_response_line(Direction::Down, Direction::Up, -1_000, 0, 2);

        let chosen =
            select_selfish_opponent_response(&graph, root, vec![modeled_win, escape.clone()]);

        assert_eq!(chosen.terminal, LineTerminal::Running);
        assert_eq!(
            chosen
                .path
                .first()
                .unwrap()
                .joint_action
                .direction_for(ActorIndex::new(1).unwrap()),
            Some(Direction::Up)
        );
    }

    #[test]
    fn clearly_irrational_enemy_response_remains_excluded_even_if_worse_for_us() {
        let graph = FutureGraph::new(state());
        let root = graph.node(graph.root());

        let exact_best = synthetic_response_line(Direction::Down, Direction::Right, -501, 57, 1);
        let irrational_attack =
            synthetic_response_line(Direction::Down, Direction::Left, -10_000, -870, 2);

        let chosen = select_selfish_opponent_response(
            &graph,
            root,
            vec![exact_best.clone(), irrational_attack],
        );

        assert_eq!(
            chosen
                .path
                .first()
                .unwrap()
                .joint_action
                .direction_for(ActorIndex::new(1).unwrap()),
            Some(Direction::Right)
        );
        assert_eq!(chosen.value, exact_best.value);
    }

    #[test]
    fn frontier_penalizes_size_disadvantage_once_actor_relatively() {
        let mut disadvantaged = state();
        disadvantaged
            .snakes
            .iter_mut()
            .find(|snake| snake.id == "enemy")
            .unwrap()
            .body
            .push(Coord { x: 4, y: 0 });

        let graph = FutureGraph::new(disadvantaged);
        let line = frontier_line(
            graph.node(graph.root()),
            true,
            ForecastCertainty::Deterministic,
        );

        assert!(line.our_utility_total < line.opponent_utility_total);
        assert!(line.value < 0);
        assert!(line.bound.is_exact());
    }

    #[test]
    fn frontier_is_neutral_when_growth_pressure_is_symmetric() {
        let graph = FutureGraph::new(state());
        let line = frontier_line(
            graph.node(graph.root()),
            true,
            ForecastCertainty::Deterministic,
        );

        assert_eq!(line.value, 0);
        assert_eq!(line.our_utility_total, line.opponent_utility_total);
    }

    #[test]
    fn confirmed_terminal_remains_absolute() {
        let mut terminal = state();
        terminal
            .snakes
            .iter_mut()
            .find(|snake| snake.id == "ours")
            .unwrap()
            .alive = false;
        let graph = FutureGraph::new(terminal);
        let line = terminal_line(graph.node(graph.root()), ForecastCertainty::Deterministic)
            .expect("terminal node must produce a line");

        assert_eq!(line.terminal, LineTerminal::Lost);
        assert_eq!(line.certainty, ForecastCertainty::Deterministic);
        assert_eq!(line.value, -TERMINAL_VALUE);
    }

    #[test]
    fn provisional_terminal_keeps_raw_actor_death_for_response_choice() {
        let enemy = ActorIndex::new(1).unwrap();
        let mut actor_scores = ActorVec::new();
        actor_scores.insert(
            enemy,
            crate::evaluation::ActorTransitionScore {
                actor_terminal: -TERMINAL_VALUE,
                actor_choice_net: -TERMINAL_VALUE,
                ..crate::evaluation::ActorTransitionScore::default()
            },
        );
        let transition = TransitionScore {
            actors: actor_scores,
            ..TransitionScore::default()
        };
        let edge = SearchEdge {
            joint_action: JointAction::new(),
            transition: transition.clone(),
            forecast_delta: super::super::forecast::ForecastDelta::FoodUncertainty,
            child: 0,
        };
        let line = EvaluatedLine {
            value: PROVISIONAL_TERMINAL_VALUE,
            benefit_total: PROVISIONAL_TERMINAL_VALUE,
            harm_total: 0,
            our_utility_total: PROVISIONAL_TERMINAL_VALUE,
            opponent_utility_total: 0,
            actor_utility_totals: ActorVec::new(),
            terminal: LineTerminal::Won,
            certainty: ForecastCertainty::FoodProvisional,
            bound: ValueBound::Exact(PROVISIONAL_TERMINAL_VALUE),
            path: BeamPath::empty(),
        };

        let shifted = line.shifted_by_edge(0, &edge, &transition);

        assert_eq!(
            shifted.actor_utility_totals.get(enemy).copied(),
            Some(-TERMINAL_VALUE)
        );
        assert_eq!(shifted.value, PROVISIONAL_TERMINAL_VALUE);
    }

    #[test]
    fn provisional_terminal_is_bounded_instead_of_absolute() {
        let mut terminal = state();
        terminal
            .snakes
            .iter_mut()
            .find(|snake| snake.id == "ours")
            .unwrap()
            .alive = false;
        let graph = FutureGraph::new(terminal);
        let line = terminal_line(graph.node(graph.root()), ForecastCertainty::FoodProvisional)
            .expect("terminal node must produce a line");

        assert_eq!(line.terminal, LineTerminal::Lost);
        assert_eq!(line.certainty, ForecastCertainty::FoodProvisional);
        assert_eq!(line.value, -PROVISIONAL_TERMINAL_VALUE);
        assert!(line.value > -TERMINAL_VALUE);
    }

    #[test]
    fn root_candidates_keep_terminal_loss_dominated() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let result = evaluate_seed_lines(&graph, 1);
        let losses = result
            .lines
            .iter()
            .filter(|line| line.terminal == LineTerminal::Lost)
            .collect::<Vec<_>>();

        assert!(losses.iter().all(|line| line.value <= -TERMINAL_VALUE));
    }

    #[test]
    fn depth_three_seed_retains_three_concrete_steps_for_running_lines() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(SEED_DEPTH).unwrap();

        let result = evaluate_seed_lines(&graph, SEED_DEPTH);
        let running = result
            .lines
            .iter()
            .filter(|line| line.terminal == LineTerminal::Running)
            .collect::<Vec<_>>();

        assert!(!running.is_empty());
        assert!(running.iter().all(|line| line.depth == SEED_DEPTH));
        assert!(running
            .iter()
            .all(|line| line.path.len() == usize::from(SEED_DEPTH)));
        assert!(running.iter().all(|line| line.bound.is_exact()));
    }

    #[test]
    fn seed_beam_targets_two_root_directions_when_available() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(SEED_DEPTH).unwrap();

        let candidates = evaluate_seed_lines(&graph, SEED_DEPTH);
        let beam = evaluate_seed_beam(&graph, SEED_DEPTH);

        let mut candidate_directions = candidates
            .lines
            .iter()
            .filter(|line| line.is_viable())
            .map(|line| line.root_direction.rank())
            .collect::<Vec<_>>();
        candidate_directions.sort_unstable();
        candidate_directions.dedup();

        let mut beam_directions = beam
            .lines
            .iter()
            .map(|line| line.root_direction.rank())
            .collect::<Vec<_>>();
        beam_directions.sort_unstable();
        beam_directions.dedup();

        if candidate_directions.len() >= 2 {
            assert!(beam_directions.len() >= 2);
        } else {
            assert_eq!(beam.lines.len(), 1);
        }
        assert!(beam.lines.len() <= 3);
    }

    #[test]
    fn continuation_evaluation_starts_at_requested_node() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();
        let tip = graph.node(graph.root()).children[0].child;
        graph.expand_to_depth(3).unwrap();

        let continuations =
            evaluate_continuations(&graph, tip, 2, ForecastCertainty::Deterministic);
        let best = continuations.first().expect("continuation must exist");

        assert!(best.terminal != LineTerminal::Running || best.depth == 2);
        assert!(best.bound.is_exact());
        assert!(best.path.first().is_none_or(|step| step.node == tip));
    }

    #[test]
    fn incomplete_node_is_never_reported_as_exact() {
        let graph = FutureGraph::new(state());
        let mut evaluator = MaximinEvaluator {
            graph: &graph,
            memo: HashMap::new(),
            stats: MaximinStats::default(),
        };

        let line = evaluator.evaluate_node(graph.root(), 2, ForecastCertainty::Deterministic);

        assert!(!line.bound.is_exact());
        assert_eq!(line.terminal, LineTerminal::Running);
    }

    #[test]
    fn our_future_choice_maximizes_our_own_utility() {
        let chosen = max_our_choices(vec![synthetic_line(9, 100, 1), synthetic_line(5, 0, 2)]);

        assert_eq!(chosen.our_utility_total, 9);
        assert_eq!(chosen.opponent_utility_total, 100);
    }

    #[test]
    fn opponent_selects_its_own_best_route_not_the_route_that_hurts_us_most() {
        let hurts_us_more = synthetic_line(-500, 20, 1);
        let benefits_enemy_more = synthetic_line(-50, 200, 2);

        let graph = FutureGraph::new(state());
        let node = graph.node(graph.root());
        let chosen = select_selfish_opponent_response(
            &graph,
            node,
            vec![hurts_us_more, benefits_enemy_more],
        );

        assert_eq!(chosen.opponent_utility_total, 200);
        assert_eq!(chosen.our_utility_total, -50);
    }

    fn synthetic_joint_line(
        ours: i64,
        enemy_a: i64,
        enemy_b: i64,
        enemy_a_move: Direction,
        enemy_b_move: Direction,
        child: NodeId,
    ) -> EvaluatedLine {
        let mut line = synthetic_multi_enemy_line(ours, enemy_a, enemy_b, child);
        line.path = BeamPath::single(BeamStep {
            node: 0,
            joint_action: JointAction::new()
                .with_move(ActorIndex::new(0).unwrap(), Direction::Up)
                .with_move(ActorIndex::new(1).unwrap(), enemy_a_move)
                .with_move(ActorIndex::new(2).unwrap(), enemy_b_move),
            child,
        });
        line
    }

    #[test]
    fn pure_joint_best_response_is_preferred_when_it_exists() {
        let graph = FutureGraph::new(multi_enemy_state());
        let node = graph.node(graph.root());

        let equilibrium = synthetic_joint_line(0, 100, 100, Direction::Up, Direction::Up, 1);
        let a_deviation = synthetic_joint_line(0, 80, 120, Direction::Down, Direction::Up, 2);
        let b_deviation = synthetic_joint_line(0, 120, 80, Direction::Up, Direction::Down, 3);
        let unrelated = synthetic_joint_line(0, 70, 70, Direction::Down, Direction::Down, 4);

        let chosen = select_selfish_opponent_response(
            &graph,
            node,
            vec![equilibrium, a_deviation, b_deviation, unrelated],
        );

        assert_eq!(
            chosen
                .path
                .first()
                .unwrap()
                .joint_action
                .direction_for(ActorIndex::new(1).unwrap()),
            Some(Direction::Up)
        );
        assert_eq!(
            chosen
                .path
                .first()
                .unwrap()
                .joint_action
                .direction_for(ActorIndex::new(2).unwrap()),
            Some(Direction::Up)
        );
    }

    #[test]
    fn multiple_opponents_are_not_collapsed_to_team_sum_during_prediction() {
        let graph = FutureGraph::new(multi_enemy_state());
        let node = graph.node(graph.root());

        let enemy_a_extreme = synthetic_multi_enemy_line(0, 300, 0, 1);
        let enemy_b_extreme = synthetic_multi_enemy_line(0, 0, 300, 2);
        let individually_balanced = synthetic_multi_enemy_line(0, 140, 140, 3);

        let chosen = select_selfish_opponent_response(
            &graph,
            node,
            vec![enemy_a_extreme, enemy_b_extreme, individually_balanced],
        );

        assert_eq!(chosen.opponent_utility_total, 280);
        assert_eq!(actor_utility(&chosen, ActorIndex::new(1).unwrap()), 140);
        assert_eq!(actor_utility(&chosen, ActorIndex::new(2).unwrap()), 140);
    }

    #[test]
    fn final_route_value_is_ours_minus_opponents() {
        let line = synthetic_line(420, 180, 1);

        assert_eq!(line.value, 240);
    }

    #[test]
    fn variant_ranking_keeps_distinct_lines_independent() {
        let mut lines = vec![
            synthetic_line(700, 0, 1),
            synthetic_line(600, 0, 2),
            synthetic_line(800, 0, 3),
            synthetic_line(700, 0, 1),
        ];

        rank_and_dedup_variants(&mut lines, 3);

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].our_utility_total, 800);
        assert_eq!(lines[1].our_utility_total, 700);
        assert_eq!(lines[2].our_utility_total, 600);
        assert_ne!(lines[1].path, lines[2].path);
    }

    #[test]
    fn depth_three_emits_one_rational_policy_per_root_direction() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(SEED_DEPTH).unwrap();

        let result = evaluate_seed_lines(&graph, SEED_DEPTH);
        let mut counts = [0_u8; 4];
        for line in &result.lines {
            counts[usize::from(line.root_direction.rank())] =
                counts[usize::from(line.root_direction.rank())].saturating_add(1);
        }

        assert!(!result.lines.is_empty());
        assert!(counts.into_iter().all(|count| count <= 1));
    }
}
