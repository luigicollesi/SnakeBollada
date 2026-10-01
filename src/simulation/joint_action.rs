use crate::direction::Direction;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JointAction {
    moves: Vec<(String, Direction)>,
}

impl JointAction {
    pub(crate) fn new() -> Self {
        Self {
            moves: Vec::with_capacity(4),
        }
    }

    pub(crate) fn with_move(mut self, snake_id: impl Into<String>, direction: Direction) -> Self {
        let snake_id = snake_id.into();
        if let Some((_, existing)) = self
            .moves
            .iter_mut()
            .find(|(existing_id, _)| existing_id == &snake_id)
        {
            *existing = direction;
            return self;
        }

        self.moves.push((snake_id, direction));
        self
    }

    pub(crate) fn direction_for(&self, snake_id: &str) -> Option<Direction> {
        self.moves
            .iter()
            .find(|(existing_id, _)| existing_id == snake_id)
            .map(|(_, direction)| *direction)
    }

    pub(crate) fn len(&self) -> usize {
        self.moves.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_one_move_per_snake() {
        let action = JointAction::new()
            .with_move("ours", Direction::Up)
            .with_move("enemy", Direction::Left);

        assert_eq!(action.len(), 2);
        assert!(!action.is_empty());
        assert_eq!(action.direction_for("ours"), Some(Direction::Up));
        assert_eq!(action.direction_for("enemy"), Some(Direction::Left));
    }

    #[test]
    fn newer_move_replaces_previous_move_for_same_snake() {
        let action = JointAction::new()
            .with_move("enemy", Direction::Up)
            .with_move("enemy", Direction::Right);

        assert_eq!(action.len(), 1);
        assert_eq!(action.direction_for("enemy"), Some(Direction::Right));
    }

    #[test]
    fn preserves_insertion_order_for_deterministic_generation() {
        let action = JointAction::new()
            .with_move("ours", Direction::Up)
            .with_move("enemy-a", Direction::Left)
            .with_move("enemy-b", Direction::Down);

        assert_eq!(action.moves[0].0, "ours");
        assert_eq!(action.moves[1].0, "enemy-a");
        assert_eq!(action.moves[2].0, "enemy-b");
    }
}
