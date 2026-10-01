use crate::Coord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoardMask {
    width: u16,
    height: u16,
    bits: Vec<u64>,
}

impl BoardMask {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        let cells = usize::from(width) * usize::from(height);
        let words = cells.div_ceil(64);

        Self {
            width,
            height,
            bits: vec![0; words],
        }
    }

    pub(crate) fn from_coords(
        width: u16,
        height: u16,
        coords: impl IntoIterator<Item = Coord>,
    ) -> Self {
        let mut mask = Self::new(width, height);
        for coord in coords {
            mask.set(coord, true);
        }
        mask
    }

    pub(crate) fn contains(&self, coord: Coord) -> bool {
        let Some(index) = self.index(coord) else {
            return false;
        };

        let word = index / 64;
        let bit = index % 64;
        (self.bits[word] & (1_u64 << bit)) != 0
    }

    pub(crate) fn set(&mut self, coord: Coord, occupied: bool) {
        let Some(index) = self.index(coord) else {
            return;
        };

        let word = index / 64;
        let bit = index % 64;
        let flag = 1_u64 << bit;

        if occupied {
            self.bits[word] |= flag;
        } else {
            self.bits[word] &= !flag;
        }
    }

    #[cfg(test)]
    pub(crate) fn count_ones(&self) -> u32 {
        self.bits.iter().map(|word| word.count_ones()).sum()
    }

    fn index(&self, coord: Coord) -> Option<usize> {
        if coord.x < 0
            || coord.y < 0
            || coord.x >= i32::from(self.width)
            || coord.y >= i32::from(self.height)
        {
            return None;
        }

        Some(coord.y as usize * usize::from(self.width) + coord.x as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_boards_larger_than_one_word() {
        let mut mask = BoardMask::new(11, 11);
        mask.set(Coord { x: 0, y: 0 }, true);
        mask.set(Coord { x: 10, y: 10 }, true);

        assert!(mask.contains(Coord { x: 0, y: 0 }));
        assert!(mask.contains(Coord { x: 10, y: 10 }));
        assert_eq!(mask.count_ones(), 2);
    }
}
