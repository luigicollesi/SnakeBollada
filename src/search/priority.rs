use std::cmp::Ordering;

use crate::direction::Direction;
use crate::search::graph::NodeId;

const REFUTATION_WEIGHT: i32 = 30;
const DANGER_WEIGHT: i32 = 25;
const RELEVANCE_WEIGHT: i32 = 20;
const TACTICAL_WEIGHT: i32 = 10;
const FORCING_WEIGHT: i32 = 10;
const UNCERTAINTY_WEIGHT: i32 = 5;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PrioritySignals {
    pub(crate) refutation: u16,
    pub(crate) danger: u16,
    pub(crate) relevance: u16,
    pub(crate) tactical: u16,
    pub(crate) forcing: u16,
    pub(crate) uncertainty: u16,
}

impl PrioritySignals {
    pub(crate) fn total_milli(self) -> i32 {
        weighted(self.refutation, REFUTATION_WEIGHT)
            .saturating_add(weighted(self.danger, DANGER_WEIGHT))
            .saturating_add(weighted(self.relevance, RELEVANCE_WEIGHT))
            .saturating_add(weighted(self.tactical, TACTICAL_WEIGHT))
            .saturating_add(weighted(self.forcing, FORCING_WEIGHT))
            .saturating_add(weighted(self.uncertainty, UNCERTAINTY_WEIGHT))
    }
}

fn weighted(value: u16, weight_percent: i32) -> i32 {
    i32::from(value)
        .saturating_mul(weight_percent)
        .saturating_div(100)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrontierPriority {
    pub(crate) total_milli: i32,
    pub(crate) refutation: u16,
    pub(crate) danger: u16,
    pub(crate) relevance: u16,
    pub(crate) tactical: u16,
    pub(crate) forcing: u16,
    pub(crate) uncertainty: u16,
    root_direction_rank: u8,
    depth: u8,
    node_id: NodeId,
}

impl FrontierPriority {
    pub(crate) fn new(
        signals: PrioritySignals,
        root_direction: Direction,
        depth: u8,
        node_id: NodeId,
    ) -> Self {
        Self {
            total_milli: signals.total_milli(),
            refutation: signals.refutation,
            danger: signals.danger,
            relevance: signals.relevance,
            tactical: signals.tactical,
            forcing: signals.forcing,
            uncertainty: signals.uncertainty,
            root_direction_rank: root_direction.rank(),
            depth,
            node_id,
        }
    }
}

impl Ord for FrontierPriority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.total_milli
            .cmp(&other.total_milli)
            .then_with(|| self.refutation.cmp(&other.refutation))
            .then_with(|| self.danger.cmp(&other.danger))
            .then_with(|| self.relevance.cmp(&other.relevance))
            .then_with(|| self.tactical.cmp(&other.tactical))
            .then_with(|| self.forcing.cmp(&other.forcing))
            .then_with(|| self.uncertainty.cmp(&other.uncertainty))
            .then_with(|| other.depth.cmp(&self.depth))
            .then_with(|| other.root_direction_rank.cmp(&self.root_direction_rank))
            .then_with(|| other.node_id.cmp(&self.node_id))
    }
}

impl PartialOrd for FrontierPriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(
        refutation: u16,
        danger: u16,
        relevance: u16,
        tactical: u16,
        forcing: u16,
        uncertainty: u16,
    ) -> PrioritySignals {
        PrioritySignals {
            refutation,
            danger,
            relevance,
            tactical,
            forcing,
            uncertainty,
        }
    }

    #[test]
    fn refutation_and_danger_dominate_equal_relevance() {
        let dangerous = FrontierPriority::new(
            signals(900, 800, 500, 100, 100, 100),
            Direction::Up,
            4,
            1,
        );
        let merely_promising = FrontierPriority::new(
            signals(100, 100, 500, 900, 500, 500),
            Direction::Right,
            4,
            2,
        );

        assert!(dangerous > merely_promising);
    }

    #[test]
    fn shallower_node_wins_exact_tie() {
        let same = signals(500, 500, 500, 500, 500, 500);
        let shallow = FrontierPriority::new(same, Direction::Up, 4, 1);
        let deep = FrontierPriority::new(same, Direction::Up, 8, 2);

        assert!(shallow > deep);
    }

    #[test]
    fn priority_is_integer_and_deterministic() {
        let input = signals(1000, 750, 500, 250, 125, 60);
        let first = FrontierPriority::new(input, Direction::Left, 5, 42);
        let second = FrontierPriority::new(input, Direction::Left, 5, 42);

        assert_eq!(first, second);
        assert_eq!(first.total_milli, input.total_milli());
    }
}
