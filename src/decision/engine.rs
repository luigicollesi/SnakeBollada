use std::cmp::Ordering;

use crate::decision::evaluation::{evaluate_graph, DirectionEvaluation};
use crate::decision::policy::ReservedCellPolicy;
use crate::forecast::ForecastCertainty;
use crate::modes::food;
use crate::search::budget::SearchBudget;
use crate::search::graph::FutureGraph;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{choose_move_baseline, Decision, DecisionReason, SearchMetadata};
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
        let budget = SearchBudget::from_state(state);
        let Ok(expansion) =
            graph.expand_iteratively(TARGET_DEPTH, MAX_ITERATIVE_DEPTH, &budget)
        else {
            return choose_move_baseline(state);
        };

        if expansion.completed_depth == 0 {
            return choose_move_baseline(state);
        }

        let evaluations = evaluate_graph(graph, expansion.completed_depth);
        let root = graph.node(graph.root());

        let Some(best) =
            choose_best_direction(&evaluations, &root.state, ReservedCellPolicy::default())
        else {
            return choose_move_baseline(state);
        };

        let food_candidates = food::candidates(
            &root.state,
            &root.analysis,
            ForecastCertainty::Deterministic,
        );
        let food_target = food_candidates
            .candidates
            .iter()
            .find(|candidate| candidate.first_move == best.direction);

        let mobility = MobilityAnalysis::from_state(&root.state);
        let reachable_cells =
            mobility.reachable_space(&root.state, &root.state.our_snake_id, best.direction);

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
                completed_depth: expansion.completed_depth,
                nodes: expansion.nodes,
                edges: expansion.edges,
                transposition_hits: expansion.transposition_hits,
                cache_reused: false,
                elapsed_us: expansion.elapsed_us,
                safety_reserve_us: expansion.safety_reserve_us,
                aggression_milli: (root.state.aggression.value.clamp(0.0, 1.0) * 1000.0)
                    .round() as u16,
            },
        }
    }
}

fn choose_best_direction<'a>(
    evaluations: &'a [DirectionEvaluation],
    state: &SimulatedGameState,
    policy: ReservedCellPolicy,
) -> Option<&'a DirectionEvaluation> {
    evaluations
        .iter()
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
        right
            .worst_strategic_utility
            .total_cmp(&left.worst_strategic_utility)
    })
    .then_with(|| {
        right
            .average_strategic_utility
            .total_cmp(&left.average_strategic_utility)
    })
    .then_with(|| {
        policy
            .penalty(state, left.direction)
            .total_cmp(&policy.penalty(state, right.direction))
    })
    .then_with(|| left.direction.rank().cmp(&right.direction.rank()))
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
