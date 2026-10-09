//! Conservative, search-independent diagnostic of sustainable territory.
//!
//! This is NOT a proof of survival, enemy intention, or ownership. The graph at
//! cycle one accounts for known body releases; the future reachability bound
//! permits optimistic revisits and does not advance opponents' complete bodies.
//! All decisions continue to be made by the existing Minimax. This module
//! supplies evidence for offline/shadow comparison before any scoring changes.

use std::collections::VecDeque;

use crate::simulation::state::SimulatedGameState;
use crate::Coord;

const MAX_CELLS: usize = 400;
const TEMPORAL_HORIZON: u16 = 6;
const GATE_THREAT_HORIZON: u16 = 4;

/// Each field is descriptive rather than a strategic weight. No component of
/// this struct is currently added to the minimax state score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerritorialControlAnalysis {
    /// Cells connected to the actor's head after currently vacating tails.
    pub(crate) accessible_now: u16,
    /// Distinct components entered through different first-step neighbors.
    pub(crate) independent_regions: u8,
    /// Size of the largest branch remaining when the current head cell is removed.
    pub(crate) largest_independent_region: u16,
    /// Current articulation cells separating at least two reachable cells.
    pub(crate) critical_gateways: u16,
    /// Largest area exposed to losing a single gateway, including its cell.
    pub(crate) single_gate_exposure: u16,
    /// Largest gate exposure for which an enemy could plausibly arrive soon.
    pub(crate) contested_gate_exposure: u16,
    /// Optimistic future cells that the actor's head could visit by horizon.
    pub(crate) future_reach: u16,
    /// Cells hypothetically reachable by the actor and at least one enemy.
    pub(crate) contested_future_reach: u16,
    /// Immediate directions with an actual physically executable continuation
    /// to the shorter corridor horizon. May be absent if projection truncated.
    pub(crate) continuing_exits: Option<u8>,
}

pub(crate) fn analyze(
    state: &SimulatedGameState,
    snake_id: &str,
) -> Option<TerritorialControlAnalysis> {
    let snake = state.snake(snake_id).filter(|snake| snake.alive)?;
    let head = snake.head()?;
    let width = usize::try_from(state.width).ok()?;
    let height = usize::try_from(state.height).ok()?;
    let cells = width.checked_mul(height)?;
    if cells == 0 || cells > MAX_CELLS {
        return None;
    }
    let root = index(head, width, height)?;

    // Known release times are an optimistic upper bound: subsequent eating,
    // head-to-head responses and moving bodies may keep cells blocked longer.
    let mut releases = vec![0_u16; cells];
    for other in state.snakes.iter().filter(|s| s.alive) {
        for (segment, &coord) in other.body.iter().enumerate() {
            if let Some(i) = index(coord, width, height) {
                let remaining = other.body.len().saturating_sub(segment);
                releases[i] = releases[i].max(u16::try_from(remaining).unwrap_or(u16::MAX));
            }
        }
    }

    // The actor's starting head is a permitted source, not a traversable
    // shortcut for another snake. All other body cells obey tail releases.
    let mut open_now: Vec<bool> = releases.iter().map(|&release| release <= 1).collect();
    open_now[root] = true;
    let connected = region(root, &open_now, width, height, None);
    let accessible_now = count(&connected);

    // Removing the source separates its genuinely distinct first-step
    // regions. Do not count two paths into the same connected area twice.
    let mut seen = vec![false; cells];
    let mut independent_regions = 0_u8;
    let mut largest_independent_region = 0_u16;
    for neighbor in neighbors(root, width, height).into_iter().flatten() {
        if !open_now[neighbor] || seen[neighbor] {
            continue;
        }
        let branch = region(neighbor, &open_now, width, height, Some(root));
        if count(&branch) == 0 {
            continue;
        }
        independent_regions = independent_regions.saturating_add(1);
        largest_independent_region = largest_independent_region.max(count(&branch));
        for (i, present) in branch.iter().enumerate() {
            seen[i] |= *present;
        }
    }

    let our_arrival = temporal_arrivals(root, &releases, width, height);
    let mut enemy_arrival = vec![u16::MAX; cells];
    for enemy in state.snakes.iter().filter(|s| s.alive && s.id != snake_id) {
        let Some(enemy_root) = enemy.head().and_then(|pos| index(pos, width, height)) else {
            continue;
        };
        let arrivals = temporal_arrivals(enemy_root, &releases, width, height);
        for (best, enemy_time) in enemy_arrival.iter_mut().zip(arrivals) {
            *best = (*best).min(enemy_time);
        }
    }

    let mut critical_gateways = 0_u16;
    let mut single_gate_exposure = 0_u16;
    let mut contested_gate_exposure = 0_u16;
    for candidate in 0..cells {
        if candidate == root || !connected[candidate] {
            continue;
        }
        let without = region(root, &open_now, width, height, Some(candidate));
        let exposed = accessible_now.saturating_sub(count(&without));
        if exposed <= 1 {
            continue;
        }
        critical_gateways = critical_gateways.saturating_add(1);
        single_gate_exposure = single_gate_exposure.max(exposed);
        // Potential competition only. The enemy may have no physically
        // executable body-safe route to this cell: do not claim certainty.
        if enemy_arrival[candidate] <= GATE_THREAT_HORIZON
            && enemy_arrival[candidate] <= our_arrival[candidate].saturating_add(1)
        {
            contested_gate_exposure = contested_gate_exposure.max(exposed);
        }
    }

    let future_reach = our_arrival
        .iter()
        .filter(|&&time| time != u16::MAX)
        .count()
        .try_into()
        .unwrap_or(u16::MAX);
    let contested_future_reach = our_arrival
        .iter()
        .zip(&enemy_arrival)
        .filter(|(ours, theirs)| **ours != u16::MAX && **theirs != u16::MAX)
        .count()
        .try_into()
        .unwrap_or(u16::MAX);

    // Optional: the exact-body short projection is deliberately bounded and
    // may refuse dense states rather than giving a misleading zero value.
    let continuing_exits = crate::analysis::CorridorOutlook::from_state(state, snake_id, 3)
        .map(|outlook| outlook.continuing_exits);

    Some(TerritorialControlAnalysis {
        accessible_now,
        independent_regions,
        largest_independent_region,
        critical_gateways,
        single_gate_exposure,
        contested_gate_exposure,
        future_reach,
        contested_future_reach,
        continuing_exits,
    })
}

fn count(cells: &[bool]) -> u16 {
    u16::try_from(cells.iter().filter(|&&cell| cell).count()).unwrap_or(u16::MAX)
}

fn region(
    source: usize,
    passable: &[bool],
    width: usize,
    height: usize,
    blocked: Option<usize>,
) -> Vec<bool> {
    let mut found = vec![false; passable.len()];
    if !passable[source] || Some(source) == blocked {
        return found;
    }
    let mut queue = VecDeque::new();
    found[source] = true;
    queue.push_back(source);
    while let Some(cell) = queue.pop_front() {
        for next in neighbors(cell, width, height).into_iter().flatten() {
            if !found[next] && passable[next] && Some(next) != blocked {
                found[next] = true;
                queue.push_back(next);
            }
        }
    }
    found
}

/// An upper bound on head reachability: possible arrival times are optimistic
/// about future body movements and permitted revisits. No "stay" action is
/// introduced, because a Battlesnake must move each turn.
fn temporal_arrivals(source: usize, releases: &[u16], width: usize, height: usize) -> Vec<u16> {
    let mut arrival = vec![u16::MAX; releases.len()];
    arrival[source] = 0;
    let mut frontier = vec![false; releases.len()];
    frontier[source] = true;
    for cycle in 1..=TEMPORAL_HORIZON {
        let mut next_frontier = vec![false; releases.len()];
        for (cell, &reachable) in frontier.iter().enumerate() {
            if !reachable {
                continue;
            }
            for next in neighbors(cell, width, height).into_iter().flatten() {
                if releases[next] <= cycle {
                    next_frontier[next] = true;
                    arrival[next] = arrival[next].min(cycle);
                }
            }
        }
        frontier = next_frontier;
        if !frontier.iter().any(|&reachable| reachable) {
            break;
        }
    }
    arrival
}

fn neighbors(cell: usize, width: usize, height: usize) -> [Option<usize>; 4] {
    let x = cell % width;
    let y = cell / width;
    [
        (y + 1 < height).then(|| cell + width),
        (x + 1 < width).then(|| cell + 1),
        (y > 0).then(|| cell - width),
        (x > 0).then(|| cell - 1),
    ]
}

fn index(coord: Coord, width: usize, height: usize) -> Option<usize> {
    let x = usize::try_from(coord.x).ok()?;
    let y = usize::try_from(coord.y).ok()?;
    if x >= width || y >= height {
        return None;
    }
    y.checked_mul(width)?.checked_add(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedSnake};

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_owned(),
            health: 100,
            alive: true,
            body: body.iter().map(|&(x, y)| Coord { x, y }).collect(),
        }
    }

    fn board(width: u32, height: u32, snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 0,
            width,
            height,
            food: vec![],
            hazards: vec![],
            snakes,
            our_snake_id: "ours".to_owned(),
            rules: RulesContext {
                name: "standard".to_owned(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn open_field_has_no_critical_gateway() {
        let state = board(
            7,
            7,
            vec![snake("ours", &[(2, 2)]), snake("enemy", &[(6, 6)])],
        );
        let result = analyze(&state, "ours").unwrap();
        assert_eq!(result.critical_gateways, 0);
        assert_eq!(result.single_gate_exposure, 0);
        assert!(result.future_reach >= 12);
        assert!(result.contested_future_reach > 0);
    }

    #[test]
    fn a_gate_can_disconnect_a_large_accessible_region() {
        // The enemy's current body is a stable divider, with only one
        // traversable doorway at (3, 3) during the immediate graph.
        let mut divider = vec![(3, 6)];
        for y in (0..=2).chain(4..=5) {
            divider.push((3, y));
        }
        // Consecutive body segments are intentionally irrelevant to the
        // static known-release diagnostic; physical validity is handled by MIN.
        let state = board(
            7,
            7,
            vec![snake("ours", &[(1, 3)]), snake("enemy", &divider)],
        );
        let result = analyze(&state, "ours").unwrap();
        assert!(result.critical_gateways > 0);
        assert!(result.single_gate_exposure >= 7);
        assert!(result.accessible_now > 7);
    }

    #[test]
    fn tail_release_reopens_a_short_corridor() {
        let state = board(
            3,
            3,
            vec![
                snake("ours", &[(0, 1), (1, 1), (1, 0), (2, 0)]),
                snake("enemy", &[(2, 2), (2, 1)]),
            ],
        );
        let first = analyze(&state, "ours").unwrap();
        assert!(first.accessible_now > 1);
        assert!(first.future_reach >= first.accessible_now);
    }

    #[test]
    fn incomplete_oversized_board_is_not_a_proven_trap() {
        let state = board(25, 25, vec![snake("ours", &[(10, 10)])]);
        assert_eq!(analyze(&state, "ours"), None);
    }

    #[test]
    fn direction_independent_perspective() {
        let state = board(
            7,
            7,
            vec![snake("ours", &[(1, 1)]), snake("enemy", &[(5, 5)])],
        );
        let ours = analyze(&state, "ours").unwrap();
        let enemy = analyze(&state, "enemy").unwrap();
        assert_eq!(ours.accessible_now, enemy.accessible_now);
        assert_eq!(ours.future_reach, enemy.future_reach);
    }

    #[test]
    fn food_does_not_create_a_fictitious_permanent_clearance() {
        let mut state = board(
            4,
            4,
            vec![
                snake("ours", &[(1, 1), (1, 0), (2, 0), (2, 1)]),
                snake("enemy", &[(3, 3)]),
            ],
        );
        state.food.push(Coord { x: 2, y: 1 });
        let result = analyze(&state, "ours").unwrap();
        // Any food-dependent release is still only optimistic; the exact
        // corridor evaluator must reject the growth-blocked tail move.
        assert_eq!(result.continuing_exits, Some(2));
    }

    #[test]
    fn recorded_hobbs_20261003_turn_313_measures_access_to_larger_region() {
        use crate::direction::Direction;
        use crate::simulation::joint_action::JointAction;
        use crate::simulation::resolver::resolve_turn;

        // Actual body and food positions from the runner-only match.
        // Use the same opponent reply (Down) to compare the two decisions;
        // this is diagnostic evidence, NOT a proof that Left wins minimax.
        let mut state = board(
            11,
            11,
            vec![
                snake(
                    "ours",
                    &[
                        (8, 9),
                        (8, 8),
                        (8, 7),
                        (8, 6),
                        (8, 5),
                        (9, 5),
                        (9, 4),
                        (9, 3),
                        (9, 2),
                        (9, 1),
                        (8, 1),
                        (8, 2),
                        (7, 2),
                        (7, 1),
                        (6, 1),
                        (6, 2),
                        (5, 2),
                        (5, 1),
                        (4, 1),
                        (3, 1),
                        (2, 1),
                        (1, 1),
                        (1, 2),
                        (2, 2),
                        (2, 3),
                        (3, 3),
                    ],
                ),
                snake(
                    "hobbs",
                    &[
                        (6, 7),
                        (6, 8),
                        (5, 8),
                        (5, 7),
                        (4, 7),
                        (4, 6),
                        (3, 6),
                        (3, 7),
                        (3, 8),
                        (3, 9),
                        (3, 10),
                        (2, 10),
                        (1, 10),
                        (0, 10),
                        (0, 9),
                        (0, 8),
                        (0, 7),
                        (0, 6),
                        (0, 5),
                        (0, 4),
                        (0, 3),
                        (1, 3),
                        (1, 4),
                        (2, 4),
                        (3, 4),
                        (4, 4),
                        (4, 5),
                        (5, 5),
                        (5, 4),
                    ],
                ),
            ],
        );
        state.snakes[0].health = 41;
        state.snakes[1].health = 62;
        state.food = vec![
            Coord { x: 9, y: 9 },
            Coord { x: 9, y: 10 },
            Coord { x: 8, y: 0 },
            Coord { x: 2, y: 5 },
        ];
        let us = state.actor_index("ours").unwrap();
        let them = state.actor_index("hobbs").unwrap();
        let step = |direction| {
            resolve_turn(
                &state,
                &JointAction::new()
                    .with_move(us, direction)
                    .with_move(them, Direction::Down),
            )
            .unwrap()
            .state
        };
        let left = step(Direction::Left);
        let up = step(Direction::Up);
        assert!(left.snake("ours").unwrap().alive);
        assert!(up.snake("ours").unwrap().alive);
        let left_access = analyze(&left, "ours").unwrap();
        let up_access = analyze(&up, "ours").unwrap();
        assert_eq!(left_access.accessible_now, up_access.accessible_now);
        assert!(
            left_access.largest_independent_region > up_access.largest_independent_region,
            "the central option preserves a larger connected branch, despite equal immediate reach"
        );
        assert_eq!(left_access.independent_regions, 2);
        assert_eq!(up_access.independent_regions, 2);
    }

    #[test]
    fn dead_enemy_cannot_pressure_a_gateway() {
        let mut state = board(
            7,
            7,
            vec![snake("ours", &[(1, 1)]), snake("enemy", &[(5, 5)])],
        );
        state.snakes[1].alive = false;
        let result = analyze(&state, "ours").unwrap();
        assert_eq!(result.contested_future_reach, 0);
        assert_eq!(result.contested_gate_exposure, 0);
    }
}
