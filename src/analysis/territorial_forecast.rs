//! Read-only forecast of competitive territory along a FutureGraph route.
//!
//! Each state uses the *same* weighted, tail-aware flood-fill parameters as
//! HobbsScore. A path is already selected by MAX/MIN; this module performs no
//! secondary search, graph expansion, action prediction, or move ranking.
//! The aggregation is diagnostic and not a proof of durable ownership.

use std::collections::HashMap;
use std::time::Instant;

use crate::direction::Direction;
use crate::evaluation::HobbsScoreParams;
use crate::search::forecast::ForecastCertainty;
use crate::search::graph::ResponseCoverage;
use crate::search::graph::{FutureGraph, NodeId};
use crate::search::path::FuturePath;
use crate::simulation::state::SimulatedGameState;

use super::TemporalTerritory;

/// Snapshot of the existing Hobbs territorial model for a graph state.
/// All values are ratios of weighted, competitively claimed cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerritorySnapshot {
    pub(crate) our_control_milli: i64,
    pub(crate) opponent_control_milli: i64,
    pub(crate) margin_milli: i64,
    pub(crate) our_claimed_cells: u32,
    pub(crate) opponent_claimed_cells: u32,
}

impl TerritorySnapshot {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Option<Self> {
        let params = HobbsScoreParams::STANDARD;
        let ours = state.actor_index(&state.our_snake_id)?;
        if !state.snake_at(ours)?.alive {
            return None;
        }
        let territory =
            TemporalTerritory::from_state(state, params.fill_cycles, params.cell_weights);
        Self::from_territory(state, &territory)
    }

    /// Reuses the Hobbs leaf flood-fill without computing another forecast.
    pub(crate) fn from_territory(
        state: &SimulatedGameState,
        territory: &TemporalTerritory,
    ) -> Option<Self> {
        let ours = state.actor_index(&state.our_snake_id)?;
        if !state.snake_at(ours)?.alive {
            return None;
        }
        let our_control_milli = territory.territory_ratio_milli(ours);
        let (opponent_control_milli, opponent_claimed_cells) = state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive && snake.id != state.our_snake_id)
            .filter_map(|(actor, _)| {
                let value = state.snake_at(crate::simulation::state::ActorIndex::new(actor)?)?;
                if !value.alive {
                    return None;
                }
                Some((
                    territory
                        .territory_ratio_milli(crate::simulation::state::ActorIndex::new(actor)?),
                    territory.claimed_cells.get(actor).copied().unwrap_or(0),
                ))
            })
            .max_by_key(|(ratio, _)| *ratio)
            .unwrap_or((0, 0));
        Some(Self {
            our_control_milli,
            opponent_control_milli,
            margin_milli: our_control_milli.saturating_sub(opponent_control_milli),
            our_claimed_cells: territory
                .claimed_cells
                .get(ours.as_usize())
                .copied()
                .unwrap_or(0),
            opponent_claimed_cells,
        })
    }
}

/// A sampled *one-path* forecast. Not a guarantee across unexpanded replies;
/// the path comes from the pessimistic line chosen by the current MAX/MIN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerritoryTrajectory {
    pub(crate) samples: u8,
    pub(crate) first_margin_milli: i64,
    pub(crate) last_margin_milli: i64,
    pub(crate) minimum_margin_milli: i64,
    pub(crate) mean_margin_milli: i64,
    pub(crate) certainty: ForecastCertainty,
}

impl TerritoryTrajectory {
    /// Normalize by the number of observations. Do not sum repeated control
    /// of identical cells into extra reward, and do not make a one-turn dip a
    /// terminal loss.
    fn summarize(samples: &[TerritorySnapshot], certainty: ForecastCertainty) -> Option<Self> {
        let first = samples.first()?;
        let last = samples.last()?;
        let total = samples
            .iter()
            .fold(0_i64, |sum, sample| sum.saturating_add(sample.margin_milli));
        let count = i64::try_from(samples.len()).ok()?;
        Some(Self {
            samples: u8::try_from(samples.len()).unwrap_or(u8::MAX),
            first_margin_milli: first.margin_milli,
            last_margin_milli: last.margin_milli,
            minimum_margin_milli: samples
                .iter()
                .map(|sample| sample.margin_milli)
                .min()
                .unwrap_or(0),
            mean_margin_milli: total.saturating_div(count),
            certainty,
        })
    }

    /// Consume the MIN/MAX-selected sequence without walking another game tree.
    /// The cache is scoped to this diagnostic and never survives a compacted
    /// node-ID remap. Unknown/time-exhausted paths are omitted, not fabricated.
    pub(crate) fn from_graph_path(
        graph: &FutureGraph,
        path: &FuturePath,
        certainty: ForecastCertainty,
        deadline: Instant,
        cache: &mut HashMap<NodeId, TerritorySnapshot>,
    ) -> Option<Self> {
        let mut ids = vec![graph.root()];
        path.for_each_step(|step| ids.push(step.child));
        let mut samples = Vec::with_capacity(ids.len());
        for id in ids {
            if Instant::now() >= deadline {
                return None;
            }
            let snapshot = if let Some(&cached) = cache.get(&id) {
                cached
            } else {
                let snapshot = TerritorySnapshot::from_state(&graph.node(id).state)?;
                cache.insert(id, snapshot);
                snapshot
            };
            samples.push(snapshot);
        }
        Self::summarize(&samples, certainty)
    }
}

/// Observed change in raw Hobbs-claimed cells along ONE cached reply chain.
/// A sharp drop indicates lost predicted control, not a proven closed gateway.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct TerritorialDrop {
    pub(crate) max_absolute_drop: u32,
    pub(crate) max_relative_drop_milli: u16,
    pub(crate) first_drop_ply: Option<u8>,
    /// Ply of the event supplying max_absolute_drop and max_relative_drop_milli.
    pub(crate) max_drop_ply: Option<u8>,
    pub(crate) opponent_gain: i32,
    /// None means the sampled chain ended before a recovery could be checked.
    pub(crate) recovered_next_ply: Option<bool>,
}

impl TerritorialDrop {
    const MIN_ABSOLUTE: u32 = 8;
    const MIN_RELATIVE_MILLI: u16 = 400;

    pub(crate) fn significant(self) -> bool {
        self.max_absolute_drop >= Self::MIN_ABSOLUTE
            && self.max_relative_drop_milli >= Self::MIN_RELATIVE_MILLI
            && self.recovered_next_ply == Some(false)
    }

    fn from_snapshots(states: &[TerritorySnapshot]) -> Option<Self> {
        if states.len() < 2 {
            return None;
        }
        let mut result = Self::default();
        for (index, pair) in states.windows(2).enumerate() {
            let previous = pair[0];
            let current = pair[1];
            let lost = previous
                .our_claimed_cells
                .saturating_sub(current.our_claimed_cells);
            let ply = u8::try_from(index + 1).unwrap_or(u8::MAX);
            let relative = lost
                .saturating_mul(1000)
                .saturating_div(previous.our_claimed_cells.max(1))
                .min(1000) as u16;
            if lost >= Self::MIN_ABSOLUTE
                && relative >= Self::MIN_RELATIVE_MILLI
                && result.first_drop_ply.is_none()
            {
                result.first_drop_ply = Some(ply);
            }
            if (relative, lost) > (result.max_relative_drop_milli, result.max_absolute_drop) {
                result.max_absolute_drop = lost;
                result.max_relative_drop_milli = relative;
                result.max_drop_ply = Some(ply);
                result.opponent_gain = (i64::from(current.opponent_claimed_cells)
                    - i64::from(previous.opponent_claimed_cells))
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX))
                    as i32;
                result.recovered_next_ply = states.get(index + 2).map(|next| {
                    next.our_claimed_cells.saturating_mul(10)
                        >= previous.our_claimed_cells.saturating_mul(8)
                });
            }
        }
        Some(result)
    }
}

/// Small, explicitly sampled ranking hint for a root move. It is NOT a
/// certificate: alpha-beta may have left other enemy responses unexplored.
/// MAX/MIN still evaluates every reply required to finish a search depth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerritoryDirectionSample {
    pub(crate) worst_mean_margin_milli: i64,
    pub(crate) worst_minimum_margin_milli: i64,
    pub(crate) examined_replies: u8,
    pub(crate) max_sampled_depth: u8,
    pub(crate) min_sampled_depth: u8,
    pub(crate) coverage: ResponseCoverage,
    /// Accumulated uncertainty from every traversed FutureGraph edge.
    pub(crate) certainty: ForecastCertainty,
    /// False for cached preferred-reply chains: preferences may originate
    /// from separate, partially-pruned iterative-deepening horizons.
    pub(crate) adversarially_complete: bool,
    /// Largest loss observed between adjacent states of a sampled reply.
    pub(crate) territorial_drop: Option<TerritorialDrop>,
    /// At least one sampled reply reached a terminal loss; do not infer
    /// a safe trajectory from the surviving prefix alone.
    pub(crate) terminal_reply_seen: bool,
}

impl TerritoryDirectionSample {
    pub(crate) const MAX_NEW_SNAPSHOTS: usize = 24;
    const MAX_REPLIES_PER_DIRECTION: usize = 2;
    const MAX_CACHED_PLIES: usize = 4;

    /// A bounded, immutable root-level comparison of cached enemy replies.
    /// Follows only the continuation/reply hints previously recorded by
    /// minimax; an absent continuation terminates the observation.
    pub(crate) fn from_cached_replies(
        graph: &FutureGraph,
        direction: Direction,
        cache: &mut HashMap<NodeId, TerritorySnapshot>,
        cached_leaf: impl Fn(NodeId) -> Option<TerritorySnapshot>,
        collect_drop: bool,
        deadline: Instant,
    ) -> Option<Self> {
        let root = graph.root();
        let mut edges = graph.known_responses(root, direction);
        if edges.is_empty() {
            return None;
        }
        if let Some(reply) = graph.node(root).preferred_reply.get(&direction) {
            edges.sort_by_key(|edge| edge.joint_action != *reply);
        }
        let mut result: Option<Self> = None;
        let mut terminal_reply_seen = false;
        for edge in edges.iter().take(Self::MAX_REPLIES_PER_DIRECTION) {
            let mut chain = vec![root, edge.child];
            let mut chain_certainty = ForecastCertainty::Deterministic.after(edge.forecast_delta);
            let mut current = edge.child;
            for _ in 1..Self::MAX_CACHED_PLIES {
                let node = graph.node(current);
                let Some(preferred) = node.preferred_direction else {
                    break;
                };
                let Some(joint) = node.preferred_reply.get(&preferred) else {
                    break;
                };
                let Some(next) = graph
                    .known_responses(current, preferred)
                    .into_iter()
                    .find(|cached| cached.joint_action == *joint)
                else {
                    break;
                };
                if chain.contains(&next.child) {
                    break; // Transpositions can form loops; don't resample forever.
                }
                chain_certainty = chain_certainty.after(next.forecast_delta);
                current = next.child;
                chain.push(current);
            }

            let mut snapshots = Vec::with_capacity(chain.len());
            let mut terminated = false;
            for &id in &chain {
                if Instant::now() >= deadline {
                    return result;
                }
                // Only V3 needs this explicit distinction. The validated
                // primary policy retains its original sampling behavior.
                if collect_drop
                    && graph
                        .node(id)
                        .state
                        .snake(&graph.node(id).state.our_snake_id)
                        .is_some_and(|snake| !snake.alive)
                {
                    terminal_reply_seen = true;
                    terminated = true;
                    break;
                }
                let snapshot = if let Some(&cached) = cache.get(&id) {
                    cached
                } else if let Some(snapshot) = cached_leaf(id) {
                    snapshot
                } else {
                    if cache.len() >= Self::MAX_NEW_SNAPSHOTS {
                        return result;
                    }
                    let snapshot = TerritorySnapshot::from_state(&graph.node(id).state)?;
                    cache.insert(id, snapshot);
                    snapshot
                };
                snapshots.push(snapshot);
            }
            // A terminal reply may supply a valid prefix but is never
            // accepted as evidence of a persistent, survivable territory.
            if snapshots.is_empty() {
                continue;
            }
            let trajectory = TerritoryTrajectory::summarize(&snapshots, chain_certainty)?;
            // No drop analysis at all in the validated primary mode.
            let drop = if collect_drop && !terminated {
                TerritorialDrop::from_snapshots(&snapshots)
            } else {
                None
            };
            let depth = trajectory.samples.saturating_sub(1);
            let sample = result.get_or_insert(Self {
                worst_mean_margin_milli: trajectory.mean_margin_milli,
                worst_minimum_margin_milli: trajectory.minimum_margin_milli,
                examined_replies: 0,
                max_sampled_depth: 0,
                min_sampled_depth: u8::MAX,
                coverage: graph.response_coverage(root, direction),
                certainty: ForecastCertainty::Deterministic,
                adversarially_complete: false,
                territorial_drop: drop,
                terminal_reply_seen,
            });
            if let Some(candidate) = drop {
                if sample.territorial_drop.is_none_or(|previous| {
                    (
                        candidate.max_relative_drop_milli,
                        candidate.max_absolute_drop,
                    ) > (previous.max_relative_drop_milli, previous.max_absolute_drop)
                }) {
                    sample.territorial_drop = Some(candidate);
                }
            }
            sample.examined_replies = sample.examined_replies.saturating_add(1);
            sample.max_sampled_depth = sample.max_sampled_depth.max(depth);
            sample.min_sampled_depth = sample.min_sampled_depth.min(depth);
            if trajectory.certainty.is_provisional() {
                sample.certainty = ForecastCertainty::FoodProvisional;
            }
            sample.worst_mean_margin_milli = sample
                .worst_mean_margin_milli
                .min(trajectory.mean_margin_milli);
            sample.worst_minimum_margin_milli = sample
                .worst_minimum_margin_milli
                .min(trajectory.minimum_margin_milli);
        }
        if let Some(sample) = result.as_mut() {
            sample.terminal_reply_seen |= terminal_reply_seen;
        }
        result
    }

    /// The sample is an ordering hint, not a new Hobbs score or MAX/MIN bound.
    pub(crate) fn ordering_key(self) -> (i64, i64) {
        (
            self.worst_mean_margin_milli,
            self.worst_minimum_margin_milli,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::budget::SearchBudget;
    use crate::search::graph::ResponseLookup;
    use crate::search::path::FutureStep;
    use crate::simulation::state::{RulesContext, SimulatedSnake};
    use crate::Coord;
    use std::time::Duration;

    fn board() -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![Coord { x: 3, y: 3 }],
            hazards: Vec::new(),
            our_snake_id: "ours".into(),
            snakes: vec![
                SimulatedSnake {
                    id: "ours".into(),
                    health: 90,
                    alive: true,
                    body: vec![Coord { x: 1, y: 2 }, Coord { x: 1, y: 1 }],
                },
                SimulatedSnake {
                    id: "enemy".into(),
                    health: 90,
                    alive: true,
                    body: vec![Coord { x: 5, y: 5 }, Coord { x: 5, y: 4 }],
                },
            ],
            rules: RulesContext {
                name: "standard".into(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn snapshot_uses_exact_hobbs_territory_ratio() {
        let state = board();
        let snapshot = TerritorySnapshot::from_state(&state).unwrap();
        let params = HobbsScoreParams::STANDARD;
        let territory =
            TemporalTerritory::from_state(&state, params.fill_cycles, params.cell_weights);
        let actor = state.actor_index("ours").unwrap();
        assert_eq!(
            snapshot.our_control_milli,
            territory.territory_ratio_milli(actor)
        );
        assert_eq!(
            snapshot.margin_milli,
            snapshot.our_control_milli - snapshot.opponent_control_milli
        );
    }

    #[test]
    fn territorial_drop_only_compares_consecutive_states_in_one_chain() {
        let snapshot = |ours, theirs| TerritorySnapshot {
            our_control_milli: 500,
            opponent_control_milli: 500,
            margin_milli: 0,
            our_claimed_cells: ours,
            opponent_claimed_cells: theirs,
        };
        let loss =
            TerritorialDrop::from_snapshots(&[snapshot(30, 20), snapshot(8, 39), snapshot(9, 38)])
                .unwrap();
        assert_eq!(loss.max_absolute_drop, 22);
        assert_eq!(loss.max_relative_drop_milli, 733);
        assert_eq!(loss.first_drop_ply, Some(1));
        assert_eq!(loss.max_drop_ply, Some(1));
        assert_eq!(loss.opponent_gain, 19);
        assert_eq!(loss.recovered_next_ply, Some(false));
        assert!(loss.significant());

        let recovered =
            TerritorialDrop::from_snapshots(&[snapshot(30, 20), snapshot(8, 39), snapshot(29, 23)])
                .unwrap();
        assert_eq!(recovered.recovered_next_ply, Some(true));
        assert!(!recovered.significant());

        let unknown = TerritorialDrop::from_snapshots(&[snapshot(30, 20), snapshot(8, 39)])
            .unwrap();
        assert_eq!(unknown.recovered_next_ply, None);
        assert!(!unknown.significant());

        // A later, larger drop must keep its own opponent gain and ply.
        let multiple = TerritorialDrop::from_snapshots(&[
            snapshot(35, 15),
            snapshot(22, 23),
            snapshot(32, 17),
            snapshot(7, 38),
            snapshot(6, 40),
        ])
        .unwrap();
        assert_eq!(multiple.first_drop_ply, Some(1));
        assert_eq!(multiple.max_drop_ply, Some(3));
        assert_eq!(multiple.max_absolute_drop, 25);
        assert_eq!(multiple.opponent_gain, 21);
        assert_eq!(multiple.recovered_next_ply, Some(false));
        assert!(multiple.significant());
        assert!(TerritorialDrop::from_snapshots(&[snapshot(30, 20)]).is_none());
    }

    #[test]
    fn trajectory_uses_mean_not_cumulative_score() {
        let sample = |margin| TerritorySnapshot {
            our_control_milli: 500 + margin / 2,
            opponent_control_milli: 500 - margin / 2,
            margin_milli: margin,
            our_claimed_cells: 12,
            opponent_claimed_cells: 12,
        };
        let result = TerritoryTrajectory::summarize(
            &[sample(200), sample(-100), sample(100)],
            ForecastCertainty::FoodProvisional,
        )
        .unwrap();
        assert_eq!(result.mean_margin_milli, 66);
        assert_eq!(result.minimum_margin_milli, -100);
        assert_eq!(result.first_margin_milli, 200);
        assert_eq!(result.last_margin_milli, 100);
        assert_eq!(result.samples, 3);
        assert!(result.certainty.is_provisional());
    }

    #[test]
    fn compare_cached_root_replies_without_altering_graph() {
        let mut graph = FutureGraph::new_beam(board());
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let root = graph.root();
        let directions = graph.available_directions(root);
        assert!(directions.len() >= 2);
        for direction in directions.iter().take(2) {
            assert!(matches!(
                graph.next_response(root, *direction, 0, &budget).unwrap(),
                ResponseLookup::Edge(_)
            ));
        }
        let before_edges = graph.edge_count();
        let before_nodes = graph.node_count();
        let mut cache = HashMap::new();
        let deadline = Instant::now() + Duration::from_secs(1);
        let first = TerritoryDirectionSample::from_cached_replies(
            &graph,
            directions[0],
            &mut cache,
            |_| None,
            false,
            deadline,
        )
        .unwrap();
        let second = TerritoryDirectionSample::from_cached_replies(
            &graph,
            directions[1],
            &mut cache,
            |_| None,
            false,
            deadline,
        )
        .unwrap();
        assert!(first.examined_replies >= 1 && second.examined_replies >= 1);
        assert_eq!(first.max_sampled_depth, 1);
        assert!(cache.len() <= TerritoryDirectionSample::MAX_NEW_SNAPSHOTS);
        assert_eq!(graph.edge_count(), before_edges);
        assert_eq!(graph.node_count(), before_nodes);
        let repeated = TerritoryDirectionSample::from_cached_replies(
            &graph,
            directions[0],
            &mut cache,
            |_| None,
            false,
            deadline,
        )
        .unwrap();
        assert_eq!(repeated, first);
        assert_eq!(repeated.coverage, ResponseCoverage::Partial);
    }

    #[test]
    fn cached_forecasts_do_not_claim_adversarial_completeness() {
        let mut graph = FutureGraph::new_beam(board());
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let root = graph.root();
        let direction = graph.available_directions(root)[0];
        assert!(matches!(
            graph.next_response(root, direction, 0, &budget).unwrap(),
            ResponseLookup::Edge(_)
        ));
        let sample = TerritoryDirectionSample::from_cached_replies(
            &graph,
            direction,
            &mut HashMap::new(),
            |_| None,
            false,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert!(!sample.adversarially_complete);
        assert_eq!(sample.min_sampled_depth, sample.max_sampled_depth);
        assert_eq!(sample.coverage, ResponseCoverage::Partial);
    }

    #[test]
    fn graph_path_forecast_never_expands_an_edge() {
        let mut graph = FutureGraph::new_beam(board());
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let root = graph.root();
        let direction = graph.available_directions(root)[0];
        let edge = match graph.next_response(root, direction, 0, &budget).unwrap() {
            ResponseLookup::Edge(edge) => edge,
            _ => panic!("expected a joint-action edge"),
        };
        let path = FuturePath::single(FutureStep {
            node: root,
            joint_action: edge.joint_action,
            child: edge.child,
        });
        let before = graph.edge_count();
        let mut cache = HashMap::new();
        let forecast = TerritoryTrajectory::from_graph_path(
            &graph,
            &path,
            ForecastCertainty::Deterministic,
            Instant::now() + Duration::from_secs(1),
            &mut cache,
        )
        .unwrap();
        assert_eq!(forecast.samples, 2);
        assert_eq!(cache.len(), 2);
        assert_eq!(graph.edge_count(), before);
        assert_eq!(
            TerritoryTrajectory::from_graph_path(
                &graph,
                &path,
                ForecastCertainty::Deterministic,
                Instant::now() + Duration::from_secs(1),
                &mut cache,
            ),
            Some(forecast)
        );
        assert_eq!(cache.len(), 2);
    }
}
