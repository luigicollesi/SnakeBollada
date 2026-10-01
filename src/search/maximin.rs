#![allow(dead_code)]

use std::collections::HashMap;

use crate::direction::Direction;
use crate::evaluation::TransitionScore;

use super::beam::{BeamLine, BeamStep, LineTerminal};
use super::bounds::ValueBound;
use super::graph::{FutureGraph, NodeId, SearchEdge, SearchNode};

const TERMINAL_VALUE: i64 = 1_000_000_000;
const INCOMPLETE_MARGIN: i64 = 20_000;

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

#[derive(Debug, Clone)]
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

        self.value = self.value.saturating_add(instant_net);
        self.benefit_total = self
            .benefit_total
            .saturating_add(transition.instant_benefit);
        self.harm_total = self.harm_total.saturating_add(transition.instant_harm);
        self.bound = shift_bound(self.bound, instant_net);
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
    memo: HashMap<MemoKey, EvaluatedLine>,
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

    if target_depth == 0 || node.is_terminal() {
        return SeedEvaluation {
            lines,
            stats: evaluator.stats,
        };
    }

    for direction in Direction::ALL {
        let Some(line) = evaluator.evaluate_direction(root, direction, target_depth) else {
            continue;
        };

        let depth = line.steps.len().try_into().unwrap_or(u8::MAX);
        lines.push(BeamLine::from_search(
            u32::from(direction.rank()).saturating_add(1),
            direction,
            depth,
            line.benefit_total,
            line.harm_total,
            line.value,
            line.terminal,
            line.bound,
            line.steps,
        ));
    }

    lines.sort_by(|left, right| {
        right
            .bound
            .lower()
            .cmp(&left.bound.lower())
            .then_with(|| right.value.cmp(&left.value))
            .then_with(|| left.root_direction.rank().cmp(&right.root_direction.rank()))
    });

    SeedEvaluation {
        lines,
        stats: evaluator.stats,
    }
}

impl MaximinEvaluator<'_> {
    fn evaluate_node(&mut self, node_id: NodeId, remaining_depth: u8) -> EvaluatedLine {
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

        let result = if let Some(terminal) = terminal_line(node) {
            self.stats.exact_nodes = self.stats.exact_nodes.saturating_add(1);
            terminal
        } else if remaining_depth == 0 {
            self.stats.exact_nodes = self.stats.exact_nodes.saturating_add(1);
            frontier_line(node, true)
        } else if !node.expansion_complete() {
            self.stats.bounded_nodes = self.stats.bounded_nodes.saturating_add(1);
            frontier_line(node, false)
        } else {
            let mut directions = Vec::new();
            for direction in Direction::ALL {
                if let Some(line) = self.evaluate_direction(node_id, direction, remaining_depth) {
                    directions.push(line);
                }
            }

            if directions.is_empty() {
                self.stats.bounded_nodes = self.stats.bounded_nodes.saturating_add(1);
                frontier_line(node, false)
            } else {
                max_our_choices(directions)
            }
        };

        self.memo.insert(key, result.clone());
        result
    }

    fn evaluate_direction(
        &mut self,
        node_id: NodeId,
        direction: Direction,
        remaining_depth: u8,
    ) -> Option<EvaluatedLine> {
        let node = self.graph.node(node_id);
        let edges = node
            .children
            .iter()
            .filter(|edge| {
                edge.joint_action.direction_for(&node.state.our_snake_id) == Some(direction)
            })
            .collect::<Vec<_>>();

        if edges.is_empty() {
            return None;
        }

        let mut outcomes = Vec::with_capacity(edges.len());
        for edge in edges {
            let child = self.graph.node(edge.child);
            let transition = TransitionScore::from_edge(node, edge, child);
            let child_line = self.evaluate_node(edge.child, remaining_depth.saturating_sub(1));
            outcomes.push(child_line.shifted_by_edge(node_id, edge, transition));
        }

        let mut result = min_opponent_responses(outcomes);
        if !node.expansion_complete() {
            result.bound = widen_bound(result.bound, result.value);
        }
        Some(result)
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
    let lower = lines
        .iter()
        .map(|line| line.bound.lower())
        .max()
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
            left.bound
                .lower()
                .cmp(&right.bound.lower())
                .then_with(|| left.value.cmp(&right.value))
        })
        .expect("MAX requires at least one line");

    chosen.bound = if all_exact {
        ValueBound::Exact(chosen.value)
    } else {
        ValueBound::Interval { lower, upper }
    };
    chosen
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
    fn opponent_min_selects_a_lethal_response_when_one_exists() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let result = evaluate_seed_lines(&graph, 1);
        let right = result
            .lines
            .iter()
            .find(|line| line.root_direction == Direction::Right)
            .expect("right must be evaluated");

        assert_eq!(right.terminal, LineTerminal::Lost);
        assert_eq!(right.value, -TERMINAL_VALUE);
        assert!(right.bound.is_exact());
        assert_eq!(right.steps.len(), 1);
    }

    #[test]
    fn root_max_keeps_safe_choices_above_refuted_direction() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let result = evaluate_seed_lines(&graph, 1);
        let best = result.lines.first().expect("at least one line");
        let right = result
            .lines
            .iter()
            .find(|line| line.root_direction == Direction::Right)
            .unwrap();

        assert!(best.value > right.value);
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
    fn max_and_min_bounds_follow_game_tree_operators() {
        let exact = |value| EvaluatedLine {
            value,
            benefit_total: value.max(0),
            harm_total: value.max(0).saturating_sub(value),
            terminal: LineTerminal::Running,
            bound: ValueBound::Exact(value),
            steps: Vec::new(),
        };

        assert_eq!(max_our_choices(vec![exact(5), exact(9)]).value, 9);
        assert_eq!(min_opponent_responses(vec![exact(5), exact(9)]).value, 5);
    }
}
