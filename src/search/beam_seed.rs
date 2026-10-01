#![allow(dead_code)]

use std::time::Instant;

use super::beam::{BeamCheckpoint, LineTerminal, BEAM_WIDTH, SEED_DEPTH};
use super::budget::SearchBudget;
use super::graph::{FutureGraph, SearchError};
use super::maximin::evaluate_seed_beam;

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
    let mut completed_depth = 0_u8;

    for depth in 1..=SEED_DEPTH {
        if budget.expired() {
            return Ok(None);
        }

        let expansion = graph.expand_depth(depth, budget)?;
        if !expansion.completed {
            return Ok(None);
        }
        completed_depth = depth;
    }

    let evaluation = evaluate_seed_beam(graph, SEED_DEPTH);
    if evaluation.lines.is_empty() || evaluation.lines.len() > BEAM_WIDTH {
        return Ok(None);
    }

    let valid = evaluation.lines.iter().all(|line| {
        line.bound.is_exact()
            && (line.terminal != LineTerminal::Running || line.depth == SEED_DEPTH)
    });
    if !valid {
        return Ok(None);
    }

    let Some(checkpoint) = BeamCheckpoint::new(evaluation.lines) else {
        return Ok(None);
    };

    Ok(Some(BeamSeedResult {
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
    }))
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
