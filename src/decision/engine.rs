use std::time::Duration;

use crate::search::beam_search::{search_beam, BeamSearchResult};
use crate::search::budget::SearchBudget;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{
    choose_move_baseline, BeamShadowMetadata, Decision, DecisionReason, SearchMetadata,
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

        let has_living_enemy = normalized
            .snakes
            .iter()
            .any(|snake| snake.alive && snake.id != normalized.our_snake_id);
        if !has_living_enemy {
            return choose_move_baseline(state);
        }

        let mut beam_graph = FutureGraph::new_beam(normalized);
        if let Some(decision) = self.try_decide_beam_with_graph(state, &mut beam_graph, 0) {
            return decision;
        }

        baseline_fallback(state)
    }

    pub(crate) fn try_decide_beam_with_graph(
        &self,
        state: &GameState,
        graph: &mut FutureGraph,
        extra_reserve_ms: u64,
    ) -> Option<Decision> {
        let budget = SearchBudget::from_state_with_extra_reserve(state, extra_reserve_ms);
        graph.reset_performance();

        let result = search_beam(graph, &budget).ok().flatten()?;
        let best = result.best_line()?;
        let direction = best.root_direction;
        let root = graph.node(graph.root());
        let reachable_cells = root.active_analysis().map_or(0, |analysis| {
            analysis
                .mobility
                .reachable_space(&root.state, &root.state.our_snake_id, direction)
        });
        let beam_metadata = beam_metadata(graph, &result, budget.elapsed());

        let search = SearchMetadata {
            completed_depth: result.completed_depth(),
            analyzed_depth: result.completed_depth(),
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
            beam_shadow: beam_metadata,
            ..SearchMetadata::default()
        };

        Some(Decision {
            direction,
            reason: DecisionReason::BeamUtility,
            reachable_cells,
            search,
        })
    }
}

fn beam_metadata(
    graph: &FutureGraph,
    result: &BeamSearchResult,
    elapsed: Duration,
) -> BeamShadowMetadata {
    let perf = graph.performance();
    let mut metadata = BeamShadowMetadata {
        enabled: true,
        completed: true,
        completed_depth: result.completed_depth(),
        attempted_depth: result.deepening.attempted_depth,
        line_count: result.checkpoint.lines.len().try_into().unwrap_or(u8::MAX),
        elapsed_us: elapsed.as_micros().try_into().unwrap_or(u64::MAX),
        action_batches: perf.action_batches,
        parallel_action_batches: perf.parallel_action_batches,
        resolved_actions: perf.resolved_actions,
        new_nodes_built: perf.new_nodes_built,
        resolve_us: perf.resolve_us,
        node_build_us: perf.node_build_us,
        merge_us: perf.merge_us,
        edge_score_us: perf.edge_score_us,
        ..BeamShadowMetadata::default()
    };

    let Some(best) = result.best_line() else {
        return metadata;
    };

    metadata.direction = Some(best.root_direction);
    metadata.best_value = best.value;

    let root_state = &graph.node(graph.root()).state;
    let our_index = root_state.actor_index(&root_state.our_snake_id);
    for step in best.path.steps() {
        let Some(edge) = graph
            .node(step.node)
            .children
            .iter()
            .find(|edge| edge.child == step.child && edge.joint_action == step.joint_action)
        else {
            continue;
        };

        for (actor, score) in edge.transition.actors.iter() {
            let food = score.food_benefit.saturating_sub(score.food_harm);
            let hunting = score.hunting_benefit.saturating_sub(score.hunting_harm);
            let survival = score.survival_benefit.saturating_sub(score.survival_harm);
            let terminal = score.terminal_benefit.saturating_sub(score.terminal_harm);

            if Some(actor) == our_index {
                metadata.our_food_utility = metadata.our_food_utility.saturating_add(food);
                metadata.our_hunting_utility = metadata.our_hunting_utility.saturating_add(hunting);
                metadata.our_survival_utility =
                    metadata.our_survival_utility.saturating_add(survival);
                metadata.our_terminal_utility =
                    metadata.our_terminal_utility.saturating_add(terminal);
            } else {
                metadata.opponent_food_utility =
                    metadata.opponent_food_utility.saturating_add(food);
                metadata.opponent_hunting_utility =
                    metadata.opponent_hunting_utility.saturating_add(hunting);
                metadata.opponent_survival_utility =
                    metadata.opponent_survival_utility.saturating_add(survival);
                metadata.opponent_terminal_utility =
                    metadata.opponent_terminal_utility.saturating_add(terminal);
            }
        }
    }

    metadata
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
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};

    fn snake(id: &str, body: Vec<Coord>) -> Battlesnake {
        Battlesnake {
            id: id.to_string(),
            name: id.to_string(),
            health: 100,
            head: body[0],
            length: body.len() as u32,
            body,
            latency: String::new(),
            shout: None,
        }
    }

    fn state(ruleset: &str) -> GameState {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 0 },
            ],
        );
        let enemy = snake("enemy", vec![Coord { x: 5, y: 5 }, Coord { x: 5, y: 4 }]);

        GameState {
            game: Game {
                id: "beam-authority".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!(ruleset))]),
                timeout: 500,
            },
            turn: 2,
            board: Board {
                width: 7,
                height: 7,
                food: vec![Coord { x: 4, y: 2 }],
                snakes: vec![ours.clone(), enemy],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn standard_ruleset_uses_beam_authority() {
        let state = state("standard");
        let decision = DecisionEngine::stateless().decide(&state);

        assert!(crate::direction::Direction::ALL.contains(&decision.direction));
        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert!(decision.search.beam_shadow.completed);
        assert_eq!(
            decision.search.beam_shadow.direction,
            Some(decision.direction)
        );
        assert!(decision.reachable_cells > 0);
    }

    #[test]
    fn persistent_beam_graph_uses_lean_analysis() {
        let state = state("standard");
        let normalized = SimulatedGameState::from(&state);
        let mut graph = FutureGraph::new_beam(normalized);

        let decision = DecisionEngine::stateless()
            .try_decide_beam_with_graph(&state, &mut graph, 0)
            .expect("standard search should produce a beam decision");

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert!(decision.search.beam_shadow.completed);
        assert_eq!(
            decision.search.completed_depth,
            decision.search.beam_shadow.completed_depth
        );
        let analysis = graph
            .node(graph.root())
            .active_analysis()
            .expect("beam root must keep actor-relative analysis");
        assert!(!analysis.actor_snapshots.is_empty());
        assert!(analysis
            .territory
            .competitive_for_snake(&graph.node(graph.root()).state.our_snake_id)
            .is_some());
    }

    #[test]
    fn unsupported_ruleset_uses_baseline() {
        let state = state("constrictor");

        let decision = DecisionEngine::stateless().decide(&state);
        let baseline = choose_move_baseline(&state);

        assert_eq!(decision.direction, baseline.direction);
        assert_eq!(decision.reason, DecisionReason::BaselineFallback);
    }
}
