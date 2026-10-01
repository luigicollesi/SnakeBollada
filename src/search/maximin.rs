#![allow(dead_code)]

use std::collections::HashMap;

use crate::direction::Direction;
use crate::evaluation::TransitionScore;

use super::beam::{select_seed_beam, BeamLine, BeamStep, LineId, LineTerminal};
use super::bounds::ValueBound;
use super::graph::{FutureGraph, NodeId, SearchEdge, SearchNode};

const TERMINAL_VALUE: i64 = 1_000_000_000;
const INCOMPLETE_MARGIN: i64 = 20_000;
const MAX_VARIANTS_PER_NODE: usize = 3;

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
    pub(crate) value: i64,
    pub(crate) terminal: LineTerminal,
    pub(crate) bound: ValueBound,
    pub(crate) steps: Vec<BeamStep>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EvaluatedLine {
    value: i64,
    benefit_total: i64,
    harm_total: i64,
    our_utility_total: i64,
    opponent_utility_total: i64,
    terminal: LineTerminal,
    bound: ValueBound,
    steps: Vec<BeamStep>,
}

impl EvaluatedLine {
    fn shifted_by_edge(
        mut self,
        node: NodeId,
        edge: &SearchEdge,
        transition: TransitionScore,
    ) -> Self {
        if self.terminal == LineTerminal::Running {
            self.our_utility_total = self.our_utility_total.saturating_add(transition.net);
            self.opponent_utility_total = self
                .opponent_utility_total
                .saturating_add(transition.opponent_net_total);
            self.value = route_value(self.our_utility_total, self.opponent_utility_total);
            self.bound = shift_bound(self.bound, transition.route_delta());
        }

        self.benefit_total = self
            .benefit_total
            .saturating_add(transition.instant_benefit);
        self.harm_total = self.harm_total.saturating_add(transition.instant_harm);
        self.steps.insert(
            0,
            BeamStep {
                node,
                joint_action: edge.joint_action.clone(),
                child: edge.child,
                transition,
            },
        );
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    node: NodeId,
    remaining_depth: u8,
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
        let variants = evaluator.evaluate_direction_variants(root, direction, target_depth);
        for line in variants {
            let depth = line.steps.len().try_into().unwrap_or(u8::MAX);
            lines.push(BeamLine {
                id: LineId(next_id),
                root_direction: direction,
                depth,
                benefit_total: line.benefit_total,
                harm_total: line.harm_total,
                our_utility_total: line.our_utility_total,
                opponent_utility_total: line.opponent_utility_total,
                value: line.value,
                terminal: line.terminal,
                bound: line.bound,
                steps: line.steps,
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
) -> Vec<ContinuationEvaluation> {
    let mut evaluator = MaximinEvaluator {
        graph,
        memo: HashMap::new(),
        stats: MaximinStats::default(),
    };

    evaluator
        .evaluate_node_variants(start_node, target_depth)
        .into_iter()
        .map(|line| ContinuationEvaluation {
            depth: line.steps.len().try_into().unwrap_or(u8::MAX),
            benefit_total: line.benefit_total,
            harm_total: line.harm_total,
            our_utility_total: line.our_utility_total,
            opponent_utility_total: line.opponent_utility_total,
            value: line.value,
            terminal: line.terminal,
            bound: line.bound,
            steps: line.steps,
        })
        .collect()
}

impl MaximinEvaluator<'_> {
    fn evaluate_node(&mut self, node_id: NodeId, remaining_depth: u8) -> EvaluatedLine {
        self.evaluate_node_variants(node_id, remaining_depth)
            .into_iter()
            .next()
            .unwrap_or_else(|| frontier_line(false))
    }

    fn evaluate_node_variants(
        &mut self,
        node_id: NodeId,
        remaining_depth: u8,
    ) -> Vec<EvaluatedLine> {
        let key = MemoKey {
            node: node_id,
            remaining_depth,
        };
        if let Some(cached) = self.memo.get(&key) {
            self.stats.memo_hits = self.stats.memo_hits.saturating_add(1);
            return cached.clone();
        }

        self.stats.nodes_evaluated = self.stats.nodes_evaluated.saturating_add(1);
        let node = self.graph.node(node_id);

        let mut result = if let Some(terminal) = terminal_line(node) {
            self.stats.exact_nodes = self.stats.exact_nodes.saturating_add(1);
            vec![terminal]
        } else if remaining_depth == 0 {
            self.stats.exact_nodes = self.stats.exact_nodes.saturating_add(1);
            vec![frontier_line(true)]
        } else if !node.expansion_complete() {
            self.stats.bounded_nodes = self.stats.bounded_nodes.saturating_add(1);
            vec![frontier_line(false)]
        } else {
            let mut variants = Vec::new();
            for direction in Direction::ALL {
                variants.extend(self.evaluate_direction_variants(
                    node_id,
                    direction,
                    remaining_depth,
                ));
            }

            if variants.is_empty() {
                self.stats.bounded_nodes = self.stats.bounded_nodes.saturating_add(1);
                vec![frontier_line(false)]
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
    ) -> Vec<EvaluatedLine> {
        let node = self.graph.node(node_id);
        let edges = node
            .children
            .iter()
            .filter(|edge| {
                edge.joint_action.direction_for(&node.state.our_snake_id) == Some(direction)
            })
            .collect::<Vec<_>>();

        if edges.is_empty() {
            return Vec::new();
        }

        let mut edge_variants = Vec::with_capacity(edges.len());
        for edge in edges {
            let child = self.graph.node(edge.child);
            let transition = edge.transition.clone();
            let variants = self
                .evaluate_node_variants(edge.child, remaining_depth.saturating_sub(1))
                .into_iter()
                .map(|line| line.shifted_by_edge(node_id, edge, transition.clone()))
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
        policies.push(max_opponent_self_utility(
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
                    policies.push(max_opponent_self_utility(selected));
                }
            }
        }

        if !node.expansion_complete() {
            for policy in &mut policies {
                policy.bound = widen_bound(policy.bound, policy.value);
            }
        }

        rank_and_dedup_variants(&mut policies, MAX_VARIANTS_PER_NODE);
        policies
    }
}

fn frontier_line(exact: bool) -> EvaluatedLine {
    EvaluatedLine {
        value: 0,
        benefit_total: 0,
        harm_total: 0,
        our_utility_total: 0,
        opponent_utility_total: 0,
        terminal: LineTerminal::Running,
        bound: if exact {
            ValueBound::Exact(0)
        } else {
            incomplete_bound(0)
        },
        steps: Vec::new(),
    }
}

fn terminal_line(node: &SearchNode) -> Option<EvaluatedLine> {
    let ours_alive = node
        .state
        .snake(&node.state.our_snake_id)
        .is_some_and(|snake| snake.alive);
    if !ours_alive {
        return Some(EvaluatedLine {
            value: -TERMINAL_VALUE,
            benefit_total: 0,
            harm_total: TERMINAL_VALUE,
            our_utility_total: -TERMINAL_VALUE,
            opponent_utility_total: 0,
            terminal: LineTerminal::Lost,
            bound: ValueBound::Exact(-TERMINAL_VALUE),
            steps: Vec::new(),
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

    Some(EvaluatedLine {
        value: TERMINAL_VALUE,
        benefit_total: TERMINAL_VALUE,
        harm_total: 0,
        our_utility_total: TERMINAL_VALUE,
        opponent_utility_total: 0,
        terminal: LineTerminal::Won,
        bound: ValueBound::Exact(TERMINAL_VALUE),
        steps: Vec::new(),
    })
}

fn max_our_choices(lines: Vec<EvaluatedLine>) -> EvaluatedLine {
    lines
        .into_iter()
        .max_by(compare_our_lines)
        .expect("MAX requires at least one line")
}

fn max_opponent_self_utility(lines: Vec<EvaluatedLine>) -> EvaluatedLine {
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

    let mut chosen = lines
        .into_iter()
        .max_by(|left, right| {
            left.opponent_utility_total
                .cmp(&right.opponent_utility_total)
                .then_with(|| right.value.cmp(&left.value))
                .then_with(|| left.our_utility_total.cmp(&right.our_utility_total))
        })
        .expect("opponent response selection requires at least one line");

    chosen.bound = if all_exact {
        ValueBound::Exact(chosen.value)
    } else {
        ValueBound::Interval { lower, upper }
    };
    chosen
}

fn rank_and_dedup_variants(lines: &mut Vec<EvaluatedLine>, limit: usize) {
    lines.sort_by(|left, right| compare_our_lines(right, left));
    let mut unique = Vec::with_capacity(lines.len().min(limit));

    for line in lines.drain(..) {
        if unique.iter().any(|existing: &EvaluatedLine| {
            existing.steps == line.steps && existing.terminal == line.terminal
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

    fn synthetic_line(ours: i64, opponents: i64, child: NodeId) -> EvaluatedLine {
        let value = route_value(ours, opponents);
        EvaluatedLine {
            value,
            benefit_total: ours.max(0),
            harm_total: ours.max(0).saturating_sub(ours),
            our_utility_total: ours,
            opponent_utility_total: opponents,
            terminal: LineTerminal::Running,
            bound: ValueBound::Exact(value),
            steps: vec![BeamStep {
                node: 0,
                joint_action: JointAction::new().with_move("ours", Direction::Up),
                child,
                transition: TransitionScore::default(),
            }],
        }
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
            .all(|line| line.steps.len() == usize::from(SEED_DEPTH)));
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

        let continuations = evaluate_continuations(&graph, tip, 2);
        let best = continuations.first().expect("continuation must exist");

        assert!(best.terminal != LineTerminal::Running || best.depth == 2);
        assert!(best.bound.is_exact());
        assert!(best.steps.first().is_none_or(|step| step.node == tip));
    }

    #[test]
    fn incomplete_node_is_never_reported_as_exact() {
        let graph = FutureGraph::new(state());
        let mut evaluator = MaximinEvaluator {
            graph: &graph,
            memo: HashMap::new(),
            stats: MaximinStats::default(),
        };

        let line = evaluator.evaluate_node(graph.root(), 2);

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

        let chosen = max_opponent_self_utility(vec![hurts_us_more, benefits_enemy_more]);

        assert_eq!(chosen.opponent_utility_total, 200);
        assert_eq!(chosen.our_utility_total, -50);
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
        assert_ne!(lines[1].steps, lines[2].steps);
    }

    #[test]
    fn depth_three_can_emit_multiple_variants_for_one_root_direction_before_beam_selection() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(SEED_DEPTH).unwrap();

        let result = evaluate_seed_lines(&graph, SEED_DEPTH);
        let mut counts = [0_u8; 4];
        for line in &result.lines {
            counts[usize::from(line.root_direction.rank())] =
                counts[usize::from(line.root_direction.rank())].saturating_add(1);
        }

        assert!(counts.into_iter().any(|count| count >= 2));
    }
}
