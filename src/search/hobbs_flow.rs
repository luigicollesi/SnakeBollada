//! Hobbs-style paranoid search over the existing FutureGraph.
//!
//! The route value comes from the reached state, not the sum of Food,
//! Hunting and Survival transition scores. Every enumerated enemy response
//! participates in MIN; opponent intent only affects expansion order.
//! Iterative deepening commits the previous complete depth on timeout.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::analysis::{
    adversarial_order, AdversarialCorridorOrder, CorridorOutlook, TemporalTerritory,
    TerritoryDirectionSample, TerritorySnapshot,
};
use crate::direction::Direction;
use crate::evaluation::{
    assess_survival_state, evaluate_hobbs_state, HobbsScoreParams, StateScore, TrapAssessment,
};
#[cfg(test)]
use crate::simulation::joint_action::JointAction;
use crate::simulation::mobility::MobilityAnalysis;

use super::budget::SearchBudget;
use super::forecast::ForecastCertainty;
use super::graph::{
    CachedSearchValue, FutureGraph, NodeId, ResponseLookup, SearchEdge, SearchError,
};
use super::path::{FuturePath, FutureStep, MAX_SEARCH_DEPTH};

#[derive(Debug, Clone)]
pub(crate) struct HobbsSearchResult {
    pub(crate) direction: Direction,
    pub(crate) score: StateScore,
    pub(crate) survival: TrapAssessment,
    pub(crate) completed_depth: u8,
    pub(crate) root_directions: usize,
    pub(crate) certainty: ForecastCertainty,
    /// Actual MAX/MIN-selected line; no additional search is required for
    /// shadow territorial diagnostics, even when iterative deepening stops.
    pub(crate) path: FuturePath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RouteRank {
    score: StateScore,
    safety: TrapAssessment,
    // Tactical pressure is only a secondary, non-terminal ordering criterion.
    // It can never turn an optimistic corridor forecast into a proven win.
    pressure: u8,
}

impl RouteRank {
    fn tier(self) -> u8 {
        match (self.score, self.safety) {
            (StateScore::Win, _) => 6,
            (StateScore::Loss, _) | (_, TrapAssessment::ProvenTrap) => 0,
            (StateScore::Tie, _) => 1,
            (_, TrapAssessment::Viable) => 5,
            (_, TrapAssessment::Unknown) => 4,
            (_, TrapAssessment::Constrained) => 3,
            (_, TrapAssessment::ForcedCorridor) => 2,
        }
    }

    fn safety_rank(self) -> u8 {
        match self.safety {
            TrapAssessment::Viable => 4,
            TrapAssessment::Unknown => 3,
            TrapAssessment::Constrained => 2,
            TrapAssessment::ForcedCorridor => 1,
            TrapAssessment::ProvenTrap => 0,
        }
    }
}

impl Ord for RouteRank {
    fn cmp(&self, other: &Self) -> Ordering {
        self.tier()
            .cmp(&other.tier())
            .then_with(|| match (self.score, other.score) {
                (
                    StateScore::Normal {
                        utility_milli: left,
                    },
                    StateScore::Normal {
                        utility_milli: right,
                    },
                ) => left
                    .saturating_add(i64::from(self.pressure) * 20)
                    .cmp(&right.saturating_add(i64::from(other.pressure) * 20)),
                _ => self.score.cmp(&other.score),
            })
            .then_with(|| self.pressure.cmp(&other.pressure))
            .then_with(|| self.safety_rank().cmp(&other.safety_rank()))
    }
}

impl PartialOrd for RouteRank {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone)]
struct Route {
    rank: RouteRank,
    path: FuturePath,
    certainty: ForecastCertainty,
    /// Exact number of simulated turns until a terminal Win/Loss/Tie.
    /// Only present when the terminal state was actually reached.
    terminal_plies: Option<u16>,
}

fn compare_routes(left: &Route, right: &Route) -> Ordering {
    match (left.rank.score, right.rank.score) {
        // If every available reply loses, delay a proven loss.
        (StateScore::Loss, StateScore::Loss)
            if left.terminal_plies.is_some() && right.terminal_plies.is_some() =>
        {
            // Avoid an immediate collision before comparing other losing lines.
            // Otherwise keep the structural guard's escape preference: a path
            // with more freedom must not lose merely because a forecast delays
            // its eventual terminal outcome by one speculative turn.
            let left_immediate = left.terminal_plies == Some(1);
            let right_immediate = right.terminal_plies == Some(1);
            right_immediate
                .cmp(&left_immediate)
                .then_with(|| left.rank.cmp(&right.rank))
                .then_with(|| left.terminal_plies.cmp(&right.terminal_plies))
        }
        // Among forced wins, complete the win earlier.
        (StateScore::Win, StateScore::Win)
            if left.terminal_plies.is_some() && right.terminal_plies.is_some() =>
        {
            right
                .terminal_plies
                .cmp(&left.terminal_plies)
                .then_with(|| left.rank.cmp(&right.rank))
        }
        _ => left.rank.cmp(&right.rank),
    }
}

/// Search-scoped caches stay valid across iterative-deepening iterations.
#[derive(Default)]
struct ScoredCache {
    leaf_ranks: HashMap<NodeId, RouteRank>,
    corridor_orders: HashMap<NodeId, AdversarialCorridorOrder>,
    territory_snapshots: HashMap<NodeId, TerritorySnapshot>,
}

impl ScoredCache {
    fn new() -> Self {
        Self::default()
    }

    fn entry(&mut self, id: NodeId) -> std::collections::hash_map::Entry<'_, NodeId, RouteRank> {
        self.leaf_ranks.entry(id)
    }

    fn get(&self, id: &NodeId) -> Option<&RouteRank> {
        self.leaf_ranks.get(id)
    }

    fn corridor(&mut self, graph: &FutureGraph, node: NodeId) -> AdversarialCorridorOrder {
        *self
            .corridor_orders
            .entry(node)
            .or_insert_with(|| adversarial_order(&graph.node(node).state))
    }
}

fn alpha_cuts_reply(incumbent: &Route, worst_so_far: &Route) -> bool {
    // MIN cannot improve its value by inspecting additional replies. Once its
    // upper bound is <= this MAX incumbent, the action cannot win the MAX.
    !compare_routes(worst_so_far, incumbent).is_gt()
}

pub(crate) fn search_hobbs(
    graph: &mut FutureGraph,
    budget: &SearchBudget,
) -> Result<Option<HobbsSearchResult>, SearchError> {
    search_hobbs_with_territory_ordering(graph, budget, false)
}

/// Experimental: only change which cached root directions are searched first.
/// Completed Minimax depths still enumerate every required enemy reply and
/// compare the exact same RouteRank as the default Hobbs flow.
pub(crate) fn search_hobbs_with_territory_ordering(
    graph: &mut FutureGraph,
    budget: &SearchBudget,
    territory_ordering: bool,
) -> Result<Option<HobbsSearchResult>, SearchError> {
    // The soft deadline leaves headroom for serialization and runtime jitter.
    let search_budget = budget.limited_to_soft_deadline();
    let mut scored = ScoredCache::new();
    let root = graph.root();
    let mut last_complete = None;
    for depth in 1..=MAX_SEARCH_DEPTH {
        if search_budget.expired() {
            break;
        }
        let Some(route) = evaluate_minimax(
            graph,
            root,
            depth,
            ForecastCertainty::Deterministic,
            &search_budget,
            &mut scored,
            territory_ordering,
        )?
        else {
            break;
        };

        let Some(our_actor) = graph
            .node(root)
            .state
            .actor_index(&graph.node(root).state.our_snake_id)
        else {
            return Ok(last_complete);
        };
        let Some(direction) = route
            .path
            .first()
            .and_then(|step| step.joint_action.direction_for(our_actor))
        else {
            return Ok(last_complete);
        };

        let directions = graph
            .node(root)
            .children
            .iter()
            .filter_map(|edge| edge.joint_action.direction_for(our_actor))
            .collect::<std::collections::HashSet<_>>();
        let survival = route.rank.safety;
        log::debug!(
            target: "search_diagnostics",
            "hobbs_depth turn={} depth={} move={:?} score={:?} survival={:?} root_directions={} nodes={}",
            graph.node(root).state.turn,
            depth,
            direction,
            route.rank.score,
            survival,
            directions.len(),
            graph.node_count(),
        );
        last_complete = Some(HobbsSearchResult {
            direction,
            score: route.rank.score,
            survival,
            completed_depth: depth,
            root_directions: directions.len(),
            certainty: route.certainty,
            path: route.path.clone(),
        });
        // If the winner/loser is known at an immediate horizon and verified
        // deterministic, continuing cannot change that branch's outcome.
        if depth > 1
            && matches!(route.rank.score, StateScore::Win)
            && !route.certainty.is_provisional()
        {
            break;
        }
    }
    Ok(last_complete)
}

fn evaluate_minimax(
    graph: &mut FutureGraph,
    node_id: NodeId,
    depth: u8,
    certainty: ForecastCertainty,
    budget: &SearchBudget,
    cache: &mut ScoredCache,
    territory_ordering: bool,
) -> Result<Option<Route>, SearchError> {
    if budget.expired() {
        return Ok(None);
    }
    if graph.node(node_id).is_terminal() || depth == 0 {
        let rank = *cache
            .entry(node_id)
            .or_insert_with(|| evaluate_leaf(graph, node_id));
        return Ok(Some(Route {
            rank,
            path: FuturePath::empty(),
            certainty,
            terminal_plies: matches!(
                rank.score,
                StateScore::Win | StateScore::Loss | StateScore::Tie
            )
            .then_some(0),
        }));
    }

    let mut directions = graph.available_directions(node_id);
    let preferred = graph.node(node_id).preferred_direction;
    if node_id == graph.root() {
        // Tactical move ordering only. A one-ply corridor estimate cannot
        // prove a forced trap: all legal joint-action replies remain in MIN.
        let mut corridor_by_direction = HashMap::<Direction, (u8, u8)>::new();
        for direction in &directions {
            let known = graph.known_responses(node_id, *direction);
            if known.is_empty() {
                continue;
            }
            let mut worst_ours = u8::MAX;
            let mut best_enemy = 0_u8;
            for edge in known {
                let forecast = cache.corridor(graph, edge.child);
                worst_ours = worst_ours.min(forecast.our_continuations());
                best_enemy = best_enemy.max(forecast.enemy_continuations());
            }
            corridor_by_direction.insert(*direction, (worst_ours, best_enemy));
        }
        // Sample at most 24 unique cached states per search. All snapshots
        // are shared between iterative depths; no new actions are generated.
        // A 2ms deadline bounds overhead and leaves the normal search reserve.
        let mut territorial = HashMap::<Direction, TerritoryDirectionSample>::new();
        if territory_ordering && depth >= 2 && !budget.expired() {
            let expires = Instant::now() + Duration::from_millis(2);
            for direction in &directions {
                if Instant::now() >= expires {
                    break;
                }
                if let Some(sample) = TerritoryDirectionSample::from_cached_replies(
                    graph,
                    *direction,
                    &mut cache.territory_snapshots,
                    expires,
                ) {
                    territorial.insert(*direction, sample);
                }
            }
        }
        directions.sort_by(|left, right| {
            let a = *left;
            let b = *right;
            let (left_exits, left_enemy) = corridor_by_direction
                .get(&a)
                .copied()
                .unwrap_or((0, u8::MAX));
            let (right_exits, right_enemy) = corridor_by_direction
                .get(&b)
                .copied()
                .unwrap_or((0, u8::MAX));
            (left_exits == 0)
                .cmp(&(right_exits == 0))
                .then_with(|| match (territorial.get(&a), territorial.get(&b)) {
                    (Some(left_hint), Some(right_hint)) => {
                        right_hint.ordering_key().cmp(&left_hint.ordering_key())
                    }
                    _ => Ordering::Equal,
                })
                .then_with(|| (Some(a) != preferred).cmp(&(Some(b) != preferred)))
                .then_with(|| right_exits.cmp(&left_exits))
                .then_with(|| left_enemy.cmp(&right_enemy))
                .then_with(|| a.rank().cmp(&b.rank()))
        });
    } else {
        directions.sort_by_key(|direction| (Some(*direction) != preferred, direction.rank()));
    }
    if directions.is_empty() {
        let rank = *cache
            .entry(node_id)
            .or_insert_with(|| evaluate_leaf(graph, node_id));
        return Ok(Some(Route {
            rank,
            path: FuturePath::empty(),
            certainty,
            terminal_plies: matches!(
                rank.score,
                StateScore::Win | StateScore::Loss | StateScore::Tie
            )
            .then_some(0),
        }));
    }

    let mut best: Option<Route> = None;
    for direction in directions {
        let mut known = graph.known_responses(node_id, direction);
        let known_count = known.len();
        let preferred_response = graph.node(node_id).preferred_reply.get(&direction).cloned();
        let corridor_hints = if node_id == graph.root() {
            known
                .iter()
                .map(|edge| (edge.child, cache.corridor(graph, edge.child)))
                .collect::<HashMap<NodeId, AdversarialCorridorOrder>>()
        } else {
            HashMap::new()
        };
        known.sort_by(|a, b| {
            (preferred_response.as_ref() == Some(&b.joint_action))
                .cmp(&(preferred_response.as_ref() == Some(&a.joint_action)))
                .then_with(
                    || match (corridor_hints.get(&a.child), corridor_hints.get(&b.child)) {
                        (Some(a), Some(b)) => a.cmp(b),
                        _ => Ordering::Equal,
                    },
                )
                .then_with(|| match (cache.get(&a.child), cache.get(&b.child)) {
                    (Some(ra), Some(rb)) => ra.cmp(rb),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                })
        });

        let mut cached = known.into_iter();
        let mut next_index = known_count;
        let mut worst: Option<Route> = None;
        let mut pruned = false;
        loop {
            let edge = if let Some(existing) = cached.next() {
                existing
            } else {
                match graph.next_response(node_id, direction, next_index, budget)? {
                    ResponseLookup::Edge(new_edge) => {
                        next_index += 1;
                        new_edge
                    }
                    ResponseLookup::Exhausted => break,
                    ResponseLookup::Deadline => return Ok(None),
                }
            };
            if budget.expired() {
                return Ok(None);
            }
            let SearchEdge {
                joint_action,
                child,
                forecast_delta,
            } = edge;
            let child_certainty = certainty.after(forecast_delta);
            let Some(mut candidate) = evaluate_minimax(
                graph,
                child,
                depth - 1,
                child_certainty,
                budget,
                cache,
                territory_ordering,
            )?
            else {
                return Ok(None);
            };
            candidate.terminal_plies = candidate
                .terminal_plies
                .map(|steps| steps.saturating_add(1));
            let immediate_safety = assess_survival_state(&graph.node(child).state);
            candidate.rank.safety = worst_safety(candidate.rank.safety, immediate_safety);
            candidate.path = candidate.path.prepend(FutureStep {
                node: node_id,
                joint_action,
                child,
            });
            if worst
                .as_ref()
                .is_none_or(|previous| compare_routes(&candidate, previous).is_lt())
            {
                worst = Some(candidate);
            }
            if let (Some(incumbent), Some(worst_so_far)) = (&best, &worst) {
                if alpha_cuts_reply(incumbent, worst_so_far) {
                    pruned = true;
                    break;
                }
            }
        }
        if let Some(worst_response) = worst {
            if let Some(first) = worst_response.path.first() {
                graph.record_worst_reply(node_id, direction, first.joint_action.clone());
            }
            if pruned {
                continue;
            }
            if best.as_ref().is_none_or(|previous| {
                compare_routes(&worst_response, previous).is_gt()
                    || (compare_routes(&worst_response, previous).is_eq()
                        && direction.rank()
                            < direction_of_first(graph, node_id, &previous.path)
                                .map_or(u8::MAX, Direction::rank))
            }) {
                best = Some(worst_response);
            }
        }
    }
    if let Some(chosen) = best.as_ref() {
        let direction = direction_of_first(graph, node_id, &chosen.path);
        graph.record_search(
            node_id,
            depth,
            CachedSearchValue {
                score: chosen.rank.score,
                safety: chosen.rank.safety,
                depth,
                certainty: chosen.certainty,
            },
            direction,
        );
    }
    Ok(best)
}

fn worst_safety(left: TrapAssessment, right: TrapAssessment) -> TrapAssessment {
    use TrapAssessment::{Constrained, ForcedCorridor, ProvenTrap, Unknown, Viable};
    match (left, right) {
        (ProvenTrap, _) | (_, ProvenTrap) => ProvenTrap,
        (ForcedCorridor, _) | (_, ForcedCorridor) => ForcedCorridor,
        (Constrained, _) | (_, Constrained) => Constrained,
        (Unknown, _) | (_, Unknown) => Unknown,
        (Viable, Viable) => Viable,
    }
}

fn direction_of_first(graph: &FutureGraph, node: NodeId, path: &FuturePath) -> Option<Direction> {
    let state = &graph.node(node).state;
    let our_actor = state.actor_index(&state.our_snake_id)?;
    path.first()?.joint_action.direction_for(our_actor)
}

fn evaluate_leaf(graph: &FutureGraph, node_id: NodeId) -> RouteRank {
    let state = &graph.node(node_id).state;
    let Some(actor) = state.actor_index(&state.our_snake_id) else {
        return RouteRank {
            score: StateScore::Loss,
            safety: TrapAssessment::Unknown,
            pressure: 0,
        };
    };
    let params = HobbsScoreParams::STANDARD;
    let territory = TemporalTerritory::from_state(state, params.fill_cycles, params.cell_weights);
    let score = evaluate_hobbs_state(state, &territory, actor, params).score;
    let survival = assess_survival_state(state);
    let (safety, pressure) = corridor_tactics(state, score, survival);
    RouteRank {
        score,
        safety,
        pressure,
    }
}

// The leaf of a deep minimax search already represents several actual joint
// turns. Keep the extra tactical projection short and strictly demand-driven;
// the full four-cycle projection is cached for near-root ordering.
const LEAF_CORRIDOR_HORIZON: u8 = 3;

/// Only verify expensive local continuations if some living snake is
/// down to a single deterministic next move. Exact MIN responses remain
/// responsible for proving whether an opponent can escape.
fn corridor_tactics(
    state: &crate::simulation::state::SimulatedGameState,
    score: StateScore,
    safety: TrapAssessment,
) -> (TrapAssessment, u8) {
    if !matches!(score, StateScore::Normal { .. }) || matches!(safety, TrapAssessment::ProvenTrap) {
        return (safety, 0);
    }
    let mobility = MobilityAnalysis::from_state(state);
    if !state
        .snakes
        .iter()
        .filter(|snake| snake.alive)
        .any(|snake| mobility.deterministic_moves_for(state, &snake.id).len() <= 1)
    {
        return (safety, 0);
    }

    let Some(ours) = CorridorOutlook::from_state(state, &state.our_snake_id, LEAF_CORRIDOR_HORIZON)
    else {
        return (safety, 0);
    };
    // One projected first exit can still lead to a sustainable circulation.
    // Penalize only the absence of ANY projected continuation; the existing
    // structural guard handles one-lane threats from stronger opponents.
    let safety = if ours.continuing_exits == 0 {
        worst_safety(safety, TrapAssessment::ForcedCorridor)
    } else {
        safety
    };
    // A prospective enemy restriction only matters if we maintain more than
    // one possible route ourselves. No tactical forecast implies a forced win.
    let pressure = if ours.continuing_exits >= 2 {
        state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != state.our_snake_id)
            .filter_map(|enemy| {
                CorridorOutlook::from_state(state, &enemy.id, LEAF_CORRIDOR_HORIZON).map(
                    |outlook| match outlook.continuing_exits {
                        0 => 2,
                        1 => 1,
                        _ => 0,
                    },
                )
            })
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    (safety, pressure)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
    use crate::Coord;
    use std::time::Duration;

    fn snake(id: &str, x: i32, y: i32, length: usize) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: vec![Coord { x, y }; length],
            alive: true,
        }
    }

    fn state() -> SimulatedGameState {
        SimulatedGameState {
            turn: 0,
            width: 7,
            height: 7,
            food: vec![Coord { x: 3, y: 4 }],
            hazards: vec![],
            snakes: vec![snake("ours", 1, 2, 3), snake("enemy", 5, 5, 3)],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    fn route(score: StateScore, safety: TrapAssessment, terminal_plies: Option<u16>) -> Route {
        Route {
            rank: RouteRank {
                score,
                safety,
                pressure: 0,
            },
            path: FuturePath::empty(),
            certainty: ForecastCertainty::Deterministic,
            terminal_plies,
        }
    }

    #[test]
    fn longer_forced_survival_beats_immediate_loss() {
        let collision = route(StateScore::Loss, TrapAssessment::Viable, Some(1));
        let delayed = route(StateScore::Loss, TrapAssessment::ForcedCorridor, Some(4));
        assert!(compare_routes(&delayed, &collision).is_gt());
    }

    #[test]
    fn losing_corner_line_does_not_beat_safer_route_on_delay_alone() {
        let safer = route(StateScore::Loss, TrapAssessment::Viable, Some(3));
        let corner = route(StateScore::Loss, TrapAssessment::ForcedCorridor, Some(5));
        assert!(compare_routes(&safer, &corner).is_gt());
    }

    #[test]
    fn local_alpha_cuts_only_while_max_action_cannot_improve() {
        let incumbent = route(
            StateScore::Normal { utility_milli: 500 },
            TrapAssessment::Viable,
            None,
        );
        let losing = route(
            StateScore::Normal { utility_milli: 250 },
            TrapAssessment::Viable,
            None,
        );
        let promising = route(
            StateScore::Normal { utility_milli: 650 },
            TrapAssessment::Viable,
            None,
        );
        assert!(alpha_cuts_reply(&incumbent, &losing));
        assert!(!alpha_cuts_reply(&incumbent, &promising));
    }

    #[test]
    fn forced_win_prefers_shorter_line() {
        let quick = route(StateScore::Win, TrapAssessment::Viable, Some(2));
        let slow = route(StateScore::Win, TrapAssessment::Viable, Some(6));
        assert!(compare_routes(&quick, &slow).is_gt());
    }

    /// State captured from the actual 20261004 match at turn 286.
    /// Right means an avoidable head-to-head with a 30-segment Hobbs.
    fn recorded_hobbs_20261004_turn_286() -> SimulatedGameState {
        fn body(points: &[(i32, i32)]) -> Vec<Coord> {
            points.iter().map(|&(x, y)| Coord { x, y }).collect()
        }
        SimulatedGameState {
            turn: 286,
            width: 11,
            height: 11,
            food: body(&[(7, 1)]),
            hazards: Vec::new(),
            snakes: vec![
                SimulatedSnake {
                    id: "ours".into(),
                    health: 98,
                    alive: true,
                    body: body(&[
                        (5, 9),
                        (5, 8),
                        (5, 7),
                        (5, 6),
                        (4, 6),
                        (3, 6),
                        (2, 6),
                        (1, 6),
                        (1, 7),
                        (1, 8),
                        (2, 8),
                        (2, 7),
                        (3, 7),
                        (3, 8),
                        (3, 9),
                        (2, 9),
                        (1, 9),
                        (0, 9),
                        (0, 8),
                        (0, 7),
                        (0, 6),
                        (0, 5),
                        (0, 4),
                        (1, 4),
                        (2, 4),
                        (3, 4),
                        (4, 4),
                        (4, 5),
                        (5, 5),
                    ]),
                },
                SimulatedSnake {
                    id: "hobbs".into(),
                    health: 85,
                    alive: true,
                    body: body(&[
                        (6, 8),
                        (6, 7),
                        (6, 6),
                        (7, 6),
                        (7, 5),
                        (7, 4),
                        (7, 3),
                        (6, 3),
                        (6, 4),
                        (5, 4),
                        (5, 3),
                        (4, 3),
                        (3, 3),
                        (2, 3),
                        (1, 3),
                        (1, 2),
                        (0, 2),
                        (0, 1),
                        (0, 0),
                        (1, 0),
                        (1, 1),
                        (2, 1),
                        (2, 0),
                        (3, 0),
                        (4, 0),
                        (5, 0),
                        (6, 0),
                        (7, 0),
                        (8, 0),
                        (9, 0),
                    ]),
                },
            ],
            our_snake_id: "ours".into(),
            rules: RulesContext {
                name: "standard".into(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn recorded_turn_286_prioritizes_surviving_the_next_turn() {
        use crate::simulation::resolver::resolve_turn;
        let state = recorded_hobbs_20261004_turn_286();
        let us = state.actor_index("ours").unwrap();
        let enemy = state.actor_index("hobbs").unwrap();
        let right = JointAction::new()
            .with_move(us, Direction::Right)
            .with_move(enemy, Direction::Up);
        let up = JointAction::new()
            .with_move(us, Direction::Up)
            .with_move(enemy, Direction::Up);
        assert!(
            !resolve_turn(&state, &right)
                .unwrap()
                .state
                .snake("ours")
                .unwrap()
                .alive
        );
        assert!(
            resolve_turn(&state, &up)
                .unwrap()
                .state
                .snake("ours")
                .unwrap()
                .alive
        );

        let mut graph = FutureGraph::new_beam(state);
        let budget = SearchBudget::for_duration(Duration::from_secs(2));
        let mut scored = ScoredCache::new();
        let root = graph.root();
        let selected = evaluate_minimax(
            &mut graph,
            root,
            1,
            ForecastCertainty::Deterministic,
            &budget,
            &mut scored,
        )
        .unwrap()
        .unwrap();
        let direction = selected
            .path
            .first()
            .unwrap()
            .joint_action
            .direction_for(us)
            .unwrap();
        assert_ne!(
            direction,
            Direction::Right,
            "never prefer immediate losing head-to-head"
        );
    }

    #[test]
    fn loss_tie_prefers_less_restricted_escape() {
        let less_restricted = RouteRank {
            score: StateScore::Loss,
            safety: TrapAssessment::Constrained,
            pressure: 0,
        };
        let forced = RouteRank {
            score: StateScore::Loss,
            safety: TrapAssessment::ForcedCorridor,
            pressure: 0,
        };
        assert!(less_restricted > forced);
    }

    #[test]
    fn forecast_preserves_worst_intermediate_safety() {
        assert_eq!(
            worst_safety(TrapAssessment::Viable, TrapAssessment::Constrained),
            TrapAssessment::Constrained
        );
        assert_eq!(
            worst_safety(TrapAssessment::Constrained, TrapAssessment::ProvenTrap),
            TrapAssessment::ProvenTrap
        );
    }

    #[test]
    fn terminal_win_outweighs_nonterminal_viability() {
        let win = RouteRank {
            score: StateScore::Win,
            safety: TrapAssessment::Unknown,
            pressure: 0,
        };
        let normal = RouteRank {
            score: StateScore::Normal {
                utility_milli: 2000,
            },
            safety: TrapAssessment::Viable,
            pressure: 0,
        };
        assert!(win > normal);
    }

    #[test]
    fn constrained_exit_loses_to_viable_alternative_without_score_weight() {
        let constrained = RouteRank {
            score: StateScore::Normal {
                utility_milli: 1000,
            },
            safety: TrapAssessment::Constrained,
            pressure: 0,
        };
        let viable = RouteRank {
            score: StateScore::Normal {
                utility_milli: -200,
            },
            safety: TrapAssessment::Viable,
            pressure: 0,
        };
        assert!(viable > constrained);
        assert!(
            constrained
                > RouteRank {
                    score: StateScore::Tie,
                    safety: TrapAssessment::Unknown,
                    pressure: 0,
                }
        );
    }

    #[test]
    fn corridor_pressure_is_a_tactical_tiebreaker_not_a_proven_win() {
        let safe = RouteRank {
            score: StateScore::Normal { utility_milli: 500 },
            safety: TrapAssessment::Viable,
            pressure: 0,
        };
        let threatening = RouteRank {
            score: StateScore::Normal { utility_milli: 480 },
            safety: TrapAssessment::Viable,
            pressure: 2,
        };
        let reckless = RouteRank {
            score: StateScore::Normal { utility_milli: 450 },
            safety: TrapAssessment::Viable,
            pressure: 2,
        };
        let unsafe_attack = RouteRank {
            score: StateScore::Normal {
                utility_milli: 1000,
            },
            safety: TrapAssessment::ForcedCorridor,
            pressure: 2,
        };
        assert!(threatening > safe);
        assert!(
            safe > reckless,
            "small corridor hints cannot override strong territory"
        );
        assert!(safe > unsafe_attack);
        assert!(
            RouteRank {
                score: StateScore::Win,
                safety: TrapAssessment::Unknown,
                pressure: 0,
            } > threatening
        );
    }

    #[test]
    fn all_root_directions_receive_a_pessimistic_answer() {
        let mut graph = FutureGraph::new_beam(state());
        let budget = SearchBudget::for_duration(Duration::from_secs(4));
        let result = search_hobbs(&mut graph, &budget).unwrap().unwrap();
        assert!(result.completed_depth >= 1);
        assert!(result.root_directions >= 2);
        let our_actor = graph.node(graph.root()).state.actor_index("ours").unwrap();
        assert!(graph
            .node(graph.root())
            .children
            .iter()
            .any(|edge| edge.joint_action.direction_for(our_actor) == Some(result.direction)));
    }

    #[test]
    fn no_depth_is_committed_when_budget_is_empty() {
        let mut graph = FutureGraph::new_beam(state());
        let budget = SearchBudget::for_duration(Duration::ZERO);
        assert!(search_hobbs(&mut graph, &budget).unwrap().is_none());
    }
}
