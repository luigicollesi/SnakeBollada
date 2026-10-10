use crate::board_mask::BoardMask;
use crate::direction::Direction;
use crate::{Coord, GameState};

#[derive(Debug, Clone)]
pub(crate) struct NavigationMap {
    pub(crate) width: u16,
    pub(crate) height: u16,
    pub(crate) occupied: BoardMask,
    pub(crate) hazards: BoardMask,
    pub(crate) lethal_head_danger: BoardMask,
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

    pub(crate) fn is_hazard(&self, coord: Coord) -> bool {
        self.hazards.contains(coord)
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
