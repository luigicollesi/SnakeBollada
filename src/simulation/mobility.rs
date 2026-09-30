use std::collections::VecDeque;

use crate::board_mask::BoardMask;
use crate::direction::{Direction, MoveMask};
use crate::Coord;

use super::state::{SimulatedGameState, SimulatedSnake};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeterministicMoveBlock {
    OutOfBounds,
    DeterministicBodyCollision,
    FatalHazard,
    Starvation,
}

#[derive(Debug, Clone)]
pub(crate) struct MobilityAnalysis {
    width: u16,
    height: u16,
    retained_body: BoardMask,
}

impl MobilityAnalysis {
    pub(crate) fn from_state(state: &SimulatedGameState) -> Self {
        let width = state.width as u16;
        let height = state.height as u16;
        let mut retained_body = BoardMask::new(width, height);

        for snake in state.snakes.iter().filter(|snake| snake.alive) {
            let retained_len = snake.body.len().saturating_sub(1);
            for segment in snake.body.iter().take(retained_len) {
                retained_body.set(*segment, true);
            }
        }

        Self {
            width,
            height,
            retained_body,
        }
    }

    pub(crate) fn classify_move(
        &self,
        state: &SimulatedGameState,
        snake: &SimulatedSnake,
        direction: Direction,
    ) -> Option<DeterministicMoveBlock> {
        let head = snake.head()?;
        let target = direction.apply(head);

        if !self.in_bounds(target) {
            return Some(DeterministicMoveBlock::OutOfBounds);
        }

        if self.retained_body.contains(target) {
            return Some(DeterministicMoveBlock::DeterministicBodyCollision);
        }

        if state.food.contains(&target) {
            return None;
        }

        let health_after_step = snake.health.saturating_sub(1);
        if health_after_step <= 0 {
            return Some(DeterministicMoveBlock::Starvation);
        }

        let hazard_stacks = state
            .hazards
            .iter()
            .filter(|hazard| **hazard == target)
            .count();
        let hazard_damage = state.rules.hazard_damage_per_turn.max(0);
        let total_hazard_damage = hazard_damage.saturating_mul(hazard_stacks as i32);

        if health_after_step.saturating_sub(total_hazard_damage) <= 0 {
            return Some(DeterministicMoveBlock::FatalHazard);
        }

        None
    }

    pub(crate) fn deterministic_moves_for(
        &self,
        state: &SimulatedGameState,
        snake_id: &str,
    ) -> MoveMask {
        let Some(snake) = state.snake(snake_id).filter(|snake| snake.alive) else {
            return MoveMask::empty();
        };

        MoveMask::from_iter(
            Direction::ALL
                .into_iter()
                .filter(|direction| self.classify_move(state, snake, *direction).is_none()),
        )
    }

    pub(crate) fn reachable_space(
        &self,
        state: &SimulatedGameState,
        snake_id: &str,
        direction: Direction,
    ) -> u32 {
        let Some(snake) = state.snake(snake_id).filter(|snake| snake.alive) else {
            return 0;
        };
        if self.classify_move(state, snake, direction).is_some() {
            return 0;
        }

        let Some(head) = snake.head() else {
            return 0;
        };
        let destination = direction.apply(head);

        let mut visited = BoardMask::new(self.width, self.height);
        let mut queue = VecDeque::new();
        let mut reachable = 0_u32;

        visited.set(destination, true);
        queue.push_back(destination);

        while let Some(current) = queue.pop_front() {
            reachable += 1;

            for next_direction in Direction::ALL {
                let next = next_direction.apply(current);
                if !self.in_bounds(next)
                    || self.retained_body.contains(next)
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

    fn in_bounds(&self, coord: Coord) -> bool {
        coord.x >= 0
            && coord.y >= 0
            && coord.x < i32::from(self.width)
            && coord.y < i32::from(self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{AggressionState, RulesContext};

    fn snake(id: &str, health: i32, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes,
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
            aggression: AggressionState::default(),
        }
    }

    #[test]
    fn wall_and_neck_are_hard_blocked() {
        let initial = state(vec![snake("ours", 100, &[(0, 1), (0, 0), (1, 0)])]);
        let mobility = MobilityAnalysis::from_state(&initial);
        let ours = initial.snake("ours").unwrap();

        assert_eq!(
            mobility.classify_move(&initial, ours, Direction::Left),
            Some(DeterministicMoveBlock::OutOfBounds)
        );
        assert_eq!(
            mobility.classify_move(&initial, ours, Direction::Down),
            Some(DeterministicMoveBlock::DeterministicBodyCollision)
        );
    }

    #[test]
    fn unique_tail_vacates_but_stacked_tail_stays_occupied() {
        let unique = state(vec![snake("ours", 100, &[(2, 2), (2, 1), (1, 1), (1, 2)])]);
        let mobility = MobilityAnalysis::from_state(&unique);
        assert_eq!(
            mobility.classify_move(&unique, unique.snake("ours").unwrap(), Direction::Left),
            None
        );

        let stacked = state(vec![snake("ours", 100, &[(2, 2), (2, 1), (1, 2), (1, 2)])]);
        let mobility = MobilityAnalysis::from_state(&stacked);
        assert_eq!(
            mobility.classify_move(&stacked, stacked.snake("ours").unwrap(), Direction::Left),
            Some(DeterministicMoveBlock::DeterministicBodyCollision)
        );
    }

    #[test]
    fn another_snakes_current_head_is_retained_body() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 2)]),
            snake("enemy", 100, &[(3, 2), (3, 1)]),
        ]);
        let mobility = MobilityAnalysis::from_state(&initial);

        assert_eq!(
            mobility.classify_move(&initial, initial.snake("ours").unwrap(), Direction::Right),
            Some(DeterministicMoveBlock::DeterministicBodyCollision)
        );
    }

    #[test]
    fn fatal_hazard_is_blocked_but_nonfatal_hazard_remains() {
        let mut initial = state(vec![snake("ours", 15, &[(2, 2)])]);
        initial.rules.hazard_damage_per_turn = 14;
        initial.hazards = vec![Coord { x: 3, y: 2 }];
        let mobility = MobilityAnalysis::from_state(&initial);

        assert_eq!(
            mobility.classify_move(&initial, initial.snake("ours").unwrap(), Direction::Right),
            Some(DeterministicMoveBlock::FatalHazard)
        );

        initial.snake_mut("ours").unwrap().health = 16;
        let mobility = MobilityAnalysis::from_state(&initial);
        assert_eq!(
            mobility.classify_move(&initial, initial.snake("ours").unwrap(), Direction::Right),
            None
        );
    }

    #[test]
    fn food_on_hazard_remains_legal_even_at_one_health() {
        let mut initial = state(vec![snake("ours", 1, &[(2, 2)])]);
        initial.rules.hazard_damage_per_turn = 100;
        initial.food = vec![Coord { x: 3, y: 2 }];
        initial.hazards = vec![Coord { x: 3, y: 2 }];
        let mobility = MobilityAnalysis::from_state(&initial);

        assert_eq!(
            mobility.classify_move(&initial, initial.snake("ours").unwrap(), Direction::Right),
            None
        );
    }

    #[test]
    fn reachable_space_uses_vacated_tails_and_retained_bodies() {
        let initial = state(vec![
            snake("ours", 100, &[(1, 1), (1, 0)]),
            snake("wall", 100, &[(3, 3), (3, 2), (3, 1), (3, 0)]),
        ]);
        let mobility = MobilityAnalysis::from_state(&initial);

        let space = mobility.reachable_space(&initial, "ours", Direction::Right);

        assert!(space > 0);
        assert!(space < initial.width * initial.height);
    }
}
