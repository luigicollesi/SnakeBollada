use std::cmp::Ordering;

use crate::decision::evaluation::{evaluate_graph_budgeted, DirectionEvaluation};
use crate::decision::policy::ReservedCellPolicy;
use crate::forecast::ForecastCertainty;
use crate::modes::food;
use crate::search::budget::SearchBudget;
use crate::search::graph::FutureGraph;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{
    choose_move_baseline, CacheInvalidationReason, Decision, DecisionReason, DepthSearchStats,
    DirectionOutcomeSummary, SearchMetadata,
};
use crate::GameState;

const TARGET_DEPTH: u8 = 3;
const MAX_ITERATIVE_DEPTH: u8 = 6;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DecisionEngine;

impl DecisionEngine {
    pub(crate) fn stateless() -> Self {
        Self
    }

    pub(crate) fn decide(&self, state: &GameState) -> Decision {
        let normalized = SimulatedGameState::from(state);

        if normalized.rules.simulation_support() != SimulationSupport::StandardLike {
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
        let budget = SearchBudget::from_state_with_extra_reserve(state, extra_reserve_ms);
        let mut completed_depth = 0_u8;
        let mut evaluations = Vec::new();
        let mut depth_stats = [
            DepthSearchStats::empty(1),
            DepthSearchStats::empty(2),
            DepthSearchStats::empty(3),
            DepthSearchStats::empty(4),
            DepthSearchStats::empty(5),
            DepthSearchStats::empty(6),
        ];

        for depth in 1..=MAX_ITERATIVE_DEPTH {
            if budget.expired() {
                break;
            }

            let frontier_nodes = graph
                .node_count_at_depth(depth.saturating_sub(1))
                .try_into()
                .unwrap_or(u32::MAX);

            if depth > TARGET_DEPTH {
                let previous = depth_stats[usize::from(depth.saturating_sub(2))];
                let estimate = estimate_next_depth(previous, frontier_nodes);
                if !budget.can_afford(estimate) {
                    break;
                }
            }

            let nodes_before = graph.node_count();
            let edges_before = graph.edge_count();

            let expansion_started = std::time::Instant::now();
            let Ok(expansion_complete) = graph.expand_depth(depth, &budget) else {
                return choose_move_baseline(state);
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
            let Some(depth_evaluations) = evaluate_graph_budgeted(graph, depth, &budget) else {
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
            evaluations = depth_evaluations;
        }

        if completed_depth == 0 {
            return choose_move_baseline(state);
        }

        let root = graph.node(graph.root());

        let Some(best) = choose_best_direction(
            &evaluations,
            &root.state,
            root.analysis.tactical.ours.safe_moves,
            ReservedCellPolicy::default(),
        ) else {
            return choose_move_baseline(state);
        };

        let food_candidates = food::candidates(
            &root.state,
            &root.analysis.state,
            ForecastCertainty::Deterministic,
        );
        let food_target = food_candidates
            .candidates
            .iter()
            .find(|candidate| candidate.first_move == best.direction);

        let reachable_cells = root.analysis.mobility.reachable_space(
            &root.state,
            &root.state.our_snake_id,
            best.direction,
        );

        Decision {
            direction: best.direction,
            reason: if food_target.is_some() {
                DecisionReason::NearestSafeFood
            } else {
                DecisionReason::SurvivalFallback
            },
            target_food: food_target.map(|candidate| candidate.target_food),
            path_distance: food_target.map(|candidate| candidate.distance),
            reachable_cells,
            search: SearchMetadata {
                completed_depth: completed_depth,
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

fn estimate_next_depth(previous: DepthSearchStats, next_frontier_nodes: u32) -> std::time::Duration {
    let previous_cost_us = previous
        .expansion_us
        .saturating_add(previous.evaluation_us)
        .max(1_000);

    if !previous.completed || previous.edges_generated == 0 || previous.frontier_nodes == 0 {
        return std::time::Duration::from_micros(previous_cost_us.saturating_mul(2));
    }

    let branching_milli = previous.branching_milli.max(1000);
    let predicted_edges = u64::from(next_frontier_nodes)
        .saturating_mul(u64::from(branching_milli))
        .saturating_div(1000)
        .max(1);
    let scaled_cost = u128::from(previous_cost_us)
        .saturating_mul(u128::from(predicted_edges))
        .saturating_div(u128::from(previous.edges_generated.max(1)));
    let with_margin = scaled_cost.saturating_mul(5).saturating_div(4);
    let estimate_us = with_margin.try_into().unwrap_or(u64::MAX);

    std::time::Duration::from_micros(estimate_us.max(1_000))
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
            total_routes: evaluation.survival.total_routes,
            death_routes: evaluation.survival.death_routes,
            dead_end_routes: evaluation.survival.dead_end_routes,
            forced_routes: evaluation.survival.forced_routes,
            constrained_routes: evaluation.survival.constrained_routes,
            min_future_mobility: evaluation.survival.min_future_mobility,
            min_reachable_space: evaluation.survival.min_reachable_space,
            worst_utility_milli: utility_milli(evaluation.worst_strategic_utility),
            average_utility_milli: utility_milli(evaluation.average_strategic_utility),
            reserved_override: evaluation.reserved_override,
        };
    }

    outcomes
}

fn utility_milli(value: f32) -> i32 {
    if !value.is_finite() {
        return 0;
    }

    (value * 1000.0)
        .round()
        .clamp(i32::MIN as f32, i32::MAX as f32) as i32
}

fn choose_best_direction<'a>(
    evaluations: &'a [DirectionEvaluation],
    state: &SimulatedGameState,
    robust_safe_moves: crate::direction::MoveMask,
    policy: ReservedCellPolicy,
) -> Option<&'a DirectionEvaluation> {
    evaluations
        .iter()
        .filter(|evaluation| {
            robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction)
        })
        .min_by(|left, right| compare_direction(left, right, state, policy))
}

fn compare_direction(
    left: &DirectionEvaluation,
    right: &DirectionEvaluation,
    state: &SimulatedGameState,
    policy: ReservedCellPolicy,
) -> Ordering {
    compare_lower_ratio(
        left.survival.death_routes,
        left.survival.total_routes,
        right.survival.death_routes,
        right.survival.total_routes,
    )
    .then_with(|| {
        compare_lower_ratio(
            left.survival.dead_end_routes,
            left.survival.total_routes,
            right.survival.dead_end_routes,
            right.survival.total_routes,
        )
    })
    .then_with(|| {
        compare_lower_ratio(
            left.survival.forced_routes,
            left.survival.total_routes,
            right.survival.forced_routes,
            right.survival.total_routes,
        )
    })
    .then_with(|| {
        compare_lower_ratio(
            left.survival.constrained_routes,
            left.survival.total_routes,
            right.survival.constrained_routes,
            right.survival.total_routes,
        )
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
        effective_reserved_penalty(policy, state, left)
            .total_cmp(&effective_reserved_penalty(policy, state, right))
    })
    .then_with(|| {
        right
            .worst_strategic_utility
            .total_cmp(&left.worst_strategic_utility)
    })
    .then_with(|| {
        right
            .average_strategic_utility
            .total_cmp(&left.average_strategic_utility)
    })
    .then_with(|| left.direction.rank().cmp(&right.direction.rank()))
}

fn effective_reserved_penalty(
    policy: ReservedCellPolicy,
    state: &SimulatedGameState,
    evaluation: &DirectionEvaluation,
) -> f32 {
    if evaluation.reserved_override {
        0.0
    } else {
        policy.penalty(state, evaluation.direction)
    }
}

fn compare_lower_ratio(
    left_numerator: u32,
    left_denominator: u32,
    right_numerator: u32,
    right_denominator: u32,
) -> Ordering {
    let left_denominator = left_denominator.max(1) as u64;
    let right_denominator = right_denominator.max(1) as u64;

    (u64::from(left_numerator) * right_denominator)
        .cmp(&(u64::from(right_numerator) * left_denominator))
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

        assert_eq!(
            DecisionEngine::stateless().decide(&state),
            choose_move_baseline(&state)
        );
    }

    #[test]
    fn robust_safe_move_excludes_immediate_head_to_head_risk() {
        use crate::direction::{Direction, MoveMask};

        let state = state("standard");
        let normalized = SimulatedGameState::from(&state);
        let safe = DirectionEvaluation {
            direction: Direction::Up,
            routes: vec![],
            survival: crate::decision::evaluation::DirectionSurvivalSummary {
                total_routes: 1,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                min_future_mobility: 2,
                min_reachable_space: 10,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            reserved_override: false,
        };
        let threatened = DirectionEvaluation {
            direction: Direction::Right,
            routes: vec![],
            survival: safe.survival,
            worst_strategic_utility: 100.0,
            average_strategic_utility: 100.0,
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
    fn confirmed_tactical_result_releases_reserved_penalty() {
        use crate::direction::Direction;

        let normalized = SimulatedGameState::from(&state("standard"));
        let mut evaluation = DirectionEvaluation {
            direction: Direction::Up,
            routes: vec![],
            survival: crate::decision::evaluation::DirectionSurvivalSummary {
                total_routes: 1,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                min_future_mobility: 2,
                min_reachable_space: 10,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            reserved_override: true,
        };

        assert_eq!(
            effective_reserved_penalty(ReservedCellPolicy::default(), &normalized, &evaluation,),
            0.0
        );

        evaluation.reserved_override = false;
        assert_eq!(
            effective_reserved_penalty(ReservedCellPolicy::default(), &normalized, &evaluation,),
            ReservedCellPolicy::default().penalty(&normalized, Direction::Up)
        );
    }

    #[test]
    fn lower_death_ratio_wins_before_utility() {
        use crate::decision::evaluation::DirectionSurvivalSummary;
        use crate::direction::Direction;

        let safe = DirectionEvaluation {
            direction: Direction::Up,
            routes: vec![],
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 0,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                min_future_mobility: 2,
                min_reachable_space: 10,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            reserved_override: false,
        };
        let dangerous = DirectionEvaluation {
            direction: Direction::Right,
            routes: vec![],
            survival: DirectionSurvivalSummary {
                total_routes: 4,
                death_routes: 1,
                dead_end_routes: 0,
                forced_routes: 0,
                constrained_routes: 0,
                min_future_mobility: 4,
                min_reachable_space: 30,
            },
            worst_strategic_utility: 100.0,
            average_strategic_utility: 100.0,
            reserved_override: false,
        };

        assert_eq!(
            compare_direction(
                &safe,
                &dangerous,
                &SimulatedGameState::from(&state("standard")),
                ReservedCellPolicy::default(),
            ),
            Ordering::Less
        );
    }
}
