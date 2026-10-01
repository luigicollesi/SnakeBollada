#![allow(dead_code)]

use super::beam::BeamCheckpoint;
use super::beam_round::{deepen_while_affordable, BeamDeepeningStats};
use super::beam_seed::{build_seed_checkpoint, BeamSeedStats};
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

    let seed_stats = seed.stats;
    let deepening = deepen_while_affordable(graph, seed.checkpoint, budget)?;

    Ok(Some(BeamSearchResult {
        checkpoint: deepening.checkpoint,
        seed: seed_stats,
        deepening: deepening.stats,
    }))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::search::beam::{BEAM_WIDTH, SEED_DEPTH};
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
    fn orchestrator_never_returns_less_than_seed_depth() {
        let mut graph = FutureGraph::new(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let result = search_beam(&mut graph, &budget)
            .unwrap()
            .expect("beam search must complete seed");

        assert!(result.completed_depth() >= SEED_DEPTH);
        assert!(!result.checkpoint.lines.is_empty());
        assert!(result.checkpoint.lines.len() <= BEAM_WIDTH);
        assert!(result.best_line().is_some());
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
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let result = search_beam(&mut graph, &budget)
            .unwrap()
            .expect("beam search must complete");

        assert!(result.checkpoint.lines.iter().all(|line| {
            line.terminal != super::super::beam::LineTerminal::Running
                || line.depth >= result.completed_depth()
        }));
    }
}
