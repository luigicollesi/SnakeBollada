use std::time::Duration;

use crate::direction::Direction;
use crate::search::beam::{BeamLine, LineTerminal};
use crate::search::beam_search::{search_beam, BeamSearchResult};
use crate::search::budget::SearchBudget;
use crate::search::forecast::{FoodForecastPolicy, ForecastCertainty, PROVISIONAL_TERMINAL_VALUE};
use crate::search::graph::FutureGraph;
use crate::simulation::state::{SimulatedGameState, SimulationSupport};
use crate::strategy::{
    choose_move_baseline, direction_stays_in_bounds, BeamShadowMetadata, Decision, DecisionReason,
    SearchMetadata,
};
use crate::GameState;

const ABSOLUTE_SWITCH_MARGIN: i64 = 150;
const RELATIVE_SWITCH_MARGIN_PERCENT: i64 = 8;
const IMMEDIATE_GROWTH_PRESSURE_THRESHOLD_MILLI: u16 = 750;
const IMMEDIATE_SAFE_FOOD_REGRET: i64 = 1_500;

#[derive(Debug, Clone)]
pub(crate) struct BeamDecisionOutcome {
    pub(crate) decision: Decision,
    pub(crate) selected_line: BeamLine,
}

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

        let forecast_policy = FoodForecastPolicy::from_game_state(state);
        let mut beam_graph = FutureGraph::new_beam_with_forecast(normalized, forecast_policy);
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
        self.try_decide_beam_with_continuity(state, graph, extra_reserve_ms, None)
            .map(|outcome| outcome.decision)
    }

    pub(crate) fn try_decide_beam_with_continuity(
        &self,
        state: &GameState,
        graph: &mut FutureGraph,
        extra_reserve_ms: u64,
        incumbent_direction: Option<Direction>,
    ) -> Option<BeamDecisionOutcome> {
        let budget = SearchBudget::from_state_with_extra_reserve(state, extra_reserve_ms);
        graph.reset_performance();

        let result = search_beam(graph, &budget).ok().flatten()?;
        let selected = select_line_with_continuity(graph, &result, incumbent_direction)?.clone();
        if let Some(root_analysis) = graph.node(graph.root()).active_analysis() {
            if let Some(our_actor) = graph
                .node(graph.root())
                .state
                .actor_index(&graph.node(graph.root()).state.our_snake_id)
            {
                if let Some(snapshot) = root_analysis.actor_snapshot(our_actor) {
                    log::debug!(
                        target: "search_diagnostics",
                        "root turn={} weights={}/{}/{} metrics moves={} space={} territory={} growth={} size_security={} enclosure={} border_struct={} border_pin={} border_escape={} starvation={} health={}",
                        state.turn,
                        snapshot.weights.food,
                        snapshot.weights.hunting,
                        snapshot.weights.survival,
                        snapshot.metrics.safe_non_reverse_moves,
                        snapshot.metrics.space_capacity_milli,
                        snapshot.metrics.territory_control_milli,
                        snapshot.metrics.growth_pressure_milli,
                        snapshot.metrics.size_security_milli,
                        snapshot.metrics.enclosure_risk,
                        snapshot.metrics.border_structural_risk_milli,
                        snapshot.metrics.border_pin_risk_milli,
                        snapshot.metrics.border_escape_pressure_milli,
                        snapshot.metrics.food_survival_pressure_milli,
                        snapshot.metrics.health_pressure_milli
                    );
                }
            }
        }
        log::debug!(
            target: "search_diagnostics",
            "decision turn={} completed_depth={} attempted_depth={} lines={} selected={:?}",
            state.turn,
            result.completed_depth(),
            result.deepening.attempted_depth,
            result
                .checkpoint
                .lines
                .iter()
                .map(|line| format!(
                    "{:?}:v={} ours={} opp={} depth={} term={:?} cert={:?}",
                    line.root_direction,
                    line.value,
                    line.our_utility_total,
                    line.opponent_utility_total,
                    line.depth,
                    line.terminal,
                    line.certainty
                ))
                .collect::<Vec<_>>()
                .join("|"),
            selected.root_direction
        );
        let direction = selected.root_direction;
        if !direction_stays_in_bounds(state, direction) {
            return None;
        }

        let root = graph.node(graph.root());
        let reachable_cells = root.active_analysis().map_or(0, |analysis| {
            analysis
                .mobility
                .reachable_space(&root.state, &root.state.our_snake_id, direction)
        });
        let beam_metadata = beam_metadata_for_line(graph, &result, &selected, budget.elapsed());

        let search = SearchMetadata {
            completed_depth: result.completed_depth(),
            analyzed_depth: selected.depth,
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
        };

        Some(BeamDecisionOutcome {
            decision: Decision {
                direction,
                reason: DecisionReason::BeamUtility,
                reachable_cells,
                search,
            },
            selected_line: selected,
        })
    }
}

fn select_line_with_continuity<'a>(
    graph: &FutureGraph,
    result: &'a BeamSearchResult,
    incumbent_direction: Option<Direction>,
) -> Option<&'a BeamLine> {
    let best = result.best_line()?;
    if let Some(surviving_line) = avoid_provisional_terminal_loss(&result.checkpoint.lines, best) {
        log::debug!(
            target: "search_diagnostics",
            "override turn={} reason=provisional_terminal_loss best={:?}:{} selected={:?}:{}",
            graph.node(graph.root()).state.turn,
            best.root_direction,
            best.value,
            surviving_line.root_direction,
            surviving_line.value
        );
        return Some(surviving_line);
    }
    if let Some(safer_line) = avoid_immediate_forced_corridor(graph, result, best) {
        log::debug!(
            target: "search_diagnostics",
            "override turn={} reason=forced_corridor best={:?}:{} selected={:?}:{}",
            graph.node(graph.root()).state.turn,
            best.root_direction,
            best.value,
            safer_line.root_direction,
            safer_line.value
        );
        return Some(safer_line);
    }
    if let Some(food_claim) = immediate_safe_growth_claim(graph, result, best) {
        log::debug!(
            target: "search_diagnostics",
            "override turn={} reason=safe_growth_claim best={:?}:{} selected={:?}:{}",
            graph.node(graph.root()).state.turn,
            best.root_direction,
            best.value,
            food_claim.root_direction,
            food_claim.value
        );
        return Some(food_claim);
    }

    let Some(direction) = incumbent_direction else {
        return Some(best);
    };
    let Some(incumbent) = result
        .checkpoint
        .lines
        .iter()
        .find(|line| line.root_direction == direction && line.is_viable())
    else {
        return Some(best);
    };

    if incumbent.root_direction == best.root_direction {
        return Some(best);
    }

    let selected =
        select_incumbent_or_challenger(best, incumbent, root_has_survival_emergency(graph));
    if selected.root_direction != best.root_direction {
        log::debug!(
            target: "search_diagnostics",
            "override turn={} reason=continuity best={:?}:{} selected={:?}:{}",
            graph.node(graph.root()).state.turn,
            best.root_direction,
            best.value,
            selected.root_direction,
            selected.value
        );
    }
    Some(selected)
}

fn avoid_provisional_terminal_loss<'a>(
    lines: &'a [BeamLine],
    best: &'a BeamLine,
) -> Option<&'a BeamLine> {
    if best.terminal != LineTerminal::Lost || !best.certainty.is_provisional() {
        return None;
    }

    lines
        .iter()
        .filter(|line| line.is_viable() && line.terminal != LineTerminal::Lost)
        .max_by(|left, right| {
            left.value
                .cmp(&right.value)
                .then_with(|| left.our_utility_total.cmp(&right.our_utility_total))
                .then_with(|| right.root_direction.rank().cmp(&left.root_direction.rank()))
        })
}

fn avoid_immediate_forced_corridor<'a>(
    graph: &FutureGraph,
    result: &'a BeamSearchResult,
    best: &'a BeamLine,
) -> Option<&'a BeamLine> {
    if best.is_confirmed_win() {
        return None;
    }

    let root = graph.node(graph.root());
    let our_actor = root.state.actor_index(&root.state.our_snake_id)?;
    let root_snapshot = root.active_analysis()?.actor_snapshot(our_actor)?;
    let best_mobility = line_first_child_mobility(graph, best, our_actor)?;
    let required_alternative_mobility = match best_mobility {
        0 => 1,
        1 if root_snapshot.metrics.safe_non_reverse_moves > 1 => 2,
        _ => return None,
    };

    result
        .checkpoint
        .lines
        .iter()
        .filter(|line| line.is_viable())
        .filter(|line| {
            line_first_child_mobility(graph, line, our_actor)
                .is_some_and(|moves| moves >= required_alternative_mobility)
        })
        .max_by(|left, right| {
            left.value
                .cmp(&right.value)
                .then_with(|| left.our_utility_total.cmp(&right.our_utility_total))
                .then_with(|| right.root_direction.rank().cmp(&left.root_direction.rank()))
        })
}

fn line_first_child_mobility(
    graph: &FutureGraph,
    line: &BeamLine,
    our_actor: crate::simulation::state::ActorIndex,
) -> Option<u8> {
    let step = line.path.first()?;
    if step.node != graph.root() {
        return None;
    }

    graph
        .node(step.child)
        .active_analysis()
        .and_then(|analysis| analysis.actor_snapshot(our_actor))
        .map(|snapshot| snapshot.metrics.safe_non_reverse_moves)
}

fn immediate_safe_growth_claim<'a>(
    graph: &FutureGraph,
    result: &'a BeamSearchResult,
    best: &'a BeamLine,
) -> Option<&'a BeamLine> {
    let root = graph.node(graph.root());
    let our_actor = root.state.actor_index(&root.state.our_snake_id)?;
    let snapshot = root.active_analysis()?.actor_snapshot(our_actor)?;
    if snapshot.metrics.growth_pressure_milli < IMMEDIATE_GROWTH_PRESSURE_THRESHOLD_MILLI {
        return None;
    }

    result
        .checkpoint
        .lines
        .iter()
        .filter(|line| {
            line.is_viable() && line.value.saturating_add(IMMEDIATE_SAFE_FOOD_REGRET) >= best.value
        })
        .filter(|line| line_immediately_claims_safe_food(graph, line, our_actor))
        .max_by(|left, right| {
            left.value
                .cmp(&right.value)
                .then_with(|| right.root_direction.rank().cmp(&left.root_direction.rank()))
        })
}

fn line_immediately_claims_safe_food(
    graph: &FutureGraph,
    line: &BeamLine,
    our_actor: crate::simulation::state::ActorIndex,
) -> bool {
    let root = graph.node(graph.root());
    let Some(step) = line.path.first() else {
        return false;
    };
    if step.node != graph.root() {
        return false;
    }
    let Some(edge) = root
        .children
        .iter()
        .find(|edge| edge.child == step.child && edge.joint_action == step.joint_action)
    else {
        return false;
    };
    let child = graph.node(edge.child);
    let Some(our_snake) = child.state.snake_at(our_actor) else {
        return false;
    };
    if !our_snake.alive {
        return false;
    }
    let Some(head) = our_snake.head() else {
        return false;
    };
    if !root.state.food.contains(&head) {
        return false;
    }

    child
        .active_analysis()
        .and_then(|analysis| analysis.actor_snapshot(our_actor))
        .is_some_and(|snapshot| snapshot.metrics.safe_non_reverse_moves > 0)
}

fn select_incumbent_or_challenger<'a>(
    challenger: &'a BeamLine,
    incumbent: &'a BeamLine,
    emergency: bool,
) -> &'a BeamLine {
    if emergency
        || incumbent.is_confirmed_loss()
        || (challenger.is_confirmed_win() && !incumbent.is_confirmed_win())
    {
        return challenger;
    }

    let relative_margin = incumbent
        .value
        .saturating_abs()
        .saturating_mul(RELATIVE_SWITCH_MARGIN_PERCENT)
        .saturating_div(100);
    let margin = ABSOLUTE_SWITCH_MARGIN.max(relative_margin);

    if challenger.value > incumbent.value.saturating_add(margin) {
        challenger
    } else {
        incumbent
    }
}

fn root_has_survival_emergency(graph: &FutureGraph) -> bool {
    let root = graph.node(graph.root());
    let Some(analysis) = root.active_analysis() else {
        return true;
    };
    let Some(actor) = root.state.actor_index(&root.state.our_snake_id) else {
        return true;
    };
    let Some(snapshot) = analysis.actor_snapshot(actor) else {
        return true;
    };

    snapshot.metrics.space_capacity_milli <= 100
        || snapshot.metrics.enclosure_risk >= 3
        || snapshot.metrics.border_escape_pressure_milli >= 800
        || snapshot.metrics.food_survival_pressure_milli >= 800
        || snapshot.metrics.health_pressure_milli >= 800
}

fn beam_metadata_for_line(
    graph: &FutureGraph,
    result: &BeamSearchResult,
    selected: &BeamLine,
    elapsed: Duration,
) -> BeamShadowMetadata {
    let perf = graph.performance();
    let mut metadata = BeamShadowMetadata {
        enabled: true,
        completed: true,
        completed_depth: result.completed_depth(),
        selected_depth: selected.depth,
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

    metadata.direction = Some(selected.root_direction);
    metadata.best_value = selected.value;
    metadata.forecast_provisional = selected.certainty.is_provisional();
    metadata.terminal_confirmed = selected.is_confirmed_win() || selected.is_confirmed_loss();

    let root_state = &graph.node(graph.root()).state;
    let our_index = root_state.actor_index(&root_state.our_snake_id);
    let mut certainty = ForecastCertainty::Deterministic;
    for step in selected.path.steps() {
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
            let raw_terminal = score.terminal_benefit.saturating_sub(score.terminal_harm);
            let terminal = if raw_terminal != 0 && certainty.is_provisional() {
                raw_terminal
                    .signum()
                    .saturating_mul(PROVISIONAL_TERMINAL_VALUE)
            } else {
                raw_terminal
            };

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

        if !graph.node(edge.child).is_terminal() {
            certainty = certainty.after(edge.forecast_delta);
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

    use crate::search::beam::LineTerminal;

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

    fn line(id: u32, direction: Direction, value: i64) -> BeamLine {
        BeamLine::exact(
            id,
            direction,
            3,
            value.max(0),
            value.max(0).saturating_sub(value),
            LineTerminal::Running,
        )
    }

    #[test]
    fn provisional_terminal_loss_does_not_beat_running_line() {
        let mut dying = line(1, Direction::Right, 5_000);
        dying.terminal = LineTerminal::Lost;
        dying.certainty = ForecastCertainty::FoodProvisional;
        let running = line(2, Direction::Up, 1_000);
        let lines = vec![dying, running];

        let chosen = avoid_provisional_terminal_loss(&lines, &lines[0])
            .expect("a still-running route must replace a provisional death");

        assert_eq!(chosen.root_direction, Direction::Up);
        assert_eq!(chosen.terminal, LineTerminal::Running);
    }

    #[test]
    fn marginal_challenger_does_not_replace_incumbent() {
        let incumbent = line(1, Direction::Right, 4_000);
        let challenger = line(2, Direction::Up, 4_100);

        let chosen = select_incumbent_or_challenger(&challenger, &incumbent, false);

        assert_eq!(chosen.root_direction, Direction::Right);
    }

    #[test]
    fn meaningful_challenger_gain_replaces_incumbent() {
        let incumbent = line(1, Direction::Right, 4_000);
        let challenger = line(2, Direction::Up, 4_500);

        let chosen = select_incumbent_or_challenger(&challenger, &incumbent, false);

        assert_eq!(chosen.root_direction, Direction::Up);
    }

    #[test]
    fn survival_emergency_bypasses_switch_margin() {
        let incumbent = line(1, Direction::Right, 4_000);
        let challenger = line(2, Direction::Up, 4_001);

        let chosen = select_incumbent_or_challenger(&challenger, &incumbent, true);

        assert_eq!(chosen.root_direction, Direction::Up);
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
            .for_snake(&graph.node(graph.root()).state.our_snake_id)
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
