//! Bounded, read-only escape diagnostics over already-expanded FutureGraph paths.
//!
//! A single chosen MAX/MIN line is NOT a certificate that all opponent
//! continuations are safe. Horizons here describe observation windows, not
//! independent tree searches or guaranteed survival beyond the window.

use std::collections::HashMap;
use std::time::Instant;

use crate::search::forecast::ForecastCertainty;
use crate::search::graph::{FutureGraph, NodeId};
use crate::search::path::FuturePath;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;

const BASE_ESCAPE_HORIZON: u8 = 4;
const NARROW_ESCAPE_HORIZON: u8 = 6;
const CONTESTED_ESCAPE_HORIZON: u8 = 8;
const MAX_ESCAPE_HORIZON: u8 = 10;
const MAX_CACHED_ESCAPE_STATES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EscapeEvidenceQuality {
    /// Search evidence had complete response coverage with fixed known food.
    /// This does NOT establish a guaranteed escape for the representative path.
    CompleteDeterministic,
    /// Complete response coverage, but the future food schedule is uncertain.
    CompleteConditionalFood,
    /// Alpha-beta cuts or incomplete response coverage make the evidence partial.
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EscapeSnapshot {
    pub(crate) immediate_exits: u8,
    /// Exits reachable on the same turn by a rival of equal/greater length.
    /// This is only a possible head-to-head threat, NOT a forced collision.
    pub(crate) contested_exits: u8,
    pub(crate) observed_dead: bool,
}

impl EscapeSnapshot {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Option<Self> {
        let ours = state.snake(&state.our_snake_id)?;
        if !ours.alive {
            return Some(Self {
                immediate_exits: 0,
                contested_exits: 0,
                observed_dead: true,
            });
        }
        let head = ours.head()?;
        let mobility = MobilityAnalysis::from_state(state);
        let legal = mobility.deterministic_moves_for(state, &state.our_snake_id);
        let mut contested = 0_u8;
        for our_direction in legal.iter() {
            let destination = our_direction.apply(head);
            let enemy_can_contest = state
                .snakes
                .iter()
                .filter(|enemy| {
                    enemy.alive && enemy.id != state.our_snake_id && enemy.length() >= ours.length()
                })
                .filter_map(|enemy| enemy.head().map(|enemy_head| (enemy, enemy_head)))
                .any(|(enemy, enemy_head)| {
                    mobility
                        .deterministic_moves_for(state, &enemy.id)
                        .iter()
                        .any(|direction| direction.apply(enemy_head) == destination)
                });
            if enemy_can_contest {
                contested = contested.saturating_add(1);
            }
        }
        Some(Self {
            immediate_exits: legal.len(),
            contested_exits: contested,
            observed_dead: false,
        })
    }

    fn suggested_horizon(self) -> u8 {
        if self.observed_dead
            || self.immediate_exits == 0
            || (self.immediate_exits == 1 && self.contested_exits > 0)
        {
            MAX_ESCAPE_HORIZON
        } else if self.contested_exits > 0 {
            CONTESTED_ESCAPE_HORIZON
        } else if self.immediate_exits <= 2 {
            NARROW_ESCAPE_HORIZON
        } else {
            BASE_ESCAPE_HORIZON
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EscapeEvidence {
    /// States observed on ONE cached minimax path, including its root.
    pub(crate) observed_states: u8,
    pub(crate) requested_horizon: u8,
    /// Number of future plies actually examined (root has ply zero).
    pub(crate) observed_horizon: u8,
    /// Number of path plies available, not the number of alternative replies.
    pub(crate) available_plies: u8,
    /// False when the cached path, deadline or work cap stopped observation.
    pub(crate) reached_requested_horizon: bool,
    pub(crate) minimum_exits: u8,
    pub(crate) minimum_uncontested_exits: u8,
    pub(crate) first_narrow_ply: Option<u8>,
    pub(crate) first_contested_ply: Option<u8>,
    pub(crate) first_all_exits_contested_ply: Option<u8>,
    pub(crate) first_no_exit_ply: Option<u8>,
    pub(crate) first_observed_death_ply: Option<u8>,
    pub(crate) quality: EscapeEvidenceQuality,
}

impl EscapeEvidence {
    fn new(quality: EscapeEvidenceQuality, available_plies: u8) -> Self {
        Self {
            observed_states: 0,
            requested_horizon: BASE_ESCAPE_HORIZON,
            observed_horizon: 0,
            available_plies,
            reached_requested_horizon: false,
            minimum_exits: u8::MAX,
            minimum_uncontested_exits: u8::MAX,
            first_narrow_ply: None,
            first_contested_ply: None,
            first_all_exits_contested_ply: None,
            first_no_exit_ply: None,
            first_observed_death_ply: None,
            quality,
        }
    }

    /// Expand the observation horizon ONLY if an already-observed state shows
    /// narrowing or a legal same-turn contest. This does not expand the graph.
    fn observe(&mut self, ply: u8, snapshot: EscapeSnapshot) {
        self.observed_states = self.observed_states.saturating_add(1);
        self.observed_horizon = ply;
        self.requested_horizon = self.requested_horizon.max(snapshot.suggested_horizon());
        self.minimum_exits = self.minimum_exits.min(snapshot.immediate_exits);
        let uncontested = snapshot
            .immediate_exits
            .saturating_sub(snapshot.contested_exits);
        self.minimum_uncontested_exits = self.minimum_uncontested_exits.min(uncontested);
        if snapshot.immediate_exits <= 1 {
            self.first_narrow_ply.get_or_insert(ply);
        }
        if snapshot.contested_exits > 0 {
            self.first_contested_ply.get_or_insert(ply);
        }
        if snapshot.immediate_exits > 0 && uncontested == 0 {
            self.first_all_exits_contested_ply.get_or_insert(ply);
        }
        if snapshot.immediate_exits == 0 {
            self.first_no_exit_ply.get_or_insert(ply);
        }
        if snapshot.observed_dead {
            self.first_observed_death_ply.get_or_insert(ply);
        }
    }

    /// Inspect a prefix (at most ten plies) of ONE stored Minimax trajectory.
    /// Paths longer than ten plies are intentionally TRUNCATED rather than
    /// discarded; short paths are explicitly marked as horizon-incomplete.
    /// The cache belongs to a single search and is invalidated on graph reroot.
    pub(crate) fn from_graph_path(
        graph: &FutureGraph,
        path: &FuturePath,
        certainty: ForecastCertainty,
        adversarially_complete: bool,
        deadline: Instant,
        cache: &mut HashMap<NodeId, EscapeSnapshot>,
    ) -> Option<Self> {
        let mut ids = vec![graph.root()];
        let mut previous = graph.root();
        let mut connected = true;
        let mut available_plies = 0_u8;
        path.for_each_step(|step| {
            available_plies = available_plies.saturating_add(1);
            if !connected || ids.len() > usize::from(MAX_ESCAPE_HORIZON) {
                return;
            }
            if step.node != previous {
                connected = false;
            } else {
                ids.push(step.child);
                previous = step.child;
            }
        });
        if !connected {
            return None;
        }

        let quality = if !adversarially_complete {
            EscapeEvidenceQuality::Partial
        } else if certainty.is_provisional() {
            EscapeEvidenceQuality::CompleteConditionalFood
        } else {
            EscapeEvidenceQuality::CompleteDeterministic
        };
        let mut result = Self::new(quality, available_plies);
        for (ply, id) in ids.into_iter().enumerate() {
            let ply = u8::try_from(ply).ok()?;
            if ply > result.requested_horizon {
                break;
            }
            if Instant::now() >= deadline {
                return None;
            }
            let snapshot = if let Some(&stored) = cache.get(&id) {
                stored
            } else {
                if cache.len() >= MAX_CACHED_ESCAPE_STATES {
                    return None;
                }
                let snapshot = EscapeSnapshot::from_state(&graph.node(id).state)?;
                cache.insert(id, snapshot);
                snapshot
            };
            result.observe(ply, snapshot);
            if snapshot.observed_dead {
                break;
            }
        }
        result.reached_requested_horizon = result.observed_horizon >= result.requested_horizon;
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::analysis::replay_fixtures::late_seed_20261003;

    fn snapshot(exits: u8, contested: u8) -> EscapeSnapshot {
        EscapeSnapshot {
            immediate_exits: exits,
            contested_exits: contested,
            observed_dead: false,
        }
    }

    #[test]
    fn adaptive_horizon_extends_only_after_observed_risk() {
        let mut evidence = EscapeEvidence::new(EscapeEvidenceQuality::Partial, 10);
        evidence.observe(0, snapshot(3, 0));
        assert_eq!(evidence.requested_horizon, 4);
        evidence.observe(4, snapshot(2, 0));
        assert_eq!(evidence.requested_horizon, 6);
        evidence.observe(5, snapshot(2, 1));
        assert_eq!(evidence.requested_horizon, 8);
        evidence.observe(7, snapshot(1, 1));
        assert_eq!(evidence.requested_horizon, 10);
        assert_eq!(evidence.first_narrow_ply, Some(7));
        assert_eq!(evidence.first_all_exits_contested_ply, Some(7));
        assert_eq!(evidence.minimum_uncontested_exits, 0);
    }

    #[test]
    fn uncertain_or_open_positions_never_claim_guaranteed_survival() {
        let mut evidence = EscapeEvidence::new(EscapeEvidenceQuality::Partial, 3);
        for ply in 0..=3 {
            evidence.observe(ply, snapshot(3, 0));
        }
        evidence.reached_requested_horizon =
            evidence.observed_horizon >= evidence.requested_horizon;
        assert_eq!(evidence.requested_horizon, 4);
        assert!(!evidence.reached_requested_horizon);
        assert_eq!(evidence.quality, EscapeEvidenceQuality::Partial);
    }

    #[test]
    fn seed_20261003_last_escape_is_contested_by_larger_hobbs() {
        let final_state = late_seed_20261003(333);
        let observation = EscapeSnapshot::from_state(&final_state).unwrap();
        assert_eq!(observation.immediate_exits, 1);
        assert_eq!(observation.contested_exits, 1);
        assert!(!observation.observed_dead);
        assert_eq!(observation.suggested_horizon(), 10);
    }

    #[test]
    fn seed_20261003_detects_narrowing_before_final_collision() {
        let early = EscapeSnapshot::from_state(&late_seed_20261003(325)).unwrap();
        let late = EscapeSnapshot::from_state(&late_seed_20261003(333)).unwrap();
        assert!(early.immediate_exits > late.immediate_exits);
        assert!(early.suggested_horizon() <= late.suggested_horizon());
    }

    #[test]
    fn inspection_does_not_expand_graph_and_uses_cached_snapshots() {
        let graph = FutureGraph::new_beam(late_seed_20261003(333));
        let nodes = graph.node_count();
        let edges = graph.edge_count();
        let path = FuturePath::empty();
        let mut cache = HashMap::new();
        let deadline = Instant::now() + Duration::from_secs(1);
        let evidence = EscapeEvidence::from_graph_path(
            &graph,
            &path,
            ForecastCertainty::FoodProvisional,
            false,
            deadline,
            &mut cache,
        )
        .unwrap();
        assert_eq!(evidence.quality, EscapeEvidenceQuality::Partial);
        assert_eq!(evidence.first_narrow_ply, Some(0));
        assert_eq!(evidence.first_contested_ply, Some(0));
        assert_eq!(evidence.first_all_exits_contested_ply, Some(0));
        assert_eq!(evidence.requested_horizon, 10);
        assert!(!evidence.reached_requested_horizon);
        assert_eq!(evidence.observed_states, 1);
        assert_eq!(cache.len(), 1);
        assert_eq!(graph.node_count(), nodes);
        assert_eq!(graph.edge_count(), edges);
        let repeat = EscapeEvidence::from_graph_path(
            &graph,
            &path,
            ForecastCertainty::FoodProvisional,
            false,
            deadline,
            &mut cache,
        )
        .unwrap();
        assert_eq!(repeat, evidence);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn complete_path_with_provisional_food_is_not_deterministic() {
        let graph = FutureGraph::new_beam(late_seed_20261003(325));
        let path = FuturePath::empty();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut cache = HashMap::new();
        let evidence = EscapeEvidence::from_graph_path(
            &graph,
            &path,
            ForecastCertainty::FoodProvisional,
            true,
            deadline,
            &mut cache,
        )
        .unwrap();
        assert_eq!(
            evidence.quality,
            EscapeEvidenceQuality::CompleteConditionalFood
        );
        assert!(!evidence.reached_requested_horizon);
    }
}
