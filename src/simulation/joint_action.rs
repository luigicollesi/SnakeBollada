use crate::direction::Direction;
use crate::simulation::state::ActorIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JointAction {
    moves: Vec<Option<Direction>>,
}

impl JointAction {
    pub(crate) fn new() -> Self {
        Self { moves: Vec::new() }
    }

    pub(crate) fn with_move(mut self, actor: ActorIndex, direction: Direction) -> Self {
        let index = actor.as_usize();
        if self.moves.len() <= index {
            self.moves.resize(index + 1, None);
        }
        self.moves[index] = Some(direction);
        self
    }

    pub(crate) fn direction_for(&self, actor: ActorIndex) -> Option<Direction> {
        self.moves.get(actor.as_usize()).copied().flatten()
    }

    pub(crate) fn len(&self) -> usize {
        self.moves
            .iter()
            .filter(|direction| direction.is_some())
            .count()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.moves.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(index: usize) -> ActorIndex {
        ActorIndex::new(index).unwrap()
    }

    #[test]
    fn stores_one_move_per_actor() {
        let action = JointAction::new()
            .with_move(actor(0), Direction::Up)
            .with_move(actor(1), Direction::Left);

        assert_eq!(action.len(), 2);
        assert!(!action.is_empty());
        assert_eq!(action.direction_for(actor(0)), Some(Direction::Up));
        assert_eq!(action.direction_for(actor(1)), Some(Direction::Left));
    }

    #[test]
    fn newer_move_replaces_previous_move_for_same_actor() {
        let action = JointAction::new()
            .with_move(actor(1), Direction::Up)
            .with_move(actor(1), Direction::Right);

        assert_eq!(action.len(), 1);
        assert_eq!(action.direction_for(actor(1)), Some(Direction::Right));
    }

    #[test]
    fn sparse_actor_indices_are_supported() {
        let action = JointAction::new()
            .with_move(actor(0), Direction::Up)
            .with_move(actor(3), Direction::Down);

        assert_eq!(action.len(), 2);
        assert_eq!(action.direction_for(actor(1)), None);
        assert_eq!(action.direction_for(actor(3)), Some(Direction::Down));
    }
}
