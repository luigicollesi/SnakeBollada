use serde::{Deserialize, Serialize};

use crate::Coord;

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Direction {
    Up,
    Right,
    Down,
    Left,
}

impl Direction {
    pub(crate) const ALL: [Direction; 4] = [
        Direction::Up,
        Direction::Right,
        Direction::Down,
        Direction::Left,
    ];

    pub(crate) const fn apply(self, coord: Coord) -> Coord {
        match self {
            Direction::Up => Coord {
                x: coord.x,
                y: coord.y + 1,
            },
            Direction::Right => Coord {
                x: coord.x + 1,
                y: coord.y,
            },
            Direction::Down => Coord {
                x: coord.x,
                y: coord.y - 1,
            },
            Direction::Left => Coord {
                x: coord.x - 1,
                y: coord.y,
            },
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Right => "right",
            Direction::Down => "down",
            Direction::Left => "left",
        }
    }

    pub(crate) const fn rank(self) -> u8 {
        match self {
            Direction::Up => 0,
            Direction::Right => 1,
            Direction::Down => 2,
            Direction::Left => 3,
        }
    }

    pub(crate) fn from_heads(previous: Coord, current: Coord) -> Option<Self> {
        match (current.x - previous.x, current.y - previous.y) {
            (0, 1) => Some(Direction::Up),
            (1, 0) => Some(Direction::Right),
            (0, -1) => Some(Direction::Down),
            (-1, 0) => Some(Direction::Left),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MoveMask(u8);

impl MoveMask {
    pub(crate) const fn empty() -> Self {
        Self(0)
    }

    pub(crate) const fn single(direction: Direction) -> Self {
        Self(1 << direction.rank())
    }

    pub(crate) fn insert(&mut self, direction: Direction) {
        self.0 |= 1 << direction.rank();
    }

    pub(crate) const fn contains(self, direction: Direction) -> bool {
        self.0 & (1 << direction.rank()) != 0
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) const fn len(self) -> u8 {
        self.0.count_ones() as u8
    }

    pub(crate) fn union_with(&mut self, other: MoveMask) -> bool {
        let previous = self.0;
        self.0 |= other.0;
        self.0 != previous
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = Direction> {
        Direction::ALL
            .into_iter()
            .filter(move |direction| self.contains(*direction))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_mask_preserves_multiple_directions() {
        let mut mask = MoveMask::empty();
        mask.insert(Direction::Up);
        mask.insert(Direction::Right);

        assert!(mask.contains(Direction::Up));
        assert!(mask.contains(Direction::Right));
        assert_eq!(mask.len(), 2);
    }

    #[test]
    fn move_mask_iteration_is_stable() {
        let mut mask = MoveMask::empty();
        mask.insert(Direction::Right);
        mask.insert(Direction::Up);

        assert_eq!(
            mask.iter().collect::<Vec<_>>(),
            vec![Direction::Up, Direction::Right]
        );
    }
}
