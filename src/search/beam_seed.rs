#![allow(dead_code)]

use std::time::Instant;

use super::beam::{
    select_seed_beam, BeamCheckpoint, BeamLine, LineId, LineTerminal, BEAM_WIDTH, SEED_DEPTH,
};
use super::beam_round::append_continuation;
use super::budget::SearchBudget;
use super::graph::{FutureGraph, NodeId, SearchError};
use super::maximin::{evaluate_continuations, evaluate_seed_beam};

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

    let first_evaluation = evaluate_seed_beam(graph, 1);
    if first_evaluation.lines.is_empty() || first_evaluation.lines.len() > BEAM_WIDTH {
        return Ok(None);
    }

    let Some(mut checkpoint) = BeamCheckpoint::new(first_evaluation.lines) else {
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

    while completed_depth < SEED_DEPTH {
        if budget.expired() {
            return Ok(None);
        }

        let Some(next_checkpoint) = deepen_seed_one_layer(
            graph,
            &checkpoint,
            budget,
            &mut next_line_id,
        )? else {
            return Ok(None);
        };

        checkpoint = next_checkpoint;
        completed_depth = checkpoint.completed_depth;

        if has_single_root_direction(&checkpoint.lines) {
            break;
        }
    }

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
        let Some(tip) = line.steps.last().map(|step| step.child) else {
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

        let Some(tip) = line.steps.last().map(|step| step.child) else {
            return Ok(None);
        };
        let continuations = evaluate_continuations(graph, tip, 1);
        for continuation in continuations
            .into_iter()
            .filter(|continuation| continuation.bound.is_exact())
        {
            let mut candidate = append_continuation(line, continuation);
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
            aggression: AggressionState::default(),
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
    fn exhausted_budget_never_claims_a_seed_checkpoint() {
        let mut graph = FutureGraph::new(state());
        let budget = SearchBudget::for_duration(Duration::ZERO);

        assert!(build_seed_checkpoint(&mut graph, &budget)
            .unwrap()
            .is_none());
    }
}
