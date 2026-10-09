//! Single authoritative move decision flow: FutureGraph + Hobbs minimax.
//! No strategic Food/Hunting mode and no legacy decision switch.

use crate::analysis::{
    analyze_territorial_control, detect_territorial_partition, verify_territorial_return,
    ReturnProof, TerritorialControlAnalysis,
};
use crate::direction::Direction;
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
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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
        self.try_decide_at(state, graph, extra_reserve_ms, Instant::now())
    }

    pub(crate) fn try_decide_at(
        &self,
        state: &GameState,
        graph: &mut FutureGraph,
        extra_reserve_ms: u64,
        request_started: Instant,
    ) -> Option<Decision> {
        let budget = SearchBudget::from_state_with_extra_reserve_at(
            state,
            extra_reserve_ms,
            request_started,
        );
        graph.reset_performance();
        let hobbs = search_hobbs(graph, &budget).ok().flatten()?;
        if TerritorialMode::configured() == TerritorialMode::Shadow {
            trace_territory_shadow(state, graph, &budget, hobbs.direction);
        }
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

/// Gradual activation: only a shadow diagnostic is implemented. Unrecognized
/// mode names (including future ordering/guarded modes) safely fall back to Off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerritorialMode {
    Off,
    Shadow,
}

impl TerritorialMode {
    fn parse(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("shadow") {
            Self::Shadow
        } else {
            Self::Off
        }
    }

    fn configured() -> Self {
        static MODE: OnceLock<TerritorialMode> = OnceLock::new();
        *MODE
            .get_or_init(|| Self::parse(&std::env::var("SNAKE_TERRITORY_MODE").unwrap_or_default()))
    }
}

/// Snapshot diagnostics are performed only AFTER minimax has selected a move.
/// They never affect ordering, evaluation, graph contents or the selected move.
/// Cap the work by both a tiny local timer and the request's safety reserve.
fn trace_territory_shadow(
    state: &GameState,
    graph: &FutureGraph,
    budget: &SearchBudget,
    selected: Direction,
) {
    const SHADOW_LIMIT: Duration = Duration::from_millis(12);
    let request_timeout = Duration::from_millis(u64::from(state.game.timeout));
    let reserve = budget.safety_reserve().max(Duration::from_millis(60));
    if budget
        .elapsed()
        .saturating_add(reserve)
        .saturating_add(SHADOW_LIMIT)
        >= request_timeout
    {
        log::debug!(target: "territorial_control", "territory_shadow skipped=budget turn={}", state.turn);
        return;
    }
    let expires = Instant::now() + SHADOW_LIMIT;
    let root_state = &graph.node(graph.root()).state;
    if let Some(snapshot) = analyze_territorial_control(root_state, &root_state.our_snake_id) {
        log_territory_snapshot(state.turn, "root", selected, snapshot);
    }

    // Known replies are not necessarily exhaustive: alpha-beta may have
    // pruned unexpanded opponent actions. Report sampling explicitly.
    for direction in Direction::ALL {
        if Instant::now() >= expires {
            log::debug!(target: "territorial_control", "territory_shadow partial=budget turn={}", state.turn);
            break;
        }
        let edges = graph.known_responses(graph.root(), direction);
        if edges.is_empty() {
            continue;
        }
        let known = edges.len();
        let mut sampled = 0_usize;
        let mut worst_access = u16::MAX;
        let mut worst_future = u16::MAX;
        let mut smallest_regions = u8::MAX;
        let mut smallest_largest_branch = u16::MAX;
        let mut biggest_gate = 0_u16;
        let mut worst_contested_gate = 0_u16;
        let mut unknown = 0_usize;
        let mut cut_candidates = 0_usize;
        let mut largest_partition = 0_u16;
        let mut return_verified = 0_usize;
        let mut return_not_guaranteed = 0_usize;
        let mut return_unknown = 0_usize;
        let mut return_checks = 0_usize;
        for edge in edges {
            if Instant::now() >= expires {
                break;
            }
            let child = &graph.node(edge.child).state;
            // Analyze only partitions that lose a substantial connected
            // region. These counters are sampled, not an exhaustive
            // assessment of replies pruned by minimax.
            if let Some(cut) =
                detect_territorial_partition(root_state, child, &root_state.our_snake_id)
            {
                cut_candidates += 1;
                largest_partition = largest_partition.max(cut.lost_access);
                if return_checks < 1 && Instant::now() < expires {
                    let report =
                        verify_territorial_return(child, &cut.target_region, 5, 120, expires);
                    return_checks += 1;
                    match report.result {
                        ReturnProof::VerifiedForFixedFood => return_verified += 1,
                        ReturnProof::NotGuaranteedWithinHorizon => {
                            return_not_guaranteed += 1;
                        }
                        ReturnProof::Unknown => return_unknown += 1,
                    }
                    log::info!(
                        target: "territorial_control",
                        "territory_return turn={} direction={:?} selected={} before={} after={} lost={} self_blocked={} proof={:?} horizon={} nodes={} evidence=sampled_fixed_food",
                        state.turn,
                        direction,
                        direction == selected,
                        cut.before,
                        cut.after,
                        cut.lost_access,
                        cut.self_blocked_gateways.len(),
                        report.result,
                        report.horizon,
                        report.explored,
                    );
                }
            }
            if let Some(snapshot) = analyze_territorial_control(child, &child.our_snake_id) {
                sampled += 1;
                worst_access = worst_access.min(snapshot.accessible_now);
                worst_future = worst_future.min(snapshot.future_reach);
                smallest_regions = smallest_regions.min(snapshot.independent_regions);
                smallest_largest_branch =
                    smallest_largest_branch.min(snapshot.largest_independent_region);
                biggest_gate = biggest_gate.max(snapshot.single_gate_exposure);
                worst_contested_gate = worst_contested_gate.max(snapshot.contested_gate_exposure);
            } else {
                unknown += 1;
            }
        }
        log::info!(
            target: "territorial_control",
            "territory_shadow turn={} direction={:?} selected={} known_replies={} sampled={} unknown={} complete_known={} min_access={:?} min_future={:?} min_regions={:?} min_largest_branch={:?} max_gate_exposure={} max_contested_gate={} cuts={} max_lost={} return_verified={} return_not_guaranteed={} return_unknown={} evidence=optimistic",
            state.turn,
            direction,
            direction == selected,
            known,
            sampled,
            unknown,
            sampled + unknown == known,
            if sampled > 0 { Some(worst_access) } else { None },
            if sampled > 0 { Some(worst_future) } else { None },
            if sampled > 0 { Some(smallest_regions) } else { None },
            if sampled > 0 { Some(smallest_largest_branch) } else { None },
            biggest_gate,
            worst_contested_gate,
            cut_candidates,
            largest_partition,
            return_verified,
            return_not_guaranteed,
            return_unknown,
        );
    }
}

fn log_territory_snapshot(
    turn: i32,
    scope: &str,
    selected: Direction,
    result: TerritorialControlAnalysis,
) {
    log::info!(
        target: "territorial_control",
        "territory_shadow turn={} scope={} selected={:?} access={} regions={} largest_branch={} gates={} largest_gate={} contested_gate={} future={} contested_future={} continuing={:?} evidence=optimistic",
        turn,
        scope,
        selected,
        result.accessible_now,
        result.independent_regions,
        result.largest_independent_region,
        result.critical_gateways,
        result.single_gate_exposure,
        result.contested_gate_exposure,
        result.future_reach,
        result.contested_future_reach,
        result.continuing_exits,
    );
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
    fn territorial_mode_defaults_to_off_and_requires_explicit_shadow() {
        assert_eq!(TerritorialMode::parse(""), TerritorialMode::Off);
        assert_eq!(TerritorialMode::parse("guarded"), TerritorialMode::Off);
        assert_eq!(TerritorialMode::parse("ordering"), TerritorialMode::Off);
        assert_eq!(TerritorialMode::parse("shadow"), TerritorialMode::Shadow);
        assert_eq!(TerritorialMode::parse(" Shadow "), TerritorialMode::Shadow);
    }

    #[test]
    fn unsupported_ruleset_uses_safe_baseline() {
        let decision = DecisionEngine::stateless().decide(&board("royale"));
        assert_eq!(decision.reason, DecisionReason::BaselineFallback);
    }
}
