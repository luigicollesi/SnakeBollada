//! Single authoritative move decision flow: FutureGraph + Hobbs minimax.
//! No strategic Food/Hunting mode and no legacy decision switch.

use crate::evaluation::StateScore;
use crate::search::budget::SearchBudget;
use crate::search::graph::FutureGraph;
use crate::search::hobbs_flow::search_hobbs;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{
    choose_move_baseline, direction_stays_in_bounds, Decision, DecisionReason, HobbsSearchMetadata,
    SearchMetadata,
};
use crate::GameState;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DecisionEngine;

impl DecisionEngine {
    pub(crate) fn stateless() -> Self {
        Self
    }

    pub(crate) fn decide(&self, state: &GameState) -> Decision {
        let normalized = SimulatedGameState::from(state);
        if normalized.rules.simulation_support() != SimulationSupport::StandardLike {
            return baseline_fallback(state);
        }
        let policy = crate::search::forecast::FoodForecastPolicy::from_game_state(state);
        let mut graph = FutureGraph::new_beam_with_forecast(normalized, policy);
        self.try_decide(state, &mut graph, 0)
            .unwrap_or_else(|| baseline_fallback(state))
    }

    pub(crate) fn try_decide(
        &self,
        state: &GameState,
        graph: &mut FutureGraph,
        extra_reserve_ms: u64,
    ) -> Option<Decision> {
        let budget = SearchBudget::from_state_with_extra_reserve(state, extra_reserve_ms);
        graph.reset_performance();
        let hobbs = search_hobbs(graph, &budget).ok().flatten()?;
        if !direction_stays_in_bounds(state, hobbs.direction) {
            return None;
        }

        let root = &graph.node(graph.root()).state;
        let mobility = crate::simulation::mobility::MobilityAnalysis::from_state(root);
        let reachable_cells = mobility.reachable_space(root, &root.our_snake_id, hobbs.direction);
        let perf = graph.performance();
        log::debug!(
            target: "search_diagnostics",
            "hobbs_selected turn={} direction={:?} score={:?} guard={:?} depth={} root_directions={} provisional={}",
            state.turn,
            hobbs.direction,
            hobbs.score,
            hobbs.survival,
            hobbs.completed_depth,
            hobbs.root_directions,
            hobbs.certainty.is_provisional()
        );

        Some(Decision {
            direction: hobbs.direction,
            reason: DecisionReason::HobbsSearch,
            reachable_cells,
            search: SearchMetadata {
                completed_depth: hobbs.completed_depth,
                analyzed_depth: hobbs.completed_depth,
                nodes: graph.node_count().try_into().unwrap_or(u32::MAX),
                edges: graph.edge_count(),
                transposition_hits: graph.transposition_hits(),
                elapsed_us: budget.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                safety_reserve_us: budget
                    .safety_reserve()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
                runtime_jitter_reserve_us: extra_reserve_ms.saturating_mul(1000),
                hobbs: HobbsSearchMetadata {
                    direction: Some(hobbs.direction),
                    score: Some(hobbs.score),
                    guard: Some(hobbs.survival),
                    root_directions: hobbs.root_directions.try_into().unwrap_or(u8::MAX),
                    forecast_provisional: hobbs.certainty.is_provisional(),
                    terminal_confirmed: matches!(
                        hobbs.score,
                        StateScore::Win | StateScore::Loss | StateScore::Tie
                    ) && !hobbs.certainty.is_provisional(),
                    action_batches: perf.action_batches,
                    parallel_action_batches: perf.parallel_action_batches,
                    resolved_actions: perf.resolved_actions,
                    new_nodes_built: perf.new_nodes_built,
                    resolve_us: perf.resolve_us,
                    node_build_us: perf.node_build_us,
                    merge_us: perf.merge_us,
                },
            },
        })
    }
}

fn baseline_fallback(state: &GameState) -> Decision {
    let mut decision = choose_move_baseline(state);
    if !matches!(
        decision.reason,
        DecisionReason::OnlyLegalMove | DecisionReason::NoSafeMove
    ) {
        decision.reason = DecisionReason::BaselineFallback;
    }
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};
    use serde_json::json;
    use std::collections::HashMap;

    fn board(ruleset: &str) -> GameState {
        let ours = Battlesnake {
            id: "ours".into(),
            name: "ours".into(),
            health: 100,
            head: Coord { x: 1, y: 1 },
            body: vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }],
            length: 2,
            latency: String::new(),
            shout: None,
        };
        GameState {
            game: Game {
                id: "hobbs-decision".into(),
                ruleset: HashMap::from([("name".into(), json!(ruleset))]),
                timeout: 500,
            },
            turn: 0,
            board: Board {
                width: 7,
                height: 7,
                food: vec![Coord { x: 3, y: 3 }],
                snakes: vec![
                    ours.clone(),
                    Battlesnake {
                        id: "enemy".into(),
                        name: "enemy".into(),
                        head: Coord { x: 5, y: 5 },
                        body: vec![Coord { x: 5, y: 5 }, Coord { x: 5, y: 4 }],
                        ..ours.clone()
                    },
                ],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn standard_ruleset_uses_hobbs_as_sole_search() {
        let decision = DecisionEngine::stateless().decide(&board("standard"));
        assert_eq!(decision.reason, DecisionReason::HobbsSearch);
        assert!(decision.search.completed_depth >= 1);
        assert_eq!(decision.search.hobbs.direction, Some(decision.direction));
    }

    #[test]
    fn unsupported_ruleset_uses_safe_baseline() {
        let decision = DecisionEngine::stateless().decide(&board("royale"));
        assert_eq!(decision.reason, DecisionReason::BaselineFallback);
    }
}
