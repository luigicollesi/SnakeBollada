#![allow(dead_code)]

use crate::direction::Direction;
use crate::evaluation::TransitionScore;
use crate::simulation::joint_action::JointAction;

use super::bounds::ValueBound;
use super::graph::NodeId;

pub(crate) const SEED_DEPTH: u8 = 3;
pub(crate) const BEAM_WIDTH: usize = 3;
pub(crate) const ROUND_DEPTH: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct LineId(pub(crate) u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineTerminal {
    Running,
    Won,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BeamStep {
    pub(crate) node: NodeId,
    pub(crate) joint_action: JointAction,
    pub(crate) child: NodeId,
    pub(crate) transition: TransitionScore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BeamLine {
    pub(crate) id: LineId,
    pub(crate) root_direction: Direction,
    pub(crate) depth: u8,
    pub(crate) benefit_total: i64,
    pub(crate) harm_total: i64,
    pub(crate) our_utility_total: i64,
    pub(crate) opponent_utility_total: i64,
    pub(crate) value: i64,
    pub(crate) terminal: LineTerminal,
    pub(crate) bound: ValueBound,
    pub(crate) steps: Vec<BeamStep>,
}

impl BeamLine {
    pub(crate) fn exact(
        id: u32,
        root_direction: Direction,
        depth: u8,
        benefit_total: i64,
        harm_total: i64,
        terminal: LineTerminal,
    ) -> Self {
        let value = benefit_total.saturating_sub(harm_total);
        Self {
            id: LineId(id),
            root_direction,
            depth,
            benefit_total,
            harm_total,
            our_utility_total: value,
            opponent_utility_total: 0,
            value,
            terminal,
            bound: ValueBound::exact(value),
            steps: Vec::new(),
        }
    }

    pub(crate) fn is_viable(&self) -> bool {
        self.terminal != LineTerminal::Lost
    }

    pub(crate) fn completes_depth(&self, target_depth: u8) -> bool {
        self.terminal != LineTerminal::Running || self.depth >= target_depth
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BeamCheckpoint {
    pub(crate) completed_depth: u8,
    pub(crate) lines: Vec<BeamLine>,
}

impl BeamCheckpoint {
    pub(crate) fn new(lines: Vec<BeamLine>) -> Option<Self> {
        let completed_depth = lines
            .iter()
            .filter(|line| line.terminal == LineTerminal::Running)
            .map(|line| line.depth)
            .min()
            .or_else(|| lines.iter().map(|line| line.depth).max())?;

        Some(Self {
            completed_depth,
            lines,
        })
    }

    pub(crate) fn can_commit(&self, candidate_lines: &[BeamLine]) -> bool {
        let target_depth = self.completed_depth.saturating_add(ROUND_DEPTH);
        candidate_lines.len() == self.lines.len()
            && candidate_lines
                .iter()
                .all(|line| line.completes_depth(target_depth) && line.bound.is_exact())
    }

    pub(crate) fn best_line(&self) -> Option<&BeamLine> {
        self.lines.iter().max_by(|left, right| {
            left.value
                .cmp(&right.value)
                .then_with(|| right.root_direction.rank().cmp(&left.root_direction.rank()))
                .then_with(|| right.id.cmp(&left.id))
        })
    }
}

pub(crate) fn select_seed_beam(candidates: &[BeamLine]) -> Vec<BeamLine> {
    let mut ranked = candidates.to_vec();
    ranked.sort_by(|left, right| {
        right
            .value
            .cmp(&left.value)
            .then_with(|| left.root_direction.rank().cmp(&right.root_direction.rank()))
            .then_with(|| left.id.cmp(&right.id))
    });

    let viable = ranked
        .iter()
        .filter(|line| line.is_viable())
        .cloned()
        .collect::<Vec<_>>();
    let pool = if viable.is_empty() { &ranked } else { &viable };

    let mut selected = Vec::with_capacity(BEAM_WIDTH);
    for direction in Direction::ALL {
        let Some(best_for_direction) = pool
            .iter()
            .filter(|line| line.root_direction == direction)
            .max_by(|left, right| {
                left.value
                    .cmp(&right.value)
                    .then_with(|| right.id.cmp(&left.id))
            })
            .cloned()
        else {
            continue;
        };
        selected.push(best_for_direction);
    }

    selected.sort_by(|left, right| {
        right
            .value
            .cmp(&left.value)
            .then_with(|| left.root_direction.rank().cmp(&right.root_direction.rank()))
            .then_with(|| left.id.cmp(&right.id))
    });
    selected.truncate(BEAM_WIDTH);
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: u32, direction: Direction, value: i64) -> BeamLine {
        BeamLine::exact(
            id,
            direction,
            SEED_DEPTH,
            value.max(0),
            value.max(0).saturating_sub(value),
            LineTerminal::Running,
        )
    }

    #[test]
    fn seed_beam_keeps_at_most_one_line_per_root_direction() {
        let candidates = vec![
            line(1, Direction::Right, 920),
            line(2, Direction::Right, 900),
            line(3, Direction::Right, 880),
            line(4, Direction::Up, 850),
            line(5, Direction::Left, 100),
        ];

        let beam = select_seed_beam(&candidates);

        assert_eq!(beam.len(), BEAM_WIDTH);
        assert!(beam.iter().any(|line| line.id == LineId(1)));
        assert!(!beam.iter().any(|line| line.id == LineId(2)));
        assert!(beam.iter().any(|line| line.root_direction == Direction::Up));
        assert!(beam
            .iter()
            .any(|line| line.root_direction == Direction::Left));
    }

    #[test]
    fn lines_from_same_direction_are_never_summed() {
        let checkpoint = BeamCheckpoint::new(vec![
            line(1, Direction::Right, 700),
            line(2, Direction::Right, 600),
            line(3, Direction::Up, 800),
        ])
        .unwrap();

        let best = checkpoint.best_line().unwrap();

        assert_eq!(best.id, LineId(3));
        assert_eq!(best.root_direction, Direction::Up);
        assert_eq!(best.value, 800);
    }

    #[test]
    fn diversity_is_not_forced_through_a_losing_line() {
        let mut losing = line(2, Direction::Up, 1000);
        losing.terminal = LineTerminal::Lost;
        let candidates = vec![
            line(1, Direction::Right, 900),
            losing,
            line(3, Direction::Right, 800),
            line(4, Direction::Right, 700),
        ];

        let beam = select_seed_beam(&candidates);

        assert_eq!(beam.len(), 1);
        assert!(beam
            .iter()
            .all(|line| line.root_direction == Direction::Right));
        assert!(beam.iter().all(BeamLine::is_viable));
    }

    #[test]
    fn incomplete_round_cannot_replace_checkpoint() {
        let checkpoint = BeamCheckpoint::new(vec![
            line(1, Direction::Right, 900),
            line(2, Direction::Up, 800),
            line(3, Direction::Right, 700),
        ])
        .unwrap();
        let mut next = checkpoint.lines.clone();
        next[0].depth = 5;
        next[1].depth = 5;
        next[2].depth = 4;

        assert!(!checkpoint.can_commit(&next));

        next[2].depth = 5;
        assert!(checkpoint.can_commit(&next));
    }

    #[test]
    fn terminal_line_counts_as_complete_for_later_rounds() {
        let checkpoint = BeamCheckpoint::new(vec![
            line(1, Direction::Right, 900),
            line(2, Direction::Up, 800),
            line(3, Direction::Right, 700),
        ])
        .unwrap();
        let mut next = checkpoint.lines.clone();
        next[0].terminal = LineTerminal::Won;
        next[0].depth = 4;
        next[1].depth = 5;
        next[2].depth = 5;

        assert!(checkpoint.can_commit(&next));
    }
}
