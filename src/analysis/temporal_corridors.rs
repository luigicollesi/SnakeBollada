//! Bounded, tail-aware temporal corridor forecasts for the Hobbs search.
//!
//! Each projected path carries its OWN evolving body, food consumption and
//! health. Opponents are represented by optimistic release times of their
//! CURRENT body cells. Their future moves are NOT guessed here: the joint
//! turn resolver and the paranoid MIN search remain authoritative.
//!
//! The outcome is a tactical hint, not proof of a forced win or loss.
//! Incomplete (too large) analyses return None rather than false certainty.

use std::collections::HashSet;

use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

pub(crate) const CORRIDOR_HORIZON: u8 = 4;
const MAX_ANALYSIS_CELLS: usize = 400;
const MAX_PROJECTIONS: usize = 768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CorridorOutlook {
    /// Distinct first moves with at least one physically executable projected
    /// continuation to the horizon; NOT independent guaranteed escape routes.
    pub(crate) continuing_exits: u8,
    /// Deterministically legal immediate moves (or emergency in-bounds moves).
    pub(crate) immediate_exits: u8,
    /// First projected cycle with no possible continuation. NOT a MIN proof.
    pub(crate) possible_closure_turn: Option<u8>,
    /// First cycle with at most one different reachable projected head cell.
    pub(crate) narrow_turn: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectedPath {
    body: Vec<Coord>,
    health: i32,
    remaining_food: Vec<Coord>,
}

impl ProjectedPath {
    fn step(
        &self,
        direction: crate::direction::Direction,
        cycle: u16,
        state: &SimulatedGameState,
        opponent_release: &[u16],
        width: usize,
        height: usize,
    ) -> Option<Self> {
        let head = direction.apply(*self.body.first()?);
        let index = position_index(head, width, height)?;
        if opponent_release[index] > cycle {
            return None;
        }

        let food_index = self.remaining_food.iter().position(|food| *food == head);
        let grows = food_index.is_some();
        // The tail only vacates when no food is eaten. A projected head cannot
        // enter ANY retained segment, including a tail held by growth.
        let retained = self.body.len().saturating_sub(usize::from(!grows));
        if self.body[..retained].contains(&head) {
            return None;
        }

        let mut remaining_food = self.remaining_food.clone();
        let health = if let Some(food_index) = food_index {
            remaining_food.swap_remove(food_index);
            state.rules.max_health
        } else {
            let hazard_stacks = state
                .hazards
                .iter()
                .filter(|hazard| **hazard == head)
                .count();
            let damage = state
                .rules
                .hazard_damage_per_turn
                .max(0)
                .saturating_mul(i32::try_from(hazard_stacks).unwrap_or(i32::MAX));
            self.health.saturating_sub(1).saturating_sub(damage)
        };
        if health <= 0 {
            return None;
        }
        let mut body = Vec::with_capacity(self.body.len() + usize::from(grows));
        body.push(head);
        body.extend_from_slice(&self.body[..retained]);
        Some(Self {
            body,
            health,
            remaining_food,
        })
    }
}

impl CorridorOutlook {
    pub(crate) fn from_state(
        state: &SimulatedGameState,
        snake_id: &str,
        horizon: u8,
    ) -> Option<Self> {
        let snake = state.snake(snake_id).filter(|snake| snake.alive)?;
        let width = usize::try_from(state.width).ok()?;
        let height = usize::try_from(state.height).ok()?;
        let size = width.checked_mul(height)?;
        if size == 0 || size > MAX_ANALYSIS_CELLS || horizon == 0 || snake.body.is_empty() {
            return None;
        }

        let mobility = MobilityAnalysis::from_state(state);
        let deterministic = mobility.deterministic_moves_for(state, snake_id);
        let legal = if deterministic.is_empty() {
            mobility.in_bounds_moves_for(state, snake_id)
        } else {
            deterministic
        };

        // Optimistic bound: other heads continue away without revisiting
        // their bodies or eating. Any actual reply is resolved by MIN.
        let mut opponent_release = vec![0_u16; size];
        for other in state
            .snakes
            .iter()
            .filter(|other| other.alive && other.id != snake_id)
        {
            for (segment, &position) in other.body.iter().enumerate() {
                if let Some(index) = position_index(position, width, height) {
                    let remaining = other.body.len().saturating_sub(segment);
                    opponent_release[index] =
                        opponent_release[index].max(u16::try_from(remaining).unwrap_or(u16::MAX));
                }
            }
        }

        let initial = ProjectedPath {
            body: snake.body.clone(),
            health: snake.health,
            remaining_food: state.food.clone(),
        };
        let mut fronts: Vec<Vec<ProjectedPath>> = legal
            .iter()
            .map(|direction| {
                initial
                    .step(direction, 1, state, &opponent_release, width, height)
                    .into_iter()
                    .collect()
            })
            .collect();
        let mut outlook = Self {
            continuing_exits: 0,
            immediate_exits: legal.len(),
            possible_closure_turn: None,
            narrow_turn: None,
        };

        for cycle in 1..=horizon {
            let mut heads = HashSet::new();
            let mut total_states = 0_usize;
            let mut continuing_exits = 0_u8;
            for paths in &fronts {
                if !paths.is_empty() {
                    continuing_exits += 1;
                }
                total_states += paths.len();
                for path in paths {
                    if let Some(&head) = path.body.first() {
                        heads.insert(head);
                    }
                }
            }
            if total_states > MAX_PROJECTIONS {
                return None;
            }
            if continuing_exits == 0 {
                outlook.possible_closure_turn = Some(cycle);
                break;
            }
            if heads.len() <= 1 {
                outlook.narrow_turn.get_or_insert(cycle);
            }
            if cycle == horizon {
                outlook.continuing_exits = continuing_exits;
                break;
            }
            let next_cycle = u16::from(cycle) + 1;
            for paths in &mut fronts {
                let mut seen = HashSet::new();
                let mut next = Vec::new();
                for path in paths.iter() {
                    for direction in crate::direction::Direction::ALL {
                        if let Some(projected) = path.step(
                            direction,
                            next_cycle,
                            state,
                            &opponent_release,
                            width,
                            height,
                        ) {
                            if seen.insert(projected.clone()) {
                                next.push(projected);
                                if next.len() > MAX_PROJECTIONS {
                                    return None;
                                }
                            }
                        }
                    }
                }
                *paths = next;
            }
        }
        Some(outlook)
    }
}

/// Only a move-ordering heuristic; a more flexible enemy continuation is
/// considered first by MIN (the opponent selects its best escape).
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
    // An unanalysed state must not be mistaken for a proven constrained one.
    let our_continuations = own.map_or(4, |outlook| outlook.continuing_exits);
    let our_closure_cycle = own
        .and_then(|outlook| outlook.possible_closure_turn)
        .unwrap_or(CORRIDOR_HORIZON + 1);
    let enemy_escape_priority = state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != state.our_snake_id)
        .filter_map(|snake| {
            CorridorOutlook::from_state(state, &snake.id, CORRIDOR_HORIZON)
                .map(|outlook| outlook.continuing_exits)
        })
        .max()
        // Unknown enemy mobility: do not assume an easy encirclement.
        .unwrap_or(4);

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
    fn moving_head_cannot_revisit_its_retained_neck() {
        let s = board(
            2,
            2,
            vec![
                snake("ours", &[(0, 0), (0, 1), (1, 1)]),
                snake("enemy", &[(1, 0), (1, 0)]),
            ],
        );
        // All next squares are blocked, including the optimistic enemy
        // release and our retained second segment.
        let o = CorridorOutlook::from_state(&s, "ours", 2).unwrap();
        assert_eq!(o.continuing_exits, 0);
        assert_eq!(o.possible_closure_turn, Some(1));
    }

    #[test]
    fn food_growth_prevents_stepping_into_the_vacating_tail() {
        let mut s = board(
            4,
            4,
            vec![
                snake("ours", &[(1, 1), (1, 0), (2, 0), (2, 1)]),
                snake("enemy", &[(3, 3)]),
            ],
        );
        let release = vec![0; 16];
        let path = ProjectedPath {
            body: s.snakes[0].body.clone(),
            health: 90,
            remaining_food: Vec::new(),
        };
        assert!(
            path.step(crate::direction::Direction::Right, 1, &s, &release, 4, 4)
                .is_some(),
            "tail vacates without food"
        );

        s.food.push(Coord { x: 2, y: 1 });
        let fed = ProjectedPath {
            remaining_food: s.food.clone(),
            ..path
        };
        assert!(
            fed.step(crate::direction::Direction::Right, 1, &s, &release, 4, 4)
                .is_none(),
            "eating holds the old tail in place"
        );
    }

    #[test]
    fn projected_food_growth_changes_the_body_shape() {
        let mut s = board(
            4,
            4,
            vec![snake("ours", &[(1, 1), (1, 0)]), snake("enemy", &[(3, 3)])],
        );
        s.food.push(Coord { x: 2, y: 1 });
        let initial = ProjectedPath {
            body: s.snakes[0].body.clone(),
            health: 15,
            remaining_food: s.food.clone(),
        };
        let release = vec![0; 16];
        let moved = initial
            .step(crate::direction::Direction::Right, 1, &s, &release, 4, 4)
            .unwrap();
        assert_eq!(
            moved.body,
            vec![
                Coord { x: 2, y: 1 },
                Coord { x: 1, y: 1 },
                Coord { x: 1, y: 0 }
            ]
        );
        assert_eq!(moved.health, 100);
        assert!(moved.remaining_food.is_empty());
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
        assert_eq!(
            corner_hint.our_continuations(),
            center_hint.our_continuations()
        );
        assert!(corner_hint.enemy_continuations() < center_hint.enemy_continuations());
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
