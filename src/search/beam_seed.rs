#![allow(dead_code)]

use std::time::Instant;

use super::beam::{
    select_seed_beam, BeamCheckpoint, BeamLine, LineId, LineTerminal, BEAM_WIDTH, SEED_DEPTH,
};
use super::beam_round::append_continuation;
use super::budget::SearchBudget;
use super::graph::{FutureGraph, NodeId, SearchError};
use super::maximin::{evaluate_continuations, evaluate_frontier, evaluate_seed_lines};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BeamSeedStats {
    pub(crate) completed: bool,
    pub(crate) completed_depth: u8,
    pub(crate) line_count: u8,
    pub(crate) new_nodes: u32,
    pub(crate) new_edges: u32,
    pub(crate) elapsed_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BeamSeedResult {
    pub(crate) checkpoint: BeamCheckpoint,
    pub(crate) stats: BeamSeedStats,
}

pub(crate) fn build_seed_checkpoint(
    graph: &mut FutureGraph,
    budget: &SearchBudget,
) -> Result<Option<BeamSeedResult>, SearchError> {
    let started = Instant::now();
    let nodes_before = graph.node_count();
    let edges_before = graph.edge_count();
    if budget.expired() {
        return Ok(None);
    }

    let first_expansion = graph.expand_depth(1, budget)?;
    if !first_expansion.completed {
        return Ok(None);
    }

    let first_evaluation = evaluate_seed_lines(graph, 1);
    let first_selected = select_seed_beam(&first_evaluation.lines);
    log::debug!(
        target: "search_diagnostics",
        "seed turn={} depth=1 raw={} selected={}",
        graph.node(graph.root()).state.turn,
        format_lines(&first_evaluation.lines),
        format_lines(&first_selected)
    );
    if first_selected.is_empty() || first_selected.len() > BEAM_WIDTH {
        return Ok(None);
    }

    let Some(mut checkpoint) = BeamCheckpoint::new(first_selected) else {
        return Ok(None);
    };
    let mut completed_depth = 1_u8;

    if has_single_root_direction(&checkpoint.lines) {
        return Ok(Some(seed_result(
            graph,
            checkpoint,
            completed_depth,
            nodes_before,
            edges_before,
            started,
        )));
    }

    let mut next_line_id = checkpoint
        .lines
        .iter()
        .map(|line| line.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

    checkpoint = deepen_seed_while_affordable(graph, checkpoint, budget, &mut next_line_id)?;
    completed_depth = checkpoint.completed_depth;

    let valid = checkpoint.lines.iter().all(|line| {
        line.bound.is_exact()
            && (line.terminal != LineTerminal::Running || line.depth == completed_depth)
    });
    if !valid {
        return Ok(None);
    }

    Ok(Some(seed_result(
        graph,
        checkpoint,
        completed_depth,
        nodes_before,
        edges_before,
        started,
    )))
}

fn deepen_seed_while_affordable(
    graph: &mut FutureGraph,
    mut checkpoint: BeamCheckpoint,
    budget: &SearchBudget,
    next_line_id: &mut u32,
) -> Result<BeamCheckpoint, SearchError> {
    while checkpoint.completed_depth < SEED_DEPTH && !has_single_root_direction(&checkpoint.lines) {
        if budget.expired() {
            break;
        }

        let Some(next_checkpoint) =
            deepen_seed_one_layer(graph, &checkpoint, budget, next_line_id)?
        else {
            break;
        };

        checkpoint = next_checkpoint;
        log::debug!(
            target: "search_diagnostics",
            "seed turn={} depth={} selected={}",
            graph.node(graph.root()).state.turn,
            checkpoint.completed_depth,
            format_lines(&checkpoint.lines)
        );
    }

    Ok(checkpoint)
}

fn format_lines(lines: &[BeamLine]) -> String {
    lines
        .iter()
        .map(|line| {
            format!(
                "{:?}:v={} ours={} opp={} depth={} term={:?} cert={:?}",
                line.root_direction,
                line.value,
                line.our_utility_total,
                line.opponent_utility_total,
                line.depth,
                line.terminal,
                line.certainty
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn deepen_seed_one_layer(
    graph: &mut FutureGraph,
    checkpoint: &BeamCheckpoint,
    budget: &SearchBudget,
    next_line_id: &mut u32,
) -> Result<Option<BeamCheckpoint>, SearchError> {
    let mut tips = Vec::<NodeId>::new();
    for line in &checkpoint.lines {
        if line.terminal != LineTerminal::Running {
            continue;
        }
        let Some(tip) = line.path.last().map(|step| step.child) else {
            return Ok(None);
        };
        if !tips.contains(&tip) {
            tips.push(tip);
        }
    }

    for tip in tips {
        let expansion = graph.expand_prioritized_subtree(tip, 1, budget)?;
        if !expansion.completed {
            return Ok(None);
        }
    }

    let mut candidates = Vec::<BeamLine>::new();
    for line in &checkpoint.lines {
        if line.terminal != LineTerminal::Running {
            candidates.push(line.clone());
            continue;
        }

        let Some(tip) = line.path.last().map(|step| step.child) else {
            return Ok(None);
        };
        let previous_frontier = evaluate_frontier(graph, tip, line.certainty);
        let continuations = evaluate_continuations(graph, tip, 1, line.certainty);
        for continuation in continuations
            .into_iter()
            .filter(|continuation| continuation.bound.is_exact())
        {
            let mut candidate = append_continuation(line, &previous_frontier, continuation);
            candidate.id = LineId(*next_line_id);
            *next_line_id = next_line_id.saturating_add(1);
            candidates.push(candidate);
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = select_seed_beam(&candidates);
    if selected.is_empty() || selected.len() > BEAM_WIDTH {
        return Ok(None);
    }

    Ok(BeamCheckpoint::new(selected))
}

fn has_single_root_direction(lines: &[BeamLine]) -> bool {
    let Some(first) = lines.first() else {
        return false;
    };
    lines
        .iter()
        .all(|line| line.root_direction == first.root_direction)
}

fn seed_result(
    graph: &FutureGraph,
    checkpoint: BeamCheckpoint,
    completed_depth: u8,
    nodes_before: usize,
    edges_before: u32,
    started: Instant,
) -> BeamSeedResult {
    BeamSeedResult {
        stats: BeamSeedStats {
            completed: true,
            completed_depth,
            line_count: checkpoint.lines.len().try_into().unwrap_or(u8::MAX),
            new_nodes: graph
                .node_count()
                .saturating_sub(nodes_before)
                .try_into()
                .unwrap_or(u32::MAX),
            new_edges: graph.edge_count().saturating_sub(edges_before),
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
        checkpoint,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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
            food: vec![Coord { x: 2, y: 5 }],
            hazards: vec![],
            snakes: vec![
                snake("ours", &[(2, 2), (2, 1)]),
                snake("enemy", &[(5, 2), (5, 1)]),
            ],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn sufficient_budget_builds_exact_depth_three_seed() {
        let mut graph = FutureGraph::new(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let result = build_seed_checkpoint(&mut graph, &budget)
            .unwrap()
            .expect("seed must complete");

        assert_eq!(result.stats.completed_depth, SEED_DEPTH);
        assert!(!result.checkpoint.lines.is_empty());
        assert!(result.checkpoint.lines.len() <= BEAM_WIDTH);
        assert!(result
            .checkpoint
            .lines
            .iter()
            .all(|line| line.bound.is_exact()));
        assert!(result
            .checkpoint
            .lines
            .iter()
            .all(|line| { line.terminal != LineTerminal::Running || line.depth == SEED_DEPTH }));
    }

    #[test]
    fn single_viable_root_direction_returns_after_depth_one() {
        let mut initial = state();
        initial.snakes[0].body = vec![
            Coord { x: 0, y: 0 },
            Coord { x: 1, y: 0 },
            Coord { x: 1, y: 1 },
        ];

        let mut graph = FutureGraph::new(initial);
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let result = build_seed_checkpoint(&mut graph, &budget)
            .unwrap()
            .expect("single-direction seed must complete");

        assert_eq!(result.stats.completed_depth, 1);
        assert_eq!(result.checkpoint.lines.len(), 1);
        assert_eq!(result.checkpoint.lines[0].depth, 1);
        assert_eq!(
            result.checkpoint.lines[0].root_direction,
            crate::direction::Direction::Up
        );
    }

    #[test]
    fn expired_deepening_budget_keeps_last_complete_seed_checkpoint() {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(1).unwrap();

        let first_evaluation = evaluate_seed_lines(&graph, 1);
        let checkpoint =
            BeamCheckpoint::new(first_evaluation.lines).expect("depth-one checkpoint must exist");
        let original = checkpoint.clone();
        let mut next_line_id = original
            .lines
            .iter()
            .map(|line| line.id.0)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let expired = SearchBudget::for_duration(Duration::ZERO);

        let retained =
            deepen_seed_while_affordable(&mut graph, checkpoint, &expired, &mut next_line_id)
                .unwrap();

        assert_eq!(retained, original);
        assert_eq!(retained.completed_depth, 1);
    }

    #[test]
    fn exhausted_budget_never_claims_a_seed_checkpoint() {
        let mut graph = FutureGraph::new(state());
        let budget = SearchBudget::for_duration(Duration::ZERO);

        assert!(build_seed_checkpoint(&mut graph, &budget)
            .unwrap()
            .is_none());
    }
}
