#![allow(dead_code)]

use std::sync::Arc;

use crate::direction::Direction;
use crate::evaluation::ActorVec;
use crate::simulation::joint_action::JointAction;

use super::bounds::ValueBound;
use super::forecast::ForecastCertainty;
use super::graph::NodeId;

pub(crate) const SEED_DEPTH: u8 = 3;
pub(crate) const BEAM_WIDTH: usize = 3;
pub(crate) const ROUND_DEPTH: u8 = 2;
pub(crate) const MAX_BEAM_DEPTH: u8 = 21;

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BeamPathNode {
    Step(BeamStep),
    Concat { left: BeamPath, right: BeamPath },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BeamPath {
    root: Option<Arc<BeamPathNode>>,
    len: usize,
}

impl BeamPath {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn single(step: BeamStep) -> Self {
        Self {
            root: Some(Arc::new(BeamPathNode::Step(step))),
            len: 1,
        }
    }

    pub(crate) fn prepend(&self, step: BeamStep) -> Self {
        Self::single(step).concat(self)
    }

    pub(crate) fn concat(&self, other: &Self) -> Self {
        if self.is_empty() {
            return other.clone();
        }
        if other.is_empty() {
            return self.clone();
        }

        Self {
            root: Some(Arc::new(BeamPathNode::Concat {
                left: self.clone(),
                right: other.clone(),
            })),
            len: self.len.saturating_add(other.len),
        }
    }

    pub(crate) const fn len(&self) -> usize {
        self.len
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(crate) fn first(&self) -> Option<&BeamStep> {
        first_step(self.root.as_deref()?)
    }

    pub(crate) fn last(&self) -> Option<&BeamStep> {
        last_step(self.root.as_deref()?)
    }

    pub(crate) fn steps(&self) -> Vec<&BeamStep> {
        let mut steps = Vec::with_capacity(self.len);
        if let Some(root) = self.root.as_deref() {
            collect_steps(root, &mut steps);
        }
        steps
    }
}

fn first_step(node: &BeamPathNode) -> Option<&BeamStep> {
    match node {
        BeamPathNode::Step(step) => Some(step),
        BeamPathNode::Concat { left, right } => left.first().or_else(|| right.first()),
    }
}

fn last_step(node: &BeamPathNode) -> Option<&BeamStep> {
    match node {
        BeamPathNode::Step(step) => Some(step),
        BeamPathNode::Concat { left, right } => right.last().or_else(|| left.last()),
    }
}

fn collect_steps<'a>(node: &'a BeamPathNode, steps: &mut Vec<&'a BeamStep>) {
    match node {
        BeamPathNode::Step(step) => steps.push(step),
        BeamPathNode::Concat { left, right } => {
            if let Some(left_root) = left.root.as_deref() {
                collect_steps(left_root, steps);
            }
            if let Some(right_root) = right.root.as_deref() {
                collect_steps(right_root, steps);
            }
        }
    }
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
    pub(crate) actor_utility_totals: ActorVec<i64>,
    pub(crate) value: i64,
    pub(crate) terminal: LineTerminal,
    pub(crate) certainty: ForecastCertainty,
    pub(crate) bound: ValueBound,
    pub(crate) path: BeamPath,
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
            actor_utility_totals: ActorVec::new(),
            value,
            terminal,
            certainty: ForecastCertainty::Deterministic,
            bound: ValueBound::exact(value),
            path: BeamPath::empty(),
        }
    }

    pub(crate) fn is_viable(&self) -> bool {
        !self.is_confirmed_loss()
    }

    pub(crate) fn is_confirmed_loss(&self) -> bool {
        self.terminal == LineTerminal::Lost && self.certainty == ForecastCertainty::Deterministic
    }

    pub(crate) fn is_confirmed_win(&self) -> bool {
        self.terminal == LineTerminal::Won && self.certainty == ForecastCertainty::Deterministic
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
        !candidate_lines.is_empty()
            && candidate_lines.len() <= BEAM_WIDTH
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

    pub(crate) fn has_running_lines(&self) -> bool {
        self.lines
            .iter()
            .any(|line| line.terminal == LineTerminal::Running)
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

    if ranked.is_empty() {
        return Vec::new();
    }

    let viable = ranked
        .iter()
        .filter(|line| line.is_viable())
        .cloned()
        .collect::<Vec<_>>();
    let pool = if viable.is_empty() { &ranked } else { &viable };

    let mut selected = Vec::with_capacity(BEAM_WIDTH);
    for candidate in pool {
        if selected
            .iter()
            .any(|chosen: &BeamLine| chosen.root_direction == candidate.root_direction)
        {
            continue;
        }

        selected.push(candidate.clone());
        if selected.len() == BEAM_WIDTH {
            break;
        }
    }

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
    fn persistent_path_concatenates_without_copying_step_vectors() {
        let first = BeamPath::single(BeamStep {
            node: 1,
            joint_action: JointAction::new(),
            child: 2,
        });
        let second = BeamPath::single(BeamStep {
            node: 2,
            joint_action: JointAction::new(),
            child: 3,
        });

        let combined = first.concat(&second);

        assert_eq!(combined.len(), 2);
        assert_eq!(combined.first().unwrap().node, 1);
        assert_eq!(combined.last().unwrap().child, 3);
        assert_eq!(combined.steps().len(), 2);
    }

    #[test]
    fn seed_beam_keeps_only_best_line_per_root_direction() {
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
        assert!(!beam.iter().any(|line| line.id == LineId(3)));
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
        assert_eq!(beam[0].id, LineId(1));
        assert_eq!(beam[0].root_direction, Direction::Right);
        assert!(beam[0].is_viable());
    }

    #[test]
    fn single_viable_root_direction_collapses_to_one_route() {
        let mut losing_up = line(4, Direction::Up, 950);
        losing_up.terminal = LineTerminal::Lost;
        let candidates = vec![
            line(1, Direction::Right, 900),
            line(2, Direction::Right, 850),
            line(3, Direction::Right, 800),
            losing_up,
        ];

        let beam = select_seed_beam(&candidates);

        assert_eq!(beam.len(), 1);
        assert_eq!(beam[0].id, LineId(1));
        assert_eq!(beam[0].root_direction, Direction::Right);
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
    fn provisional_loss_remains_viable_for_comparison() {
        let mut loss = line(9, Direction::Down, -50_000);
        loss.terminal = LineTerminal::Lost;
        loss.certainty = ForecastCertainty::FoodProvisional;

        assert!(loss.is_viable());
        assert!(!loss.is_confirmed_loss());
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
