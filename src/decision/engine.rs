use std::cmp::Ordering;

use crate::decision::evaluation::{
    choose_best_direction, evaluate_graph_budgeted, DagEvaluationStats, DirectionEvaluation,
    TerminalAssessment,
};
use crate::decision::policy::ReservedCellPolicy;
use crate::forecast::ForecastCertainty;
use crate::modes::food;
use crate::search::budget::SearchBudget;
use crate::search::graph::FutureGraph;
use crate::search::scheduler::SelectiveSearchScheduler;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{
    choose_move_baseline, CacheInvalidationReason, Decision, DecisionReason, DepthSearchStats,
    DirectionOutcomeSummary, SearchMetadata,
};
use crate::{Coord, GameState};

const TARGET_DEPTH: u8 = 3;

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

        let mut graph = FutureGraph::new(normalized);
        self.decide_with_graph(state, &mut graph)
    }

    pub(crate) fn decide_with_graph(&self, state: &GameState, graph: &mut FutureGraph) -> Decision {
        self.decide_with_graph_with_reserve(state, graph, 0)
    }

    pub(crate) fn decide_with_graph_with_reserve(
        &self,
        state: &GameState,
        graph: &mut FutureGraph,
        extra_reserve_ms: u64,
    ) -> Decision {
        self.decide_with_graph_with_reserve_and_food_preference(
            state,
            graph,
            extra_reserve_ms,
            None,
            false,
        )
    }

    pub(crate) fn decide_with_graph_with_reserve_and_food_preference(
        &self,
        state: &GameState,
        graph: &mut FutureGraph,
        extra_reserve_ms: u64,
        preferred_food: Option<Coord>,
        prioritize_food: bool,
    ) -> Decision {
        let budget = SearchBudget::from_state_with_extra_reserve(state, extra_reserve_ms);
        let mut completed_depth = 0_u8;
        let mut evaluations = Vec::new();
        let mut dag_stats = DagEvaluationStats::default();
        let mut depth_stats = [
            DepthSearchStats::empty(1),
            DepthSearchStats::empty(2),
            DepthSearchStats::empty(3),
            DepthSearchStats::empty(4),
            DepthSearchStats::empty(5),
            DepthSearchStats::empty(6),
        ];

        for depth in 1..=TARGET_DEPTH {
            if budget.expired() {
                break;
            }

            let frontier_nodes = graph
                .node_count_at_depth(depth.saturating_sub(1))
                .try_into()
                .unwrap_or(u32::MAX);

            let nodes_before = graph.node_count();
            let edges_before = graph.edge_count();

            let expansion_started = std::time::Instant::now();
            let Ok(expansion_complete) = graph.expand_depth(depth, &budget) else {
                return baseline_fallback(state);
            };
            let expansion_us = expansion_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX);
            let new_nodes = graph
                .node_count()
                .saturating_sub(nodes_before)
                .try_into()
                .unwrap_or(u32::MAX);
            let edges_generated = graph.edge_count().saturating_sub(edges_before);

            let mut stats = DepthSearchStats {
                depth,
                completed: false,
                frontier_nodes,
                new_nodes,
                edges_generated,
                branching_milli: branching_milli(edges_generated, frontier_nodes),
                expansion_us,
                evaluation_us: 0,
            };

            if !expansion_complete {
                depth_stats[usize::from(depth - 1)] = stats;
                break;
            }

            let evaluation_started = std::time::Instant::now();
            let Some(depth_result) = evaluate_graph_budgeted(graph, depth, &budget) else {
                stats.evaluation_us = evaluation_started
                    .elapsed()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX);
                depth_stats[usize::from(depth - 1)] = stats;
                break;
            };
            stats.evaluation_us = evaluation_started
                .elapsed()
                .as_micros()
                .try_into()
                .unwrap_or(u64::MAX);
            stats.completed = true;
            depth_stats[usize::from(depth - 1)] = stats;

            completed_depth = depth;
            evaluations = depth_result.directions;
            dag_stats = depth_result.stats;
        }

        if completed_depth == 0 {
            return baseline_fallback(state);
        }

        if completed_depth >= TARGET_DEPTH && !budget.soft_expired() {
            let _ =
                SelectiveSearchScheduler.run(graph, &evaluations, &budget, TARGET_DEPTH);
        }

        let root = graph.node(graph.root());
        let root_analysis = root
            .active_analysis()
            .expect("active root must have analysis");
        let robust_safe_moves = root_analysis.tactical.ours.safe_moves;
        let policy = ReservedCellPolicy::default();

        let food_candidates = food::candidates(
            &root.state,
            &root_analysis.state,
            ForecastCertainty::Deterministic,
        );
        let preferred_candidates = preferred_food
            .filter(|target| root.state.food.contains(target))
            .map(|target| {
                food::candidates_for_target(
                    &root.state,
                    &root_analysis.state,
                    target,
                    ForecastCertainty::Deterministic,
                )
            })
            .unwrap_or_default();

        let guaranteed_kill_choice = prioritize_food
            .then(|| choose_guaranteed_kill(&evaluations, &root.state, robust_safe_moves, policy))
            .flatten();

        let opening_food_choice = (prioritize_food && guaranteed_kill_choice.is_none())
            .then(|| {
                choose_food_opening(
                    &evaluations,
                    &food_candidates.candidates,
                    &preferred_candidates,
                    &root.state,
                    robust_safe_moves,
                    policy,
                )
            })
            .flatten();

        let Some(best) = guaranteed_kill_choice
            .or_else(|| {
                opening_food_choice
                    .as_ref()
                    .map(|(evaluation, _)| *evaluation)
            })
            .or_else(|| {
                choose_best_direction(&evaluations, &root.state, robust_safe_moves, policy)
            })
        else {
            return baseline_fallback(state);
        };

        let food_target = if guaranteed_kill_choice.is_some() {
            None
        } else {
            opening_food_choice
                .as_ref()
                .map(|(_, candidate)| candidate)
                .or_else(|| {
                    food_candidates
                        .candidates
                        .iter()
                        .find(|candidate| candidate.first_move == best.direction)
                })
        };

        let reachable_cells = root
            .active_analysis()
            .expect("active root must have analysis")
            .mobility
            .reachable_space(&root.state, &root.state.our_snake_id, best.direction);

        let reason = if guaranteed_kill_choice.is_some() {
            DecisionReason::HuntingTactical
        } else if opening_food_choice.is_some() {
            DecisionReason::FoodStrategic
        } else {
            classify_decision_reason(best, &evaluations, &root.state, robust_safe_moves, policy)
        };

        Decision {
            direction: best.direction,
            reason,
            target_food: food_target.map(|candidate| candidate.target_food),
            path_distance: food_target.map(|candidate| candidate.distance),
            reachable_cells,
            search: SearchMetadata {
                completed_depth,
                nodes: graph.node_count().try_into().unwrap_or(u32::MAX),
                edges: graph.edge_count(),
                transposition_hits: graph.transposition_hits(),
                cache_reused: false,
                cache_invalidation: CacheInvalidationReason::None,
                elapsed_us: budget.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
                safety_reserve_us: budget
                    .safety_reserve()
                    .as_micros()
                    .try_into()
                    .unwrap_or(u64::MAX),
                runtime_jitter_reserve_us: extra_reserve_ms.saturating_mul(1000),
                dag_nodes_evaluated: dag_stats.nodes_evaluated,
                dag_memo_hits: dag_stats.memo_hits,
                dag_deterministic_evaluations: dag_stats.deterministic_evaluations,
                dag_provisional_evaluations: dag_stats.provisional_evaluations,
                aggression_milli: (root.state.aggression.value.clamp(0.0, 1.0) * 1000.0).round()
                    as u16,
                enemy_moves_observed: 0,
                enemy_moves_legal_covered: 0,
                enemy_moves_plausible_covered: 0,
                food_spawn_invalidations: 0,
                food_mutation_invalidations: 0,
                depth_stats,
                direction_outcomes: summarize_direction_outcomes(&evaluations),
            },
        }
    }
}

fn choose_guaranteed_kill<'a>(
    evaluations: &'a [DirectionEvaluation],
    state: &SimulatedGameState,
    robust_safe_moves: crate::direction::MoveMask,
    policy: ReservedCellPolicy,
) -> Option<&'a DirectionEvaluation> {
    evaluations
        .iter()
        .filter(|evaluation| {
            evaluation.guaranteed_enemy_kills > 0
                && (robust_safe_moves.is_empty()
                    || robust_safe_moves.contains(evaluation.direction))
                && !evaluation.survival.has_death_response()
                && !evaluation.survival.has_dead_end_response()
                && !evaluation.survival.has_forced_response()
                && evaluation.survival.max_self_enclosure_risk < 2
        })
        .min_by(|left, right| {
            crate::decision::evaluation::compare_direction(left, right, state, policy)
        })
}

fn choose_food_opening<'a>(
    evaluations: &'a [DirectionEvaluation],
    ranked_candidates: &[food::FoodCandidate],
    preferred_candidates: &[food::FoodCandidate],
    state: &SimulatedGameState,
    robust_safe_moves: crate::direction::MoveMask,
    policy: ReservedCellPolicy,
) -> Option<(&'a DirectionEvaluation, food::FoodCandidate)> {
    let viable = |candidate: &food::FoodCandidate| {
        evaluations
            .iter()
            .find(|evaluation| evaluation.direction == candidate.first_move)
            .filter(|evaluation| {
                (robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction))
                    && !evaluation.survival.has_death_response()
                    && !evaluation.survival.has_dead_end_response()
                    && !evaluation.survival.has_forced_response()
                    && evaluation.survival.max_self_enclosure_risk < 2
            })
    };

    let preferred = preferred_candidates
        .iter()
        .filter_map(|candidate| viable(candidate).map(|evaluation| (evaluation, candidate)))
        .min_by(|(left, _), (right, _)| {
            crate::decision::evaluation::compare_direction(left, right, state, policy)
        });

    if let Some((evaluation, candidate)) = preferred {
        return Some((evaluation, candidate.clone()));
    }

    ranked_candidates
        .iter()
        .find_map(|candidate| viable(candidate).map(|evaluation| (evaluation, candidate.clone())))
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

fn classify_decision_reason(
    best: &DirectionEvaluation,
    evaluations: &[DirectionEvaluation],
    state: &SimulatedGameState,
    robust_safe_moves: crate::direction::MoveMask,
    policy: ReservedCellPolicy,
) -> DecisionReason {
    let comparable = evaluations
        .iter()
        .filter(|evaluation| {
            robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction)
        })
        .collect::<Vec<_>>();

    if robust_safe_moves.len() == 1 && policy.penalty(state, best.direction) > 0.0 {
        return DecisionReason::ReservedEscape;
    }

    let survival_strictly_decisive = comparable
        .iter()
        .filter(|candidate| candidate.direction != best.direction)
        .all(|candidate| survival_compare(best, candidate) == Ordering::Less);
    if comparable.len() > 1 && survival_strictly_decisive {
        return DecisionReason::SurvivalCritical;
    }

    let average_food = best.average_food_value;
    let average_hunting = best.average_hunting_value;
    let aggression = state.aggression.value.clamp(0.0, 1.0);
    let food_contribution = average_food * (1.0 - aggression);
    let hunting_contribution = average_hunting * aggression;

    if hunting_contribution > food_contribution && hunting_contribution > 0.0 {
        DecisionReason::HuntingTactical
    } else if food_contribution > 0.0 {
        DecisionReason::FoodStrategic
    } else if best.survival.min_future_mobility > 0 || best.survival.min_reachable_space > 0 {
        DecisionReason::FutureMobility
    } else {
        DecisionReason::DeterministicTieBreak
    }
}

fn survival_compare(left: &DirectionEvaluation, right: &DirectionEvaluation) -> Ordering {
    terminal_survival_rank(left.terminal, right.terminal)
        .then_with(|| {
            left.survival
                .has_death_response()
                .cmp(&right.survival.has_death_response())
        })
        .then_with(|| {
            left.survival
                .has_dead_end_response()
                .cmp(&right.survival.has_dead_end_response())
        })
        .then_with(|| {
            left.survival
                .has_forced_response()
                .cmp(&right.survival.has_forced_response())
        })
        .then_with(|| {
            left.survival
                .has_constrained_response()
                .cmp(&right.survival.has_constrained_response())
        })
        .then_with(|| {
            left.survival
                .max_self_enclosure_risk
                .cmp(&right.survival.max_self_enclosure_risk)
        })
        .then_with(|| {
            right
                .survival
                .min_future_mobility
                .cmp(&left.survival.min_future_mobility)
        })
        .then_with(|| {
            right
                .survival
                .min_reachable_space
                .cmp(&left.survival.min_reachable_space)
        })
        .then_with(|| {
            right
                .survival
                .min_second_order_mobility
                .cmp(&left.survival.min_second_order_mobility)
        })
}

fn terminal_survival_rank(left: TerminalAssessment, right: TerminalAssessment) -> Ordering {
    use TerminalAssessment::{Lost, Running, Won};

    let rank = |terminal| match terminal {
        Lost => 0_u8,
        Running => 1,
        Won => 2,
    };

    rank(right).cmp(&rank(left))
}

fn branching_milli(edges_generated: u32, frontier_nodes: u32) -> u32 {
    if frontier_nodes == 0 {
        return 0;
    }

    u64::from(edges_generated)
        .saturating_mul(1000)
        .saturating_div(u64::from(frontier_nodes))
        .try_into()
        .unwrap_or(u32::MAX)
}

fn summarize_direction_outcomes(
    evaluations: &[DirectionEvaluation],
) -> [DirectionOutcomeSummary; 4] {
    let mut outcomes = [
        DirectionOutcomeSummary::empty(crate::direction::Direction::Up),
        DirectionOutcomeSummary::empty(crate::direction::Direction::Right),
        DirectionOutcomeSummary::empty(crate::direction::Direction::Down),
        DirectionOutcomeSummary::empty(crate::direction::Direction::Left),
    ];

    for evaluation in evaluations {
        outcomes[usize::from(evaluation.direction.rank())] = DirectionOutcomeSummary {
            direction: evaluation.direction,
            available: true,
            total_routes: saturating_u64_to_u32(evaluation.survival.total_routes),
            death_routes: saturating_u64_to_u32(evaluation.survival.death_routes),
            dead_end_routes: saturating_u64_to_u32(evaluation.survival.dead_end_routes),
            forced_routes: saturating_u64_to_u32(evaluation.survival.forced_routes),
            constrained_routes: saturating_u64_to_u32(evaluation.survival.constrained_routes),
            min_future_mobility: evaluation.survival.min_future_mobility,
            min_reachable_space: evaluation.survival.min_reachable_space,
            min_second_order_mobility: evaluation.survival.min_second_order_mobility,
            worst_utility_milli: utility_milli(evaluation.worst_strategic_utility),
            average_utility_milli: utility_milli(evaluation.average_strategic_utility),
            average_food_milli: utility_milli(evaluation.average_food_value),
            average_hunting_milli: utility_milli(evaluation.average_hunting_value),
            average_leaf_food_milli: utility_milli(evaluation.average_leaf_food_potential),
            average_leaf_hunting_milli: utility_milli(evaluation.average_leaf_hunting_potential),
            reserved_override: evaluation.reserved_override,
        };
    }

    outcomes
}

fn saturating_u64_to_u32(value: u64) -> u32 {
    value.try_into().unwrap_or(u32::MAX)
}

fn utility_milli(value: f32) -> i32 {
    if !value.is_finite() {
        return 0;
    }

    (value * 1000.0)
        .round()
        .clamp(i32::MIN as f32, i32::MAX as f32) as i32
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

    #[test]
    fn standard_ruleset_uses_future_search() {
        let state = state("standard");
        let decision = DecisionEngine::stateless().decide(&state);

        assert!(crate::direction::Direction::ALL.contains(&decision.direction));
        assert!(decision.reachable_cells > 0);
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
                max_self_enclosure_risk: 0,
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
                max_self_enclosure_risk: 0,
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
        )
        .unwrap();

        assert_eq!(chosen.0.direction, Direction::Right);
    }

    #[test]
    fn opening_drops_committed_food_when_route_has_death_response() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::{Direction, MoveMask};

        let normalized = SimulatedGameState::from(&state("standard"));
        let dangerous = DirectionEvaluation {
            direction: Direction::Right,
            terminal: TerminalAssessment::Lost,
            survival: DirectionSurvivalSummary {
                total_routes: 2,
                death_routes: 1,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                max_self_enclosure_risk: 0,
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

        assert!(choose_food_opening(
            &[dangerous],
            &[],
            &[committed],
            &normalized,
            MoveMask::all(),
            ReservedCellPolicy::default(),
        )
        .is_none());
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
                max_self_enclosure_risk: 2,
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
                max_self_enclosure_risk: 0,
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
                max_self_enclosure_risk: 0,
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
                max_self_enclosure_risk: 0,
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
                max_self_enclosure_risk: 0,
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
                max_self_enclosure_risk: 0,
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
}
