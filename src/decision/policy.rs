use crate::direction::Direction;
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ReservedCellPolicy {
    pub(crate) edge_penalty: f32,
    pub(crate) corner_penalty: f32,
}

impl Default for ReservedCellPolicy {
    fn default() -> Self {
        Self {
            edge_penalty: 0.10,
            corner_penalty: 0.25,
        }
    }
}

impl ReservedCellPolicy {
    pub(crate) fn penalty(&self, state: &SimulatedGameState, direction: Direction) -> f32 {
        let Some(head) = state
            .snake(&state.our_snake_id)
            .and_then(|snake| snake.head())
        else {
            return 0.0;
        };

        let target = direction.apply(head);
        if target.x < 0
            || target.y < 0
            || target.x >= state.width as i32
            || target.y >= state.height as i32
        {
            return self.corner_penalty;
        }

        let on_horizontal_edge = target.x == 0 || target.x == state.width as i32 - 1;
        let on_vertical_edge = target.y == 0 || target.y == state.height as i32 - 1;

        match (on_horizontal_edge, on_vertical_edge) {
            (true, true) => self.corner_penalty,
            (true, false) | (false, true) => self.edge_penalty,
            (false, false) => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};
    use crate::Coord;

    use super::*;

    fn state(head: Coord) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 5,
            height: 5,
            food: vec![],
            hazards: vec![],
            snakes: vec![SimulatedSnake {
                id: "ours".to_string(),
                health: 100,
                body: vec![head],
                alive: true,
            }],
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
    fn interior_has_no_penalty() {
        let policy = ReservedCellPolicy::default();
        assert_eq!(
            policy.penalty(&state(Coord { x: 2, y: 2 }), Direction::Up),
            0.0
        );
    }

    #[test]
    fn corner_is_more_reserved_than_edge() {
        let policy = ReservedCellPolicy::default();
        let edge = policy.penalty(&state(Coord { x: 1, y: 1 }), Direction::Left);
        let corner = policy.penalty(&state(Coord { x: 1, y: 0 }), Direction::Left);

        assert!(corner > edge);
    }
}
