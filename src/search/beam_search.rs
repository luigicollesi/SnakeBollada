#![allow(dead_code)]

use super::beam::BeamCheckpoint;
use super::beam_round::{deepen_while_affordable, BeamDeepeningStats};
use super::beam_seed::{build_seed_checkpoint, BeamSeedResult, BeamSeedStats};
use super::budget::SearchBudget;
use super::graph::{FutureGraph, SearchError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BeamSearchResult {
    pub(crate) checkpoint: BeamCheckpoint,
    pub(crate) seed: BeamSeedStats,
    pub(crate) deepening: BeamDeepeningStats,
}

impl BeamSearchResult {
    pub(crate) fn best_line(&self) -> Option<&super::beam::BeamLine> {
        self.checkpoint.best_line()
    }

    pub(crate) fn completed_depth(&self) -> u8 {
        self.checkpoint.completed_depth
    }
}

pub(crate) fn search_beam(
    graph: &mut FutureGraph,
    budget: &SearchBudget,
) -> Result<Option<BeamSearchResult>, SearchError> {
    let Some(seed) = build_seed_checkpoint(graph, budget)? else {
        return Ok(None);
    };

    search_from_seed(graph, seed, budget).map(Some)
}

fn search_from_seed(
    graph: &mut FutureGraph,
    seed: BeamSeedResult,
    budget: &SearchBudget,
) -> Result<BeamSearchResult, SearchError> {
    let seed_stats = seed.stats;

    if has_single_root_direction(&seed.checkpoint) {
        let completed_depth = seed.checkpoint.completed_depth;
        log::debug!(
            target: "search_diagnostics",
            "stop turn={} reason=single_root_after_seed depth={} lines={}",
            graph.node(graph.root()).state.turn,
            completed_depth,
            seed.checkpoint.lines.len()
        );
        return Ok(BeamSearchResult {
            checkpoint: seed.checkpoint,
            seed: seed_stats,
            deepening: BeamDeepeningStats {
                rounds_completed: 0,
                attempted_depth: completed_depth,
                completed_depth,
                elapsed_us: 0,
            },
        });
    }

    let deepening = deepen_while_affordable(graph, seed.checkpoint, budget)?;
    log::debug!(
        target: "search_diagnostics",
        "stop turn={} reason=deepening_finished completed_depth={} attempted_depth={} rounds={} lines={}",
        graph.node(graph.root()).state.turn,
        deepening.stats.completed_depth,
        deepening.stats.attempted_depth,
        deepening.stats.rounds_completed,
        deepening.checkpoint.lines.len()
    );

    Ok(BeamSearchResult {
        checkpoint: deepening.checkpoint,
        seed: seed_stats,
        deepening: deepening.stats,
    })
}

fn has_single_root_direction(checkpoint: &BeamCheckpoint) -> bool {
    let Some(first) = checkpoint.lines.first() else {
        return false;
    };

    checkpoint
        .lines
        .iter()
        .all(|line| line.root_direction == first.root_direction)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::search::beam::{BEAM_WIDTH, SEED_DEPTH};
    use crate::search::maximin::evaluate_seed_beam;
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

    fn deterministic_seed(graph: &mut FutureGraph) -> BeamSeedResult {
        graph.expand_to_depth(SEED_DEPTH).unwrap();
        let evaluation = evaluate_seed_beam(graph, SEED_DEPTH);
        let checkpoint = BeamCheckpoint::new(evaluation.lines).expect("seed lines must exist");

        BeamSeedResult {
            stats: BeamSeedStats {
                completed: true,
                completed_depth: SEED_DEPTH,
                line_count: checkpoint.lines.len().try_into().unwrap_or(u8::MAX),
                new_nodes: graph.node_count().try_into().unwrap_or(u32::MAX),
                new_edges: graph.edge_count(),
                elapsed_us: 0,
            },
            checkpoint,
        }
    }

    #[test]
    fn orchestrator_never_returns_less_than_seed_depth() {
        let mut graph = FutureGraph::new(state());
        let seed = deterministic_seed(&mut graph);
        let budget = SearchBudget::for_duration(Duration::ZERO);

        let result = search_from_seed(&mut graph, seed, &budget).unwrap();

        assert!(result.completed_depth() >= SEED_DEPTH);
        assert!(!result.checkpoint.lines.is_empty());
        assert!(result.checkpoint.lines.len() <= BEAM_WIDTH);
        assert!(result.best_line().is_some());
    }

    #[test]
    fn single_root_direction_skips_deepening() {
        use crate::direction::Direction;
        use crate::search::beam::{BeamLine, LineTerminal};

        let mut graph = FutureGraph::new(state());
        let line = BeamLine::exact(
            1,
            Direction::Right,
            SEED_DEPTH,
            100,
            0,
            LineTerminal::Running,
        );
        let checkpoint = BeamCheckpoint::new(vec![line]).unwrap();
        let seed = BeamSeedResult {
            stats: BeamSeedStats {
                completed: true,
                completed_depth: SEED_DEPTH,
                line_count: 1,
                new_nodes: 0,
                new_edges: 0,
                elapsed_us: 0,
            },
            checkpoint,
        };
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let result = search_from_seed(&mut graph, seed, &budget).unwrap();

        assert_eq!(result.checkpoint.lines.len(), 1);
        assert_eq!(result.best_line().unwrap().root_direction, Direction::Right);
        assert_eq!(result.deepening.rounds_completed, 0);
        assert_eq!(result.deepening.attempted_depth, SEED_DEPTH);
        assert_eq!(result.completed_depth(), SEED_DEPTH);
    }

    #[test]
    fn orchestrator_returns_none_when_seed_cannot_finish() {
        let mut graph = FutureGraph::new(state());
        let budget = SearchBudget::for_duration(Duration::ZERO);

        assert!(search_beam(&mut graph, &budget).unwrap().is_none());
    }

    #[test]
    fn every_committed_running_line_has_common_completed_depth() {
        let mut graph = FutureGraph::new(state());
        let seed = deterministic_seed(&mut graph);
        let budget = SearchBudget::for_duration(Duration::ZERO);

        let result = search_from_seed(&mut graph, seed, &budget).unwrap();

        assert!(result.checkpoint.lines.iter().all(|line| {
            line.terminal != super::super::beam::LineTerminal::Running
                || line.depth >= result.completed_depth()
        }));
    }
}
