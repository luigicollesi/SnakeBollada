//! Short-horizon corridor reachability for move ordering.
//!
//! This is an *optimistic spatial forecast*, NOT a proof of survival or of a
//! forced enemy loss. A frontier can revisit a cell by a different hypothetical
//! route and enemy movement, future food growth and self-intersections are not
//! simulated. The regular joint-action resolver remains authoritative.
//!
//! It is deliberately computed only for cached near-root continuations in
//! Hobbs search. It must never be an additive Food/Hunting utility.

use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

pub(crate) const CORRIDOR_HORIZON: u8 = 4;
const MAX_ANALYSIS_CELLS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CorridorOutlook {
    /// Number of different first moves with an optimistic continuation all
    /// the way to the horizon. This is NOT a count of independent safe paths.
    pub(crate) continuing_exits: u8,
    /// Legal next steps before the forecast is applied.
    pub(crate) immediate_exits: u8,
    /// Earliest horizon cycle on which no optimistic continuation remains.
    pub(crate) possible_closure_turn: Option<u8>,
    /// First cycle when only a single future head cell remains reachable
    /// across all the separate first-move projections.
    pub(crate) narrow_turn: Option<u8>,
}

impl CorridorOutlook {
    pub(crate) fn from_state(
        state: &SimulatedGameState,
        snake_id: &str,
        horizon: u8,
    ) -> Option<Self> {
        let snake = state.snake(snake_id).filter(|snake| snake.alive)?;
        let head = snake.head()?;
        let width = usize::try_from(state.width).ok()?;
        let height = usize::try_from(state.height).ok()?;
        let size = width.checked_mul(height)?;
        if size == 0 || size > MAX_ANALYSIS_CELLS || horizon == 0 {
            return None;
        }

        let mobility = MobilityAnalysis::from_state(state);
        let legal = mobility.deterministic_moves_for(state, snake_id);
        let legal = if legal.is_empty() {
            mobility.in_bounds_moves_for(state, snake_id)
        } else {
            legal
        };
        let immediate_exits = legal.len();

        // Last segment to vacate a stacked cell determines the first
        // optimistic cycle on which a head may enter it.
        let mut release = vec![0_u16; size];
        for other in state.snakes.iter().filter(|other| other.alive) {
            for (segment, &position) in other.body.iter().enumerate() {
                if let Some(cell) = position_index(position, width, height) {
                    let remaining = other.body.len().saturating_sub(segment);
                    release[cell] = release[cell].max(u16::try_from(remaining).unwrap_or(u16::MAX));
                }
            }
        }

        let mut fronts = Vec::new();
        for direction in legal.iter() {
            let mut mask = vec![false; size];
            if let Some(cell) = position_index(direction.apply(head), width, height) {
                if release[cell] <= 1 {
                    mask[cell] = true;
                }
            }
            fronts.push(mask);
        }
        let mut narrow_turn = None;
        let mut possible_closure_turn = None;
        let mut continuing_exits = 0;
        let steps = horizon.max(1);
        for cycle in 1..=steps {
            let mut union = vec![false; size];
            let mut active = 0_u8;
            for front in &fronts {
                if front.iter().any(|occupied| *occupied) {
                    active = active.saturating_add(1);
                    for (index, &occupied) in front.iter().enumerate() {
                        union[index] |= occupied;
                    }
                }
            }
            if cycle == steps {
                continuing_exits = active;
            }
            if active == 0 {
                possible_closure_turn.get_or_insert(cycle);
                break;
            }
            if union.iter().filter(|&&occupied| occupied).count() <= 1 {
                narrow_turn.get_or_insert(cycle);
            }
            if cycle == steps {
                break;
            }
            let next_cycle = u16::from(cycle) + 1;
            for front in &mut fronts {
                let mut next = vec![false; size];
                for (index, &occupied) in front.iter().enumerate() {
                    if !occupied {
                        continue;
                    }
                    let x = index % width;
                    let y = index / width;
                    let neighbors = [
                        (y + 1 < height).then(|| index + width),
                        (x + 1 < width).then(|| index + 1),
                        (y > 0).then(|| index - width),
                        (x > 0).then(|| index - 1),
                    ];
                    for neighbor in neighbors.into_iter().flatten() {
                        if release[neighbor] <= next_cycle {
                            next[neighbor] = true;
                        }
                    }
                }
                *front = next;
            }
        }
        Some(Self {
            continuing_exits,
            immediate_exits,
            possible_closure_turn,
            narrow_turn,
        })
    }

    /// Prefer lines that preserve more independent *first* options and avoid
    /// an early possible closure. Never use this as a win/loss proof.
    fn flexibility_rank(self) -> (u8, u8, u8) {
        (
            self.continuing_exits,
            self.possible_closure_turn.unwrap_or(CORRIDOR_HORIZON + 1),
            self.immediate_exits,
        )
    }
}

/// Lower keys are searched first, not ranked as strategic outcomes.
/// For MIN, prioritize replies that threaten our circulation; if our
/// continuations are comparable, prioritize replies leaving the opponent
/// *more* future freedom (the adversary's best escape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct AdversarialCorridorOrder {
    our_continuations: u8,
    our_closure_cycle: u8,
    enemy_escape_priority: u8,
}

impl AdversarialCorridorOrder {
    pub(crate) const fn our_continuations(self) -> u8 {
        self.our_continuations
    }

    pub(crate) const fn enemy_continuations(self) -> u8 {
        u8::MAX - self.enemy_escape_priority
    }
}

pub(crate) fn adversarial_order(state: &SimulatedGameState) -> AdversarialCorridorOrder {
    let own = CorridorOutlook::from_state(state, &state.our_snake_id, CORRIDOR_HORIZON);
    let our_continuations = own.map_or(4, |outlook| outlook.flexibility_rank().0);
    let our_closure_cycle = own
        .and_then(|outlook| outlook.possible_closure_turn)
        .unwrap_or(CORRIDOR_HORIZON + 1);
    let enemy_escape_priority = state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != state.our_snake_id)
        .filter_map(|snake| CorridorOutlook::from_state(state, &snake.id, CORRIDOR_HORIZON))
        .map(|outlook| outlook.continuing_exits)
        .max()
        .unwrap_or(0);

    AdversarialCorridorOrder {
        our_continuations,
        our_closure_cycle,
        enemy_escape_priority: u8::MAX - enemy_escape_priority,
    }
}

fn position_index(position: Coord, width: usize, height: usize) -> Option<usize> {
    let x = usize::try_from(position.x).ok()?;
    let y = usize::try_from(position.y).ok()?;
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
            body: body.iter().map(|&(x, y)| Coord { x, y }).collect(),
            health: 90,
            alive: true,
        }
    }

    fn board(width: u32, height: u32, snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 20,
            width,
            height,
            food: Vec::new(),
            hazards: Vec::new(),
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
    fn open_board_has_multiple_continuations() {
        let s = board(
            7,
            7,
            vec![snake("ours", &[(3, 3)]), snake("enemy", &[(6, 6)])],
        );
        let o = CorridorOutlook::from_state(&s, "ours", 4).unwrap();
        assert!(o.immediate_exits >= 3);
        assert!(o.continuing_exits >= 3);
        assert_eq!(o.possible_closure_turn, None);
    }

    #[test]
    fn stacked_tail_is_not_opened_one_cycle_early() {
        let s = board(
            3,
            3,
            vec![
                snake("ours", &[(0, 0), (0, 1), (1, 1), (1, 0), (1, 0)]),
                snake("enemy", &[(2, 2)]),
            ],
        );
        let o = CorridorOutlook::from_state(&s, "ours", 3).unwrap();
        assert_eq!(o.immediate_exits, 2);
        // Both in-bounds fallback directions lead into retained body
        // segments: no optimistic continuation exists at cycle one.
        assert_eq!(o.continuing_exits, 0);
        assert_eq!(o.possible_closure_turn, Some(1));
    }

    #[test]
    fn one_open_turn_can_still_close_on_second_cycle() {
        let s = board(
            3,
            3,
            vec![
                snake("ours", &[(0, 0), (0, 1), (1, 1), (1, 2), (0, 2)]),
                snake("enemy", &[(2, 0), (2, 1), (2, 2)]),
            ],
        );
        let forecast = CorridorOutlook::from_state(&s, "ours", 4).unwrap();
        assert_eq!(forecast.immediate_exits, 1);
        assert_eq!(forecast.continuing_exits, 0);
        assert_eq!(forecast.narrow_turn, Some(1));
        assert_eq!(forecast.possible_closure_turn, Some(2));
    }

    #[test]
    fn adversarial_order_checks_enemy_escape_symmetrically() {
        let corner = board(
            7,
            7,
            vec![snake("ours", &[(3, 3)]), snake("enemy", &[(6, 6)])],
        );
        let center = board(
            7,
            7,
            vec![snake("ours", &[(3, 3)]), snake("enemy", &[(5, 5)])],
        );
        let corner_hint = adversarial_order(&corner);
        let center_hint = adversarial_order(&center);
        assert_eq!(corner_hint.our_continuations(), center_hint.our_continuations());
        assert!(corner_hint.enemy_continuations() < center_hint.enemy_continuations());
        // The adversary's more flexible escape is a more pessimistic reply.
        assert!(center_hint < corner_hint);
    }

    #[test]
    fn horizon_one_agrees_with_immediate_exit_count_in_open_field() {
        let s = board(
            7,
            7,
            vec![snake("ours", &[(2, 2)]), snake("enemy", &[(6, 6)])],
        );
        let o = CorridorOutlook::from_state(&s, "ours", 1).unwrap();
        assert_eq!(o.continuing_exits, o.immediate_exits);
    }

    #[test]
    fn perspective_can_analyze_enemy_symmetrically() {
        let s = board(
            5,
            5,
            vec![snake("ours", &[(2, 2)]), snake("enemy", &[(0, 0)])],
        );
        let us = CorridorOutlook::from_state(&s, "ours", 4).unwrap();
        let enemy = CorridorOutlook::from_state(&s, "enemy", 4).unwrap();
        assert!(us.continuing_exits > enemy.immediate_exits);
    }
}
