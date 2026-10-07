#![allow(dead_code)]

use std::time::{Duration, Instant};

use super::actor_priority::select_hunting_search_beam;
use super::beam::{BeamCheckpoint, BeamLine, LineId, LineTerminal, MAX_BEAM_DEPTH, ROUND_DEPTH};
use super::bounds::ValueBound;
use super::budget::SearchBudget;
use super::graph::{FutureGraph, NodeId, SearchError};
use super::maximin::{evaluate_continuations, evaluate_frontier, ContinuationEvaluation};

const FIRST_ROUND_ESTIMATE: Duration = Duration::from_millis(5);
const FINAL_SELECTION_RESERVE: Duration = Duration::from_millis(2);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BeamRoundStats {
    pub(crate) target_depth: u8,
    pub(crate) committed: bool,
    pub(crate) lines_attempted: u8,
    pub(crate) new_nodes: u32,
    pub(crate) new_edges: u32,
    pub(crate) elapsed_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BeamRoundOutcome {
    pub(crate) checkpoint: BeamCheckpoint,
    pub(crate) stats: BeamRoundStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BeamDeepeningStats {
    pub(crate) rounds_completed: u8,
    pub(crate) attempted_depth: u8,
    pub(crate) completed_depth: u8,
    pub(crate) elapsed_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BeamDeepeningResult {
    pub(crate) checkpoint: BeamCheckpoint,
    pub(crate) stats: BeamDeepeningStats,
}

pub(crate) fn deepen_checkpoint_once(
    graph: &mut FutureGraph,
    checkpoint: &BeamCheckpoint,
    budget: &SearchBudget,
) -> Result<BeamRoundOutcome, SearchError> {
    let target_depth = checkpoint
        .completed_depth
        .saturating_add(ROUND_DEPTH)
        .min(MAX_BEAM_DEPTH);
    let nodes_before = graph.node_count();
    let edges_before = graph.edge_count();
    let started = Instant::now();
    let lines_attempted = checkpoint
        .lines
        .iter()
        .filter(|line| line.terminal == LineTerminal::Running)
        .count()
        .try_into()
        .unwrap_or(u8::MAX);

    let mut working_lines = checkpoint.lines.clone();
    let mut next_line_id = working_lines
        .iter()
        .map(|line| line.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);

    // ROUND_DEPTH is intentionally applied as independent one-ply beam steps.
    // We prune after every layer so discarded futures never consume the next
    // layer's node-analysis budget.
    for _ in 0..ROUND_DEPTH {
        if !working_lines
            .iter()
            .any(|line| line.terminal == LineTerminal::Running)
        {
            break;
        }
        let Some(next_lines) =
            advance_lines_one_layer(graph, &working_lines, budget, &mut next_line_id)?
        else {
            return Ok(incomplete_outcome(
                checkpoint,
                target_depth,
                lines_attempted,
                nodes_before,
                edges_before,
                graph,
                started,
            ));
        };
        working_lines = next_lines;
    }

    if !checkpoint.can_commit(&working_lines) {
        return Ok(incomplete_outcome(
            checkpoint,
            target_depth,
            lines_attempted,
            nodes_before,
            edges_before,
            graph,
            started,
        ));
    }

    sort_lines(&mut working_lines);
    let committed = BeamCheckpoint::new(working_lines)
        .expect("committed beam round must contain at least one line");

    Ok(BeamRoundOutcome {
        checkpoint: committed,
        stats: BeamRoundStats {
            target_depth,
            committed: true,
            lines_attempted,
            new_nodes: graph
                .node_count()
                .saturating_sub(nodes_before)
                .try_into()
                .unwrap_or(u32::MAX),
            new_edges: graph.edge_count().saturating_sub(edges_before),
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
    })
}

fn advance_lines_one_layer(
    graph: &mut FutureGraph,
    lines: &[BeamLine],
    budget: &SearchBudget,
    next_line_id: &mut u32,
) -> Result<Option<Vec<BeamLine>>, SearchError> {
    let mut tips = Vec::<NodeId>::new();
    for line in lines {
        if line.terminal != LineTerminal::Running {
            continue;
        }
        let Some(tip) = line_tip(line) else {
            return Ok(None);
        };
        if !tips.contains(&tip) {
            tips.push(tip);
        }
    }

    for tip in tips {
        let expansion = graph.expand_prioritized_subtree(tip, 1, budget)?;
        if !expansion.completed {
            return Ok(None);
        }
    }

    let mut candidates = Vec::new();
    for line in lines {
        if line.terminal != LineTerminal::Running {
            candidates.push(line.clone());
            continue;
        }

        let Some(tip) = line_tip(line) else {
            return Ok(None);
        };

        let previous_frontier = evaluate_frontier(graph, tip, line.certainty);
        let continuations = evaluate_continuations(graph, tip, 1, line.certainty);
        for continuation in continuations
            .into_iter()
            .filter(|continuation| continuation.bound.is_exact())
        {
            let mut candidate = append_continuation(line, &previous_frontier, continuation);
            candidate.id = LineId(*next_line_id);
            *next_line_id = next_line_id.saturating_add(1);
            candidates.push(candidate);
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    let selected = select_hunting_search_beam(graph, &candidates);
    log::debug!(
        target: "search_diagnostics",
        "round turn={} candidates={} selected={}",
        graph.node(graph.root()).state.turn,
        format_lines(&candidates),
        format_lines(&selected)
    );
    if selected.is_empty() {
        Ok(None)
    } else {
        Ok(Some(selected))
    }
}

pub(crate) fn deepen_while_affordable(
    graph: &mut FutureGraph,
    initial: BeamCheckpoint,
    budget: &SearchBudget,
) -> Result<BeamDeepeningResult, SearchError> {
    let started = Instant::now();
    let soft_budget = budget.limited_to_soft_deadline();
    let mut checkpoint = initial;
    let mut rounds_completed = 0_u8;
    let mut attempted_depth = checkpoint.completed_depth;
    let mut next_estimate = FIRST_ROUND_ESTIMATE;

    loop {
        if !checkpoint.has_running_lines() || checkpoint.completed_depth >= MAX_BEAM_DEPTH {
            break;
        }

        let required = next_estimate.saturating_add(FINAL_SELECTION_RESERVE);
        if !soft_budget.can_afford_hard(required) {
            break;
        }

        attempted_depth = checkpoint
            .completed_depth
            .saturating_add(ROUND_DEPTH)
            .min(MAX_BEAM_DEPTH);
        let round_started = Instant::now();
        let outcome = deepen_checkpoint_once(graph, &checkpoint, &soft_budget)?;
        let round_elapsed = round_started.elapsed();

        if !outcome.stats.committed {
            break;
        }

        checkpoint = outcome.checkpoint;
        rounds_completed = rounds_completed.saturating_add(1);
        next_estimate = estimate_next_round(round_elapsed);
    }

    Ok(BeamDeepeningResult {
        stats: BeamDeepeningStats {
            rounds_completed,
            attempted_depth,
            completed_depth: checkpoint.completed_depth,
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
        checkpoint,
    })
}

fn format_lines(lines: &[BeamLine]) -> String {
    lines
        .iter()
        .map(|line| {
            format!(
                "{:?}:v={} ours={} opp={} depth={} term={:?} cert={:?}",
                line.root_direction,
                line.value,
                line.our_utility_total,
                line.opponent_utility_total,
                line.depth,
                line.terminal,
                line.certainty
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

pub(crate) fn append_continuation(
    line: &BeamLine,
    previous_frontier: &ContinuationEvaluation,
    continuation: ContinuationEvaluation,
) -> BeamLine {
    let prefix_our_utility = line
        .our_utility_total
        .saturating_sub(previous_frontier.our_utility_total);
    let prefix_opponent_utility = line
        .opponent_utility_total
        .saturating_sub(previous_frontier.opponent_utility_total);
    let prefix_value = prefix_our_utility.saturating_sub(prefix_opponent_utility);

    let path = line.path.concat(&continuation.path);

    let mut actor_utility_totals = line.actor_utility_totals.clone();
    for (actor_id, utility) in previous_frontier.actor_utility_totals.iter() {
        actor_utility_totals.add(actor_id, utility.saturating_neg());
    }
    for (actor_id, utility) in continuation.actor_utility_totals.iter() {
        actor_utility_totals.add(actor_id, *utility);
    }

    let (our_utility_total, opponent_utility_total, value, bound) = if continuation.terminal
        == LineTerminal::Running
        || continuation.certainty.is_provisional()
    {
        let ours = prefix_our_utility.saturating_add(continuation.our_utility_total);
        let opponents = prefix_opponent_utility.saturating_add(continuation.opponent_utility_total);
        (
            ours,
            opponents,
            ours.saturating_sub(opponents),
            shift_bound(continuation.bound, prefix_value),
        )
    } else {
        (
            continuation.our_utility_total,
            continuation.opponent_utility_total,
            continuation.value,
            continuation.bound,
        )
    };

    let prefix_benefit = line
        .benefit_total
        .saturating_sub(previous_frontier.benefit_total);
    let prefix_harm = line.harm_total.saturating_sub(previous_frontier.harm_total);

    BeamLine {
        id: line.id,
        root_direction: line.root_direction,
        depth: path.len().try_into().unwrap_or(u8::MAX),
        benefit_total: prefix_benefit.saturating_add(continuation.benefit_total),
        harm_total: prefix_harm.saturating_add(continuation.harm_total),
        our_utility_total,
        opponent_utility_total,
        actor_utility_totals,
        value,
        terminal: continuation.terminal,
        certainty: continuation.certainty,
        bound,
        path,
    }
}

fn line_tip(line: &BeamLine) -> Option<NodeId> {
    line.path.last().map(|step| step.child)
}

fn incomplete_outcome(
    checkpoint: &BeamCheckpoint,
    target_depth: u8,
    lines_attempted: u8,
    nodes_before: usize,
    edges_before: u32,
    graph: &FutureGraph,
    started: Instant,
) -> BeamRoundOutcome {
    BeamRoundOutcome {
        checkpoint: checkpoint.clone(),
        stats: BeamRoundStats {
            target_depth,
            committed: false,
            lines_attempted,
            new_nodes: graph
                .node_count()
                .saturating_sub(nodes_before)
                .try_into()
                .unwrap_or(u32::MAX),
            new_edges: graph.edge_count().saturating_sub(edges_before),
            elapsed_us: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        },
    }
}

fn shift_bound(bound: ValueBound, delta: i64) -> ValueBound {
    match bound {
        ValueBound::Exact(value) => ValueBound::Exact(value.saturating_add(delta)),
        ValueBound::LowerBound(value) => ValueBound::LowerBound(value.saturating_add(delta)),
        ValueBound::UpperBound(value) => ValueBound::UpperBound(value.saturating_add(delta)),
        ValueBound::Interval { lower, upper } => ValueBound::Interval {
            lower: lower.saturating_add(delta),
            upper: upper.saturating_add(delta),
        },
    }
}

fn estimate_next_round(previous: Duration) -> Duration {
    let micros = previous.as_micros();
    let scaled = micros
        .saturating_mul(5)
        .saturating_div(4)
        .saturating_add(2_000)
        .min(u128::from(u64::MAX));
    Duration::from_micros(scaled.try_into().unwrap_or(u64::MAX))
}

fn sort_lines(lines: &mut [BeamLine]) {
    lines.sort_by(|left, right| {
        right
            .bound
            .lower()
            .cmp(&left.bound.lower())
            .then_with(|| right.value.cmp(&left.value))
            .then_with(|| left.root_direction.rank().cmp(&right.root_direction.rank()))
            .then_with(|| left.id.cmp(&right.id))
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::search::beam::{select_seed_beam, BeamPath, BEAM_WIDTH, SEED_DEPTH};
    use crate::search::maximin::evaluate_seed_lines;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
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
        }
    }

    fn seeded() -> (FutureGraph, BeamCheckpoint) {
        let mut graph = FutureGraph::new(state());
        graph.expand_to_depth(SEED_DEPTH).unwrap();
        let candidates = evaluate_seed_lines(&graph, SEED_DEPTH).lines;
        let lines = select_seed_beam(&candidates);
        assert_eq!(lines.len(), BEAM_WIDTH);
        let checkpoint = BeamCheckpoint::new(lines).unwrap();
        (graph, checkpoint)
    }

    #[test]
    fn completed_round_advances_running_lines_by_two() {
        let (mut graph, checkpoint) = seeded();
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let outcome = deepen_checkpoint_once(&mut graph, &checkpoint, &budget).unwrap();

        assert!(outcome.stats.committed);
        assert_eq!(
            outcome.checkpoint.completed_depth,
            checkpoint.completed_depth + ROUND_DEPTH
        );
        assert!(outcome
            .checkpoint
            .lines
            .iter()
            .all(|line| line.bound.is_exact()));
        assert!(outcome.checkpoint.lines.iter().all(|line| {
            line.terminal != LineTerminal::Running
                || line.depth >= checkpoint.completed_depth + ROUND_DEPTH
        }));
    }

    #[test]
    fn expired_round_keeps_previous_checkpoint() {
        let (mut graph, checkpoint) = seeded();
        let budget = SearchBudget::for_duration(Duration::ZERO);

        let outcome = deepen_checkpoint_once(&mut graph, &checkpoint, &budget).unwrap();

        assert!(!outcome.stats.committed);
        assert_eq!(outcome.checkpoint, checkpoint);
    }

    #[test]
    fn repeated_rounds_only_commit_complete_depths() {
        let (mut graph, checkpoint) = seeded();
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let first = deepen_checkpoint_once(&mut graph, &checkpoint, &budget).unwrap();
        assert!(first.stats.committed);
        let second = deepen_checkpoint_once(&mut graph, &first.checkpoint, &budget).unwrap();
        assert!(second.stats.committed);

        assert_eq!(first.checkpoint.completed_depth, 5);
        assert_eq!(second.checkpoint.completed_depth, 7);
    }

    #[test]
    fn terminal_checkpoint_does_not_fake_additional_depth() {
        let mut graph = FutureGraph::new(state());
        let line = BeamLine::exact(
            1,
            crate::direction::Direction::Right,
            7,
            1_000_000_000,
            0,
            LineTerminal::Won,
        );
        let checkpoint = BeamCheckpoint::new(vec![line]).unwrap();
        let budget = SearchBudget::for_duration(Duration::from_secs(10));

        let result = deepen_while_affordable(&mut graph, checkpoint, &budget).unwrap();

        assert_eq!(result.stats.rounds_completed, 0);
        assert_eq!(result.stats.completed_depth, 7);
        assert_eq!(result.stats.attempted_depth, 7);
    }

    #[test]
    fn append_replaces_old_leaf_value_instead_of_double_counting_it() {
        let (graph, checkpoint) = seeded();
        let line = checkpoint.lines.first().unwrap().clone();
        let tip = line_tip(&line).unwrap();
        let previous_frontier = evaluate_frontier(&graph, tip, line.certainty);
        let continuation = ContinuationEvaluation {
            depth: 2,
            benefit_total: 900,
            harm_total: 300,
            our_utility_total: 900,
            opponent_utility_total: 300,
            actor_utility_totals: crate::evaluation::ActorVec::from_iter([
                (crate::simulation::state::ActorIndex::new(0).unwrap(), 900),
                (crate::simulation::state::ActorIndex::new(1).unwrap(), 300),
            ]),
            value: 600,
            terminal: LineTerminal::Running,
            certainty: crate::search::forecast::ForecastCertainty::Deterministic,
            bound: ValueBound::Exact(600),
            path: BeamPath::empty(),
        };

        let deepened = append_continuation(&line, &previous_frontier, continuation);

        assert_eq!(
            deepened.value,
            line.value
                .saturating_sub(previous_frontier.value)
                .saturating_add(600)
        );
        assert_eq!(
            deepened.our_utility_total,
            line.our_utility_total
                .saturating_sub(previous_frontier.our_utility_total)
                .saturating_add(900)
        );
        assert_eq!(
            deepened.opponent_utility_total,
            line.opponent_utility_total
                .saturating_sub(previous_frontier.opponent_utility_total)
                .saturating_add(300)
        );
    }
}
