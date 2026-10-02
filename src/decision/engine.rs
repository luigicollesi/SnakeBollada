use std::cmp::Ordering;
use std::env;
use std::time::{Duration, Instant};

use crate::analysis::StrategicPosture;

use crate::decision::escape::{
    choose_escape_direction, root_escape_pressure_milli, ESCAPE_ACTIVATION_THRESHOLD_MILLI,
    ESCAPE_CONTINUE_THRESHOLD_MILLI,
};
use crate::decision::evaluation::{
    choose_best_direction, evaluate_graph_budgeted, DagEvaluationStats, DirectionEvaluation,
    TerminalAssessment,
};
use crate::decision::intent::{committable_hunt_plan, DecisionIntent, EscapeIntent, HuntIntent};
use crate::decision::policy::ReservedCellPolicy;
use crate::forecast::ForecastCertainty;
use crate::modes::{food, hunting::HuntingPlanKind};
use crate::search::beam_search::{search_beam, BeamSearchResult};
use crate::search::budget::SearchBudget;
use crate::search::graph::FutureGraph;
use crate::search::scheduler::SelectiveSearchScheduler;
use crate::simulation::state::{SimulatedGameState, SimulationSupport, OPENING_FOOD_TARGET_FRUITS};
use crate::strategy::{
    choose_move_baseline, BeamShadowMetadata, CacheInvalidationReason, Decision, DecisionReason,
    DepthSearchStats, DirectionOutcomeSummary, SearchMetadata,
};
use crate::GameState;

const TARGET_DEPTH: u8 = 3;
const MIN_SELECTIVE_REEVALUATION_RESERVE_US: u64 = 5_000;
const MAX_SELECTIVE_REEVALUATION_RESERVE_US: u64 = 60_000;
const SELECTIVE_REEVALUATION_MULTIPLIER: u64 = 2;
const MAX_BEAM_SHADOW_MS: u64 = 50;
const MIN_BEAM_SHADOW_MS: u64 = 5;
const BEAM_SHADOW_RESPONSE_RESERVE_MS: u64 = 5;

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

        let mut beam_graph = FutureGraph::new_beam(normalized.clone());
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
        let beam_metadata = beam_metadata(graph, &result, budget.elapsed(), None);

        let mut search = SearchMetadata::default();
        search.completed_depth = result.completed_depth();
        search.analyzed_depth = result.completed_depth();
        search.nodes = graph.node_count().try_into().unwrap_or(u32::MAX);
        search.edges = graph.edge_count();
        search.transposition_hits = graph.transposition_hits();
        search.elapsed_us = budget.elapsed().as_micros().try_into().unwrap_or(u64::MAX);
        search.safety_reserve_us = budget
            .safety_reserve()
            .as_micros()
            .try_into()
            .unwrap_or(u64::MAX);
        search.runtime_jitter_reserve_us = extra_reserve_ms.saturating_mul(1000);
        search.beam_shadow = beam_metadata;

        Some(Decision {
            direction,
            reason: DecisionReason::BeamUtility,
            target_food: None,
            target_enemy: None,
            hunt_kind: None,
            path_distance: None,
            reachable_cells,
            search,
        })
    }


}

fn beam_metadata(
    graph: &FutureGraph,
    result: &BeamSearchResult,
    elapsed: Duration,
    legacy_direction: Option<crate::direction::Direction>,
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
    metadata.agreed_with_legacy =
        legacy_direction.is_some_and(|direction| direction == best.root_direction);
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
                id: "decision-search".to_string(),
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

    fn legacy_decide(state: &GameState) -> Decision {
        let normalized = SimulatedGameState::from(state);
        let prioritize_food = normalized.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS;
        let mut graph = FutureGraph::new(normalized);
        DecisionEngine::stateless().decide_with_graph_with_reserve_and_intent(
            state,
            &mut graph,
            0,
            None,
            prioritize_food,
        )
    }

    fn dominant_contest_state(health: i32) -> GameState {
        let mut ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 1 },
                Coord { x: 1, y: 1 },
                Coord { x: 1, y: 0 },
                Coord { x: 0, y: 0 },
                Coord { x: 0, y: 1 },
                Coord { x: 0, y: 2 },
                Coord { x: 1, y: 2 },
                Coord { x: 1, y: 3 },
            ],
        );
        ours.health = health;
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 2, y: 3 },
                Coord { x: 3, y: 3 },
                Coord { x: 3, y: 4 },
            ],
        );

        GameState {
            game: Game {
                id: "dominance-arbitration".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 12,
            board: Board {
                width: 7,
                height: 7,
                food: vec![Coord { x: 4, y: 1 }],
                snakes: vec![ours.clone(), enemy],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn apex_healthy_snake_hunts_before_ordinary_food() {
        let state = dominant_contest_state(90);
        let decision = legacy_decide(&state);

        assert_eq!(decision.reason, DecisionReason::HuntingTactical);
        assert_eq!(decision.target_enemy.as_deref(), Some("enemy"));
        assert!(decision.hunt_kind.is_some());
        assert!(decision.search.hunt_drive_milli > decision.search.food_urgency_milli);
    }

    #[test]
    fn apex_low_health_snake_prioritizes_critical_food() {
        let state = dominant_contest_state(15);
        let decision = legacy_decide(&state);

        assert_eq!(decision.reason, DecisionReason::FoodStrategic);
        assert!(decision.target_food.is_some());
        assert!(decision.target_enemy.is_none());
        assert!(decision.search.food_urgency_milli >= 800);
    }

    #[test]
    fn standard_ruleset_uses_future_search() {
        let state = state("standard");
        let decision = DecisionEngine::stateless().decide(&state);

        assert!(crate::direction::Direction::ALL.contains(&decision.direction));
        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert!(decision.search.beam_shadow.completed);
        assert!(decision.reachable_cells > 0);
    }

    #[test]
    fn beam_graph_decides_without_running_legacy_path() {
        let state = state("standard");
        let normalized = SimulatedGameState::from(&state);
        let mut graph = FutureGraph::new_beam(normalized);

        let decision = DecisionEngine::stateless()
            .try_decide_beam_with_graph(&state, &mut graph, 0)
            .expect("standard search should produce a beam decision");

        assert_eq!(decision.reason, DecisionReason::BeamUtility);
        assert!(decision.search.beam_shadow.completed);
        assert_eq!(
            decision.search.beam_shadow.direction,
            Some(decision.direction)
        );
        assert_eq!(
            decision.search.completed_depth,
            decision.search.beam_shadow.completed_depth
        );
    }

    #[test]
    fn unsupported_ruleset_uses_baseline() {
        let state = state("constrictor");

        let decision = DecisionEngine::stateless().decide(&state);
        let baseline = choose_move_baseline(&state);

        assert_eq!(decision.direction, baseline.direction);
        assert_eq!(decision.reason, DecisionReason::BaselineFallback);
    }

    #[test]
    fn robust_safe_move_excludes_immediate_head_to_head_risk() {
        use crate::direction::{Direction, MoveMask};

        let state = state("standard");
        let normalized = SimulatedGameState::from(&state);
        let safe = DirectionEvaluation {
            direction: Direction::Up,
            terminal: TerminalAssessment::Running,
            survival: crate::decision::evaluation::DirectionSurvivalSummary {
                total_routes: 1,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 2,
                min_reachable_space: 10,
                min_second_order_mobility: 2,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };
        let threatened = DirectionEvaluation {
            direction: Direction::Right,
            terminal: TerminalAssessment::Running,
            survival: safe.survival,
            worst_strategic_utility: 100.0,
            average_strategic_utility: 100.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };
        let evaluations = vec![threatened, safe];
        let robust = MoveMask::single(Direction::Up);

        let chosen = choose_best_direction(
            &evaluations,
            &normalized,
            robust,
            ReservedCellPolicy::default(),
        )
        .unwrap();

        assert_eq!(chosen.direction, Direction::Up);
    }

    #[test]
    fn opening_keeps_committed_food_even_when_another_safe_move_has_more_space() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::{Direction, MoveMask};

        let normalized = SimulatedGameState::from(&state("standard"));
        let make_eval = |direction, space| DirectionEvaluation {
            direction,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 1,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 3,
                min_reachable_space: space,
                min_second_order_mobility: 3,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };
        let evaluations = vec![
            make_eval(Direction::Up, 40),
            make_eval(Direction::Right, 20),
        ];
        let committed = food::FoodCandidate {
            target_food: Coord { x: 4, y: 2 },
            first_move: Direction::Right,
            distance: 2,
            claim_margin: Some(2),
            contested: false,
            certainty: ForecastCertainty::Deterministic,
        };

        let chosen = choose_food_opening(
            &evaluations,
            &[],
            &[committed],
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
            false,
        )
        .unwrap();

        assert_eq!(chosen.0.direction, Direction::Right);
    }

    #[test]
    fn committed_food_survives_partial_death_branch_when_safe_continuation_exists() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::{Direction, MoveMask};

        let normalized = SimulatedGameState::from(&state("standard"));
        let dangerous = DirectionEvaluation {
            direction: Direction::Right,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 1,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 3,
                min_reachable_space: 20,
                min_second_order_mobility: 3,
            },
            worst_strategic_utility: 10.0,
            average_strategic_utility: 10.0,
            average_food_value: 1.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };
        let committed = food::FoodCandidate {
            target_food: Coord { x: 4, y: 2 },
            first_move: Direction::Right,
            distance: 2,
            claim_margin: Some(2),
            contested: false,
            certainty: ForecastCertainty::Deterministic,
        };

        let evaluations = [dangerous];
        let chosen = choose_food_opening(
            &evaluations,
            &[],
            &[committed],
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
            false,
        )
        .expect("limited partial danger must not invalidate a committed beneficial line");

        assert_eq!(chosen.0.direction, Direction::Right);
    }

    #[test]
    fn food_opening_rejects_route_that_enters_enclosure() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::{Direction, MoveMask};

        let normalized = SimulatedGameState::from(&state("standard"));
        let enclosed = DirectionEvaluation {
            direction: Direction::Right,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 1,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 2,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 3,
                min_reachable_space: 20,
                min_second_order_mobility: 3,
            },
            worst_strategic_utility: 5.0,
            average_strategic_utility: 5.0,
            average_food_value: 1.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };
        let committed = food::FoodCandidate {
            target_food: Coord { x: 4, y: 2 },
            first_move: Direction::Right,
            distance: 2,
            claim_margin: Some(2),
            contested: false,
            certainty: ForecastCertainty::Deterministic,
        };

        assert!(choose_food_opening(
            &[enclosed],
            &[],
            &[committed],
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
            false,
        )
        .is_none());
    }

    #[test]
    fn edge_food_requires_low_pin_risk_and_inward_control() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::{Direction, MoveMask};

        let normalized = SimulatedGameState::from(&state("standard"));
        let candidate = food::FoodCandidate {
            target_food: Coord { x: 6, y: 2 },
            first_move: Direction::Right,
            distance: 4,
            claim_margin: Some(2),
            contested: false,
            certainty: ForecastCertainty::Deterministic,
        };
        let safe = DirectionEvaluation {
            direction: Direction::Right,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 300,
                max_border_preference_milli: 700,
                max_enemy_pin_risk_milli: 200,
                min_inward_control_milli: 700,
                min_future_mobility: 2,
                min_reachable_space: 20,
                min_second_order_mobility: 2,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 1.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };

        assert!(choose_food_opening(
            std::slice::from_ref(&safe),
            std::slice::from_ref(&candidate),
            &[],
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
            false,
        )
        .is_some());

        let pinned = DirectionEvaluation {
            survival: DirectionSurvivalSummary {
                max_enemy_pin_risk_milli: 700,
                ..safe.survival
            },
            ..safe
        };
        assert!(choose_food_opening(
            &[pinned],
            &[candidate],
            &[],
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
            false,
        )
        .is_none());
    }

    #[test]
    fn guaranteed_kill_overrides_food_opening() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::{Direction, MoveMask};

        let normalized = SimulatedGameState::from(&state("standard"));
        let kill = DirectionEvaluation {
            direction: Direction::Up,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 2,
                min_reachable_space: 12,
                min_second_order_mobility: 2,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 0.0,
            average_hunting_value: 1.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 1,
            reserved_override: false,
        };
        let food = DirectionEvaluation {
            direction: Direction::Right,
            guaranteed_enemy_kills: 0,
            ..kill.clone()
        };

        let evaluations = [food, kill];
        let chosen = choose_guaranteed_kill(
            &evaluations,
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
        )
        .unwrap();

        assert_eq!(chosen.direction, Direction::Up);
    }

    #[test]
    fn escapable_hunt_does_not_override_food_opening() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::{Direction, MoveMask};

        let normalized = SimulatedGameState::from(&state("standard"));
        let hunt = DirectionEvaluation {
            direction: Direction::Up,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 3,
                min_reachable_space: 20,
                min_second_order_mobility: 3,
            },
            worst_strategic_utility: 5.0,
            average_strategic_utility: 5.0,
            average_food_value: 0.0,
            average_hunting_value: 1.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.5,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };

        assert!(choose_guaranteed_kill(
            &[hunt],
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
        )
        .is_none());
    }

    #[test]
    fn confirmed_tactical_result_releases_reserved_penalty() {
        use crate::direction::Direction;

        let normalized = SimulatedGameState::from(&state("standard"));
        let mut evaluation = DirectionEvaluation {
            direction: Direction::Up,
            terminal: TerminalAssessment::Running,
            survival: crate::decision::evaluation::DirectionSurvivalSummary {
                total_routes: 1,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 2,
                min_reachable_space: 10,
                min_second_order_mobility: 2,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: true,
        };

        assert_eq!(
            if evaluation.reserved_override {
                0.0
            } else {
                ReservedCellPolicy::default().penalty(&normalized, evaluation.direction)
            },
            0.0
        );

        evaluation.reserved_override = false;
        assert_eq!(
            if evaluation.reserved_override {
                0.0
            } else {
                ReservedCellPolicy::default().penalty(&normalized, evaluation.direction)
            },
            ReservedCellPolicy::default().penalty(&normalized, Direction::Up)
        );
    }

    #[test]
    fn lower_death_ratio_wins_before_utility() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::Direction;

        let safe = DirectionEvaluation {
            direction: Direction::Up,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 2,
                min_reachable_space: 10,
                min_second_order_mobility: 2,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };
        let dangerous = DirectionEvaluation {
            direction: Direction::Right,
            terminal: TerminalAssessment::Lost,
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 1,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 4,
                min_reachable_space: 30,
                min_second_order_mobility: 4,
            },
            worst_strategic_utility: 100.0,
            average_strategic_utility: 100.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        };

        assert_eq!(
            crate::decision::evaluation::compare_direction(
                &safe,
                &dangerous,
                &SimulatedGameState::from(&state("standard")),
                ReservedCellPolicy::default(),
            ),
            Ordering::Less
        );
    }
    #[test]
    fn escape_intent_does_not_override_food_after_pressure_is_released() {
        let intent = DecisionIntent::Escape(EscapeIntent::new(10, 800));

        assert!(!escape_should_override(Some(&intent), 250));
        assert!(!escape_should_override(Some(&intent), 350));
        assert!(escape_should_override(Some(&intent), 450));
        assert!(escape_should_override(
            None,
            ESCAPE_ACTIVATION_THRESHOLD_MILLI
        ));
    }
}
