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
        let instant_net = transition
            .instant_benefit
            .saturating_sub(transition.instant_harm);

        if self.terminal == LineTerminal::Running {
            self.value = self.value.saturating_add(instant_net);
            self.bound = shift_bound(self.bound, instant_net);
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
            .unwrap_or_else(|| frontier_line(self.graph.node(node_id), false))
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
            vec![frontier_line(node, true)]
        } else if !node.expansion_complete() {
            self.stats.bounded_nodes = self.stats.bounded_nodes.saturating_add(1);
            vec![frontier_line(node, false)]
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
                vec![frontier_line(node, false)]
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
            let transition = TransitionScore::from_edge(node, edge, child);
            let variants = self
                .evaluate_node_variants(edge.child, remaining_depth.saturating_sub(1))
                .into_iter()
                .map(|line| line.shifted_by_edge(node_id, edge, transition))
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
        policies.push(min_opponent_responses(
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
                    policies.push(min_opponent_responses(selected));
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

fn frontier_line(node: &SearchNode, exact: bool) -> EvaluatedLine {
    let actor_id = &node.state.our_snake_id;
    let evaluation = node
        .active_analysis()
        .and_then(|analysis| analysis.actor_evaluations.get(actor_id));

    let (value, benefit_total, harm_total) = evaluation.map_or((0, 0, 0), |evaluation| {
        (
            evaluation.net,
            evaluation.benefit_total,
            evaluation.harm_total,
        )
    });

    EvaluatedLine {
        value,
        benefit_total,
        harm_total,
        terminal: LineTerminal::Running,
        bound: if exact {
            ValueBound::Exact(value)
        } else {
            incomplete_bound(value)
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

fn min_opponent_responses(lines: Vec<EvaluatedLine>) -> EvaluatedLine {
    let lower = lines
        .iter()
        .map(|line| line.bound.lower())
        .min()
        .unwrap_or(i64::MIN);
    let upper = lines
        .iter()
        .map(|line| line.bound.upper())
        .min()
        .unwrap_or(i64::MAX);
    let all_exact = lines.iter().all(|line| line.bound.is_exact());

    let mut chosen = lines
        .into_iter()
        .min_by(|left, right| {
            left.bound
                .lower()
                .cmp(&right.bound.lower())
                .then_with(|| left.value.cmp(&right.value))
        })
        .expect("MIN requires at least one line");

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
    left.bound
        .lower()
        .cmp(&right.bound.lower())
        .then_with(|| left.value.cmp(&right.value))
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

    fn synthetic_line(value: i64, child: NodeId) -> EvaluatedLine {
        EvaluatedLine {
            value,
            benefit_total: value.max(0),
            harm_total: value.max(0).saturating_sub(value),
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
    fn opponent_min_selects_a_lethal_response_when_one_exists() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let result = evaluate_seed_lines(&graph, 1);
        let right = result
            .lines
            .iter()
            .filter(|line| line.root_direction == Direction::Right)
            .min_by_key(|line| line.value)
            .expect("right must be evaluated");

        assert_eq!(right.terminal, LineTerminal::Lost);
        assert_eq!(right.value, -TERMINAL_VALUE);
        assert!(right.bound.is_exact());
        assert_eq!(right.steps.len(), 1);
    }

    #[test]
    fn root_candidates_keep_safe_choices_above_refuted_direction() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let result = evaluate_seed_lines(&graph, 1);
        let best = result.lines.first().expect("at least one line");
        let worst_right = result
            .lines
            .iter()
            .filter(|line| line.root_direction == Direction::Right)
            .map(|line| line.value)
            .min()
            .unwrap();

        assert!(best.value > worst_right);
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
    fn seed_beam_selects_three_lines_with_root_diversity() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(SEED_DEPTH).unwrap();

        let candidates = evaluate_seed_lines(&graph, SEED_DEPTH);
        let beam = evaluate_seed_beam(&graph, SEED_DEPTH);

        assert!(candidates.lines.len() >= 3);
        assert_eq!(beam.lines.len(), 3);

        let mut root_directions = beam
            .lines
            .iter()
            .map(|line| line.root_direction.rank())
            .collect::<Vec<_>>();
        root_directions.sort_unstable();
        root_directions.dedup();

        let candidate_direction_count = {
            let mut directions = candidates
                .lines
                .iter()
                .filter(|line| line.is_viable())
                .map(|line| line.root_direction.rank())
                .collect::<Vec<_>>();
            directions.sort_unstable();
            directions.dedup();
            directions.len()
        };

        if candidate_direction_count >= 2 {
            assert!(root_directions.len() >= 2);
        }
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
    fn max_and_min_follow_game_tree_operators() {
        assert_eq!(
            max_our_choices(vec![synthetic_line(5, 1), synthetic_line(9, 2)]).value,
            9
        );
        assert_eq!(
            min_opponent_responses(vec![synthetic_line(5, 1), synthetic_line(9, 2)]).value,
            5
        );
    }

    #[test]
    fn variant_ranking_keeps_distinct_lines_independent() {
        let mut lines = vec![
            synthetic_line(700, 1),
            synthetic_line(600, 2),
            synthetic_line(800, 3),
            synthetic_line(700, 1),
        ];

        rank_and_dedup_variants(&mut lines, 3);

        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].value, 800);
        assert_eq!(lines[1].value, 700);
        assert_eq!(lines[2].value, 600);
        assert_ne!(lines[1].steps, lines[2].steps);
    }

    #[test]
    fn depth_three_can_emit_multiple_variants_for_one_root_direction() {
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
