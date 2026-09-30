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
