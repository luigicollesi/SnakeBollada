use std::collections::VecDeque;

use crate::board_mask::BoardMask;
use crate::strategy::Direction;
use crate::{Coord, GameState};

#[derive(Debug, Clone)]
pub(crate) struct NavigationMap {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) occupied: BoardMask,
    pub(crate) food: BoardMask,
    pub(crate) hazards: BoardMask,
    pub(crate) lethal_head_danger: BoardMask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoodPathCandidate {
    pub(crate) food: Coord,
    pub(crate) distance: u16,
    pub(crate) first_move: Direction,
}

impl NavigationMap {
    pub(crate) fn from_state(state: &GameState) -> Self {
        let width = state.board.width as u16;
        let height = state.board.height as u16;

        let mut occupied = BoardMask::new(width, height);

        let own_body_without_tail = state.you.body.len().saturating_sub(1);
        for coord in state.you.body.iter().take(own_body_without_tail) {
            occupied.set(*coord, true);
        }

        for snake in &state.board.snakes {
            if snake.id == state.you.id {
                continue;
            }
            for coord in &snake.body {
                occupied.set(*coord, true);
            }
        }

        let mut result = Self {
            width,
            height,
            occupied,
            food: BoardMask::from_coords(width, height, state.board.food.iter().copied()),
            hazards: BoardMask::from_coords(width, height, state.board.hazards.iter().copied()),
            lethal_head_danger: BoardMask::new(width, height),
        };

        result.populate_head_danger(state);
        result
    }

    pub(crate) fn in_bounds(&self, coord: Coord) -> bool {
        coord.x >= 0
            && coord.y >= 0
            && coord.x < i32::from(self.width)
            && coord.y < i32::from(self.height)
    }

    pub(crate) fn is_blocked(&self, coord: Coord) -> bool {
        !self.in_bounds(coord)
            || self.occupied.contains(coord)
            || self.lethal_head_danger.contains(coord)
    }

    pub(crate) fn is_food(&self, coord: Coord) -> bool {
        self.food.contains(coord)
    }

    pub(crate) fn is_hazard(&self, coord: Coord) -> bool {
        self.hazards.contains(coord)
    }

    fn cell_count(&self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }

    fn index(&self, coord: Coord) -> Option<usize> {
        if !self.in_bounds(coord) {
            return None;
        }

        Some(coord.y as usize * usize::from(self.width) + coord.x as usize)
    }

    fn populate_head_danger(&mut self, state: &GameState) {
        for enemy in &state.board.snakes {
            if enemy.id == state.you.id || enemy.length < state.you.length {
                continue;
            }

            let neck = enemy.body.get(1).copied();

            for direction in Direction::ALL {
                let target = direction.apply(enemy.head);
                if !self.in_bounds(target) || neck == Some(target) {
                    continue;
                }
                if !self.occupied.contains(target) {
                    self.lethal_head_danger.set(target, true);
                }
            }
        }
    }
}

pub(crate) fn food_candidates(
    map: &NavigationMap,
    start: Coord,
    allow_hazards: bool,
) -> Vec<FoodPathCandidate> {
    let Some(start_index) = map.index(start) else {
        return Vec::new();
    };

    let mut queue = VecDeque::new();
    let mut distance = vec![u16::MAX; map.cell_count()];
    let mut first_move = vec![None; map.cell_count()];

    distance[start_index] = 0;
    queue.push_back(start);

    let mut candidates = Vec::new();

    while let Some(current) = queue.pop_front() {
        let current_index = map.index(current).expect("queued coordinate is in bounds");

        if current != start && map.is_food(current) {
            candidates.push(FoodPathCandidate {
                food: current,
                distance: distance[current_index],
                first_move: first_move[current_index]
                    .expect("food reached from the head has a first move"),
            });
        }

        for direction in Direction::ALL {
            let next = direction.apply(current);
            let Some(next_index) = map.index(next) else {
                continue;
            };

            if map.is_blocked(next) || (!allow_hazards && map.is_hazard(next)) {
                continue;
            }
            if distance[next_index] != u16::MAX {
                continue;
            }

            distance[next_index] = distance[current_index].saturating_add(1);
            first_move[next_index] = if current == start {
                Some(direction)
            } else {
                first_move[current_index]
            };
            queue.push_back(next);
        }
    }

    candidates.sort_by_key(|candidate| (candidate.distance, candidate.first_move.rank()));
    candidates
}

pub(crate) fn reachable_after_move(
    map: &NavigationMap,
    state: &GameState,
    direction: Direction,
) -> u32 {
    let destination = direction.apply(state.you.head);
    if map.is_blocked(destination) {
        return 0;
    }

    let mut blocked = map.occupied.clone();

    if map.is_food(destination) {
        if let Some(tail) = state.you.body.last().copied() {
            blocked.set(tail, true);
        }
    }

    let mut visited = BoardMask::new(map.width, map.height);
    let mut queue = VecDeque::new();
    let mut reachable = 0_u32;

    visited.set(destination, true);
    queue.push_back(destination);

    while let Some(current) = queue.pop_front() {
        reachable += 1;

        for direction in Direction::ALL {
            let next = direction.apply(current);
            if !map.in_bounds(next)
                || blocked.contains(next)
                || map.lethal_head_danger.contains(next)
                || visited.contains(next)
            {
                continue;
            }

            visited.set(next, true);
            queue.push_back(next);
        }
    }

    reachable
}
