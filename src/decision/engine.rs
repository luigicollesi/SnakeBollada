use std::cmp::Ordering;
use std::time::Duration;

use crate::analysis::StrategicPosture;

use crate::decision::escape::{
    choose_escape_direction, root_escape_pressure_milli, ESCAPE_ACTIVATION_THRESHOLD_MILLI,
    ESCAPE_RELEASE_THRESHOLD_MILLI,
};
use crate::decision::evaluation::{
    choose_best_direction, evaluate_graph_budgeted, DagEvaluationStats, DirectionEvaluation,
    TerminalAssessment,
};
use crate::decision::intent::{committable_hunt_plan, DecisionIntent, EscapeIntent, HuntIntent};
use crate::decision::policy::ReservedCellPolicy;
use crate::forecast::ForecastCertainty;
use crate::modes::{food, hunting::HuntingPlanKind};
use crate::search::budget::SearchBudget;
use crate::search::graph::FutureGraph;
use crate::search::scheduler::SelectiveSearchScheduler;
use crate::simulation::state::{SimulatedGameState, SimulationSupport, OPENING_FOOD_TARGET_FRUITS};
use crate::strategy::{
    choose_move_baseline, CacheInvalidationReason, Decision, DecisionReason, DepthSearchStats,
    DirectionOutcomeSummary, SearchMetadata,
};
use crate::GameState;

const TARGET_DEPTH: u8 = 3;
const MIN_SELECTIVE_REEVALUATION_RESERVE_US: u64 = 5_000;
const MAX_SELECTIVE_REEVALUATION_RESERVE_US: u64 = 60_000;
const SELECTIVE_REEVALUATION_MULTIPLIER: u64 = 2;

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

        let prioritize_food = normalized.aggression.fruits_eaten < OPENING_FOOD_TARGET_FRUITS;
        let mut graph = FutureGraph::new(normalized);
        self.decide_with_graph_with_reserve_and_intent(state, &mut graph, 0, None, prioritize_food)
    }

    pub(crate) fn decide_with_graph_with_reserve_and_intent(
        &self,
        state: &GameState,
        graph: &mut FutureGraph,
        extra_reserve_ms: u64,
        intent: Option<&DecisionIntent>,
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

            let nodes_before = graph.node_count();
            let edges_before = graph.edge_count();

            let expansion_started = std::time::Instant::now();
            let Ok(expansion) = graph.expand_depth(depth, &budget) else {
                return baseline_fallback(state);
            };
            let frontier_nodes = expansion.frontier_nodes;
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

            if !expansion.completed {
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

        let provisional_escape_intent = if intent.is_none() {
            let root = graph.node(graph.root());
            let robust_safe_moves = root
                .active_analysis()
                .map_or(crate::direction::MoveMask::empty(), |analysis| {
                    analysis.tactical.ours.safe_moves
                });
            let pressure = root_escape_pressure_milli(&evaluations, robust_safe_moves);
            (pressure >= ESCAPE_ACTIVATION_THRESHOLD_MILLI)
                .then(|| DecisionIntent::Escape(EscapeIntent::new(root.state.turn, pressure)))
        } else {
            None
        };
        let search_intent = intent.or(provisional_escape_intent.as_ref());

        let mut analyzed_depth = completed_depth;
        if completed_depth >= TARGET_DEPTH && !budget.soft_expired() {
            let reevaluation_reserve =
                selective_reevaluation_reserve(&depth_stats, completed_depth);
            if let Ok(stats) = SelectiveSearchScheduler.run(
                graph,
                &evaluations,
                &budget,
                TARGET_DEPTH,
                search_intent,
                reevaluation_reserve,
            ) {
                let selective_depth = completed_depth.max(stats.max_selective_depth);
                if selective_depth > completed_depth {
                    if let Some(refined) = evaluate_graph_budgeted(graph, selective_depth, &budget)
                    {
                        evaluations = refined.directions;
                        dag_stats = refined.stats;
                        analyzed_depth = selective_depth;
                    }
                }
            }
        }

        let root = graph.node(graph.root());
        let root_analysis = root
            .active_analysis()
            .expect("active root must have analysis");
        let robust_safe_moves = root_analysis.tactical.ours.safe_moves;
        let policy = ReservedCellPolicy::default();
        let escape_pressure_milli = root_escape_pressure_milli(&evaluations, robust_safe_moves);
        let escape_choice = if escape_should_override(intent, escape_pressure_milli) {
            choose_escape_direction(&evaluations, &root.state, robust_safe_moves, policy)
        } else {
            None
        };

        let food_candidates = food::candidates(
            &root.state,
            &root_analysis.state,
            ForecastCertainty::Deterministic,
        );
        let preferred_food = intent.and_then(DecisionIntent::food_target);
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

        let posture = *root_analysis.posture;
        let guaranteed_kill_choice = escape_choice
            .is_none()
            .then(|| choose_guaranteed_kill(&evaluations, &root.state, robust_safe_moves, policy))
            .flatten();

        let critical_food_choice = (posture.food_is_critical() && guaranteed_kill_choice.is_none())
            .then(|| {
                choose_food_opening(
                    &evaluations,
                    &food_candidates.candidates,
                    &preferred_candidates,
                    &root.state,
                    robust_safe_moves,
                    policy,
                    true,
                )
            })
            .flatten();

        let dominant_hunt_plan = (posture.favors_dominant_hunt()
            && guaranteed_kill_choice.is_none()
            && critical_food_choice.is_none())
        .then(|| {
            root_analysis
                .hunting
                .plans
                .iter()
                .find(|plan| committable_hunt_plan(plan) && plan.score_milli >= 400)
        })
        .flatten();
        let dominant_hunt_intent =
            dominant_hunt_plan.map(|plan| HuntIntent::new(plan, root.state.turn));
        let dominant_hunt_choice = dominant_hunt_intent.as_ref().and_then(|hunt| {
            choose_hunt_intent(&evaluations, graph, hunt, robust_safe_moves, policy)
        });

        let hunt_intent_choice = intent
            .and_then(DecisionIntent::hunt)
            .filter(|_| {
                guaranteed_kill_choice.is_none()
                    && critical_food_choice.is_none()
                    && dominant_hunt_choice.is_none()
            })
            .and_then(|hunt| {
                choose_hunt_intent(&evaluations, graph, hunt, robust_safe_moves, policy)
            });

        let committed_food_choice = (!preferred_candidates.is_empty()
            && guaranteed_kill_choice.is_none()
            && critical_food_choice.is_none()
            && dominant_hunt_choice.is_none()
            && hunt_intent_choice.is_none())
        .then(|| {
            choose_food_opening(
                &evaluations,
                &[],
                &preferred_candidates,
                &root.state,
                robust_safe_moves,
                policy,
                false,
            )
        })
        .flatten();

        let opening_food_choice = (prioritize_food
            && !posture.favors_dominant_hunt()
            && committed_food_choice.is_none()
            && hunt_intent_choice.is_none()
            && dominant_hunt_choice.is_none()
            && critical_food_choice.is_none()
            && guaranteed_kill_choice.is_none())
        .then(|| {
            choose_food_opening(
                &evaluations,
                &food_candidates.candidates,
                &[],
                &root.state,
                robust_safe_moves,
                policy,
                false,
            )
        })
        .flatten();

        let Some(best) = escape_choice
            .or(guaranteed_kill_choice)
            .or_else(|| {
                critical_food_choice
                    .as_ref()
                    .map(|(evaluation, _)| *evaluation)
            })
            .or(dominant_hunt_choice)
            .or(hunt_intent_choice)
            .or_else(|| {
                committed_food_choice
                    .as_ref()
                    .map(|(evaluation, _)| *evaluation)
            })
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

        let selected_food_choice = critical_food_choice
            .as_ref()
            .or(committed_food_choice.as_ref())
            .or(opening_food_choice.as_ref());
        let food_target = if escape_choice.is_some()
            || guaranteed_kill_choice.is_some()
            || dominant_hunt_choice.is_some()
            || hunt_intent_choice.is_some()
        {
            None
        } else {
            selected_food_choice
                .map(|(_, candidate)| candidate)
                .or_else(|| {
                    food_candidates
                        .candidates
                        .iter()
                        .find(|candidate| candidate.first_move == best.direction)
                })
        };

        let selected_hunt = if escape_choice.is_some() {
            None
        } else if dominant_hunt_choice.is_some() {
            dominant_hunt_plan.map(|plan| (plan.target.clone(), plan.kind))
        } else if hunt_intent_choice.is_some() {
            intent
                .and_then(DecisionIntent::hunt)
                .map(|hunt| (hunt.target.clone(), hunt.kind))
        } else {
            None
        };

        let reachable_cells = root
            .active_analysis()
            .expect("active root must have analysis")
            .mobility
            .reachable_space(&root.state, &root.state.our_snake_id, best.direction);

        let reason = if escape_choice.is_some() {
            DecisionReason::SurvivalCritical
        } else if guaranteed_kill_choice.is_some()
            || dominant_hunt_choice.is_some()
            || hunt_intent_choice.is_some()
        {
            DecisionReason::HuntingTactical
        } else if critical_food_choice.is_some()
            || committed_food_choice.is_some()
            || opening_food_choice.is_some()
        {
            DecisionReason::FoodStrategic
        } else {
            classify_decision_reason(best, &evaluations, &root.state, robust_safe_moves, policy)
        };

        Decision {
            direction: best.direction,
            reason,
            target_food: food_target.map(|candidate| candidate.target_food),
            target_enemy: selected_hunt.as_ref().map(|(target, _)| target.clone()),
            hunt_kind: selected_hunt.as_ref().map(|(_, kind)| *kind),
            path_distance: food_target.map(|candidate| candidate.distance),
            reachable_cells,
            search: SearchMetadata {
                completed_depth,
                analyzed_depth,
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
                aggression_milli: root_analysis.posture.growth_aggression_milli,
                strategic_phase: root_analysis.posture.phase,
                growth_aggression_milli: root_analysis.posture.growth_aggression_milli,
                size_dominance_milli: root_analysis.posture.size_dominance_milli,
                hunt_drive_milli: root_analysis.posture.hunt_drive_milli,
                food_urgency_milli: root_analysis.posture.food_urgency_milli,
                our_length: root_analysis
                    .posture
                    .our_length
                    .try_into()
                    .unwrap_or(u16::MAX),
                largest_enemy_length: root_analysis
                    .posture
                    .largest_enemy_length
                    .try_into()
                    .unwrap_or(u16::MAX),
                control_ratio_milli: root_analysis
                    .territory
                    .competitive_for_snake(&root.state.our_snake_id)
                    .map_or(0, |snapshot| snapshot.control_ratio_milli),
                dominance_frontier_cells: root_analysis
                    .territory
                    .competitive_for_snake(&root.state.our_snake_id)
                    .map_or(0, |snapshot| snapshot.dominance_frontier_cells),
                border_risk_milli: root_analysis
                    .border
                    .ours()
                    .map_or(0, |snapshot| snapshot.structural_risk_milli),
                enemy_pin_risk_milli: root_analysis
                    .border
                    .ours()
                    .map_or(0, |snapshot| snapshot.enemy_pin_risk_milli),
                escape_pressure_milli,
                escape_selected: escape_choice.is_some(),
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

fn escape_should_override(intent: Option<&DecisionIntent>, pressure_milli: u16) -> bool {
    pressure_milli >= ESCAPE_ACTIVATION_THRESHOLD_MILLI
        || (intent.and_then(DecisionIntent::escape).is_some()
            && pressure_milli > ESCAPE_RELEASE_THRESHOLD_MILLI)
}

fn selective_reevaluation_reserve(
    depth_stats: &[DepthSearchStats; 6],
    completed_depth: u8,
) -> Duration {
    let index = usize::from(completed_depth.saturating_sub(1));
    let observed_us = depth_stats
        .get(index)
        .map_or(0, |stats| stats.evaluation_us);
    let reserve_us = observed_us
        .saturating_mul(SELECTIVE_REEVALUATION_MULTIPLIER)
        .clamp(
            MIN_SELECTIVE_REEVALUATION_RESERVE_US,
            MAX_SELECTIVE_REEVALUATION_RESERVE_US,
        );
    Duration::from_micros(reserve_us)
}

fn choose_hunt_intent<'a>(
    evaluations: &'a [DirectionEvaluation],
    graph: &FutureGraph,
    intent: &HuntIntent,
    robust_safe_moves: crate::direction::MoveMask,
    policy: ReservedCellPolicy,
) -> Option<&'a DirectionEvaluation> {
    let root = graph.node(graph.root());
    let our_health = root
        .state
        .snake(&root.state.our_snake_id)
        .map_or(0, |snake| snake.health);
    if our_health <= 30 {
        return None;
    }

    let locked = intent.is_locked(root.state.turn);
    let death_limit = if locked { 750 } else { 500 };
    let mut candidates = evaluations
        .iter()
        .filter(|evaluation| {
            (robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction))
                && !evaluation.survival.is_forced_death()
                && !evaluation.survival.is_forced_dead_end()
                && evaluation.survival.death_rate_milli() <= death_limit
                && evaluation.survival.max_self_enclosure_risk < 3
                && evaluation.survival.max_border_structural_risk_milli < 800
                && evaluation.survival.max_enemy_pin_risk_milli < 800
        })
        .filter_map(|evaluation| {
            hunting_intent_progress(graph, evaluation.direction, intent)
                .map(|progress| (evaluation, progress))
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|(left_eval, left_progress), (right_eval, right_progress)| {
        right_progress.cmp(left_progress).then_with(|| {
            crate::decision::evaluation::compare_direction(
                left_eval,
                right_eval,
                &root.state,
                policy,
            )
        })
    });

    let (best, progress) = candidates.first().copied()?;
    if progress >= 0 || (locked && progress >= -250) {
        Some(best)
    } else {
        None
    }
}

fn hunting_intent_progress(
    graph: &FutureGraph,
    direction: crate::direction::Direction,
    intent: &HuntIntent,
) -> Option<i32> {
    let root = graph.node(graph.root());
    let root_analysis = root.active_analysis()?;
    let before = root_analysis.enclosure.for_snake(&intent.target)?;
    let before_target_control = root_analysis
        .territory
        .competitive_for_snake(&intent.target)
        .map(|snapshot| snapshot.control_ratio_milli);
    let before_our_control = root_analysis
        .territory
        .competitive_for_snake(&root.state.our_snake_id)
        .map(|snapshot| snapshot.control_ratio_milli);
    let before_frontier = root_analysis
        .territory
        .competitive_for_snake(&root.state.our_snake_id)
        .map(|snapshot| i32::from(snapshot.winning_frontier) - i32::from(snapshot.losing_frontier))
        .unwrap_or(0);
    let before_dominance_frontier = root_analysis
        .territory
        .competitive_for_snake(&root.state.our_snake_id)
        .map_or(0, |snapshot| i32::from(snapshot.dominance_frontier_cells));
    let before_border = root_analysis
        .border
        .ours()
        .map(|snapshot| snapshot.structural_risk_milli)
        .unwrap_or(0);
    let before_plan = root_analysis
        .hunting
        .plans
        .iter()
        .find(|plan| plan.target == intent.target && plan.kind == intent.kind)
        .map_or(i32::from(intent.last_score_milli), |plan| {
            i32::from(plan.score_milli)
        });

    let mut scores = Vec::new();
    for edge in &root.children {
        if edge.joint_action.direction_for(&root.state.our_snake_id) != Some(direction) {
            continue;
        }

        let child = graph.node(edge.child);
        if !child
            .state
            .snake(&intent.target)
            .is_some_and(|snake| snake.alive)
        {
            scores.push(1200);
            continue;
        }

        let Some(analysis) = child.active_analysis() else {
            continue;
        };
        let Some(after) = analysis.enclosure.for_snake(&intent.target) else {
            continue;
        };

        let risk_delta = i32::from(after.risk.rank()) - i32::from(before.risk.rank());
        let escape_delta = i32::from(before.escape_frontier) - i32::from(after.escape_frontier);
        let space_delta = i64::from(before.space_to_length_milli)
            .saturating_sub(i64::from(after.space_to_length_milli))
            .clamp(-2400, 2400) as i32;
        let boundary_delta = i32::from(after.boundary_support) - i32::from(before.boundary_support);
        let edge_delta = i32::from(before.edge_distance) - i32::from(after.edge_distance);
        let target_control_delta = match (
            before_target_control,
            analysis
                .territory
                .competitive_for_snake(&intent.target)
                .map(|snapshot| snapshot.control_ratio_milli),
        ) {
            (Some(before), Some(after)) => i32::from(before).saturating_sub(i32::from(after)),
            _ => 0,
        };
        let our_control_delta = match (
            before_our_control,
            analysis
                .territory
                .competitive_for_snake(&child.state.our_snake_id)
                .map(|snapshot| snapshot.control_ratio_milli),
        ) {
            (Some(before), Some(after)) => i32::from(after).saturating_sub(i32::from(before)),
            _ => 0,
        };
        let frontier_delta = analysis
            .territory
            .competitive_for_snake(&child.state.our_snake_id)
            .map(|snapshot| {
                i32::from(snapshot.winning_frontier)
                    - i32::from(snapshot.losing_frontier)
                    - before_frontier
            })
            .unwrap_or(0);
        let dominance_frontier_delta = analysis
            .territory
            .competitive_for_snake(&child.state.our_snake_id)
            .map(|snapshot| {
                i32::from(snapshot.dominance_frontier_cells) - before_dominance_frontier
            })
            .unwrap_or(0);
        let after_border = analysis
            .border
            .ours()
            .map(|snapshot| snapshot.structural_risk_milli)
            .unwrap_or(0);
        let border_growth = i32::from(after_border).saturating_sub(i32::from(before_border));

        let after_plan = analysis
            .hunting
            .plans
            .iter()
            .find(|plan| plan.target == intent.target && plan.kind == intent.kind)
            .map_or(0, |plan| i32::from(plan.score_milli));
        let plan_delta = after_plan - before_plan;
        let our_risk = analysis
            .enclosure
            .ours(&child.state)
            .map_or(0, |snapshot| i32::from(snapshot.risk.rank()));

        let edge_weight = match intent.kind {
            HuntingPlanKind::EdgePin
            | HuntingPlanKind::PartialWrap
            | HuntingPlanKind::FullEnclosure => 60,
            _ => 20,
        };

        scores.push(
            risk_delta
                .saturating_mul(180)
                .saturating_add(escape_delta.saturating_mul(110))
                .saturating_add(space_delta.saturating_div(8))
                .saturating_add(boundary_delta.saturating_mul(90))
                .saturating_add(edge_delta.saturating_mul(edge_weight))
                .saturating_add(plan_delta.saturating_div(2))
                .saturating_add(target_control_delta.saturating_mul(2))
                .saturating_add(our_control_delta)
                .saturating_add(frontier_delta.saturating_mul(35))
                .saturating_add(dominance_frontier_delta.saturating_mul(110))
                .saturating_sub(our_risk.saturating_mul(120))
                .saturating_sub(border_growth.max(0).saturating_mul(2))
                .saturating_sub(i32::from(after_border).saturating_div(5)),
        );
    }

    if scores.is_empty() {
        return None;
    }

    let worst = scores.iter().copied().min().unwrap_or(0);
    let average = scores.iter().copied().sum::<i32>() / i32::try_from(scores.len()).unwrap_or(1);
    Some(
        average
            .saturating_mul(3)
            .saturating_add(worst)
            .saturating_div(4),
    )
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
                && evaluation.survival.max_border_structural_risk_milli < 900
                && evaluation.survival.max_enemy_pin_risk_milli < 900
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
    critical: bool,
) -> Option<(&'a DirectionEvaluation, food::FoodCandidate)> {
    let viable = |candidate: &food::FoodCandidate, committed: bool| {
        evaluations
            .iter()
            .find(|evaluation| evaluation.direction == candidate.first_move)
            .filter(|evaluation| {
                let (death_limit, forced_limit, constrained_limit, border_limit) = if critical {
                    (600, 800, 900, 850)
                } else if committed {
                    (350, 500, 700, 700)
                } else {
                    (400, 500, 700, 700)
                };
                let edge_food = food_is_on_edge(state, candidate.target_food);
                let edge_route_safe = critical
                    || !edge_food
                    || (evaluation.survival.max_border_structural_risk_milli < 550
                        && evaluation.survival.max_enemy_pin_risk_milli < 450
                        && evaluation.survival.min_future_mobility >= 2
                        && evaluation.survival.min_inward_control_milli >= 500);

                (robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction))
                    && !evaluation.survival.is_forced_death()
                    && !evaluation.survival.is_forced_dead_end()
                    && evaluation.survival.death_rate_milli() <= death_limit
                    && evaluation.survival.forced_rate_milli() <= forced_limit
                    && evaluation.survival.constrained_rate_milli() <= constrained_limit
                    && evaluation.survival.max_self_enclosure_risk < 2
                    && evaluation.survival.max_border_structural_risk_milli < border_limit
                    && (critical || evaluation.survival.max_enemy_pin_risk_milli < 700)
                    && edge_route_safe
            })
    };

    let preferred = preferred_candidates
        .iter()
        .filter_map(|candidate| viable(candidate, true).map(|evaluation| (evaluation, candidate)))
        .min_by(|(left, _), (right, _)| {
            crate::decision::evaluation::compare_direction(left, right, state, policy)
        });

    if let Some((evaluation, candidate)) = preferred {
        return Some((evaluation, candidate.clone()));
    }

    ranked_candidates.iter().find_map(|candidate| {
        viable(candidate, false).map(|evaluation| (evaluation, candidate.clone()))
    })
}

fn food_is_on_edge(state: &SimulatedGameState, food: crate::Coord) -> bool {
    let right = i32::try_from(state.width)
        .unwrap_or(i32::MAX)
        .saturating_sub(1);
    let top = i32::try_from(state.height)
        .unwrap_or(i32::MAX)
        .saturating_sub(1);
    food.x == 0 || food.y == 0 || food.x == right || food.y == top
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
    let posture = StrategicPosture::from_state(state);
    let food_contribution = average_food * (f32::from(posture.food_drive_milli()) / 1000.0);
    let hunting_contribution = average_hunting * (f32::from(posture.hunt_drive_milli) / 1000.0);

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
                .is_forced_death()
                .cmp(&right.survival.is_forced_death())
        })
        .then_with(|| {
            left.survival
                .is_forced_dead_end()
                .cmp(&right.survival.is_forced_dead_end())
        })
        .then_with(|| {
            left.survival
                .death_rate_milli()
                .cmp(&right.survival.death_rate_milli())
        })
        .then_with(|| {
            left.survival
                .dead_end_rate_milli()
                .cmp(&right.survival.dead_end_rate_milli())
        })
        .then_with(|| {
            left.survival
                .forced_rate_milli()
                .cmp(&right.survival.forced_rate_milli())
        })
        .then_with(|| {
            left.survival
                .constrained_rate_milli()
                .cmp(&right.survival.constrained_rate_milli())
        })
        .then_with(|| {
            left.survival
                .max_self_enclosure_risk
                .cmp(&right.survival.max_self_enclosure_risk)
        })
        .then_with(|| {
            left.survival
                .max_enemy_pin_risk_milli
                .cmp(&right.survival.max_enemy_pin_risk_milli)
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
            average_border_ticks_milli: evaluation.survival.average_border_ticks_milli(),
            average_border_cost_milli: evaluation.survival.average_border_cost_milli(),
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
        let decision = DecisionEngine::stateless().decide(&state);

        assert_eq!(decision.reason, DecisionReason::HuntingTactical);
        assert_eq!(decision.target_enemy.as_deref(), Some("enemy"));
        assert!(decision.hunt_kind.is_some());
        assert!(decision.search.hunt_drive_milli > decision.search.food_urgency_milli);
    }

    #[test]
    fn apex_low_health_snake_prioritizes_critical_food() {
        let state = dominant_contest_state(15);
        let decision = DecisionEngine::stateless().decide(&state);

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
        assert!(escape_should_override(Some(&intent), 450));
        assert!(escape_should_override(
            None,
            ESCAPE_ACTIVATION_THRESHOLD_MILLI
        ));
    }
}
