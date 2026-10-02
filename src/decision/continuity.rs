use std::sync::Arc;

use crate::direction::Direction;
use crate::search::beam::BeamLine;
use crate::search::graph::FutureGraph;

use super::state_key::StateKey;

const MAX_PLANNED_STEPS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlannedStep {
    expected_state: Arc<StateKey>,
    direction: Direction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DecisionContinuity {
    steps: Vec<PlannedStep>,
}

impl DecisionContinuity {
    pub(crate) fn incumbent_for(&mut self, actual: &StateKey) -> Option<Direction> {
        let Some(step) = self.steps.first() else {
            return None;
        };

        if step.expected_state.as_ref() != actual {
            self.clear();
            return None;
        }

        let direction = step.direction;
        self.steps.remove(0);
        Some(direction)
    }

    pub(crate) fn replace_from_line(&mut self, graph: &FutureGraph, line: &BeamLine) {
        self.clear();

        for step in line.path.steps().into_iter().skip(1).take(MAX_PLANNED_STEPS) {
            let node = graph.node(step.node);
            let Some(our_actor) = node.state.actor_index(&node.state.our_snake_id) else {
                break;
            };
            let Some(direction) = step.joint_action.direction_for(our_actor) else {
                break;
            };

            self.steps.push(PlannedStep {
                expected_state: Arc::clone(&node.key),
                direction,
            });
        }
    }

    pub(crate) fn clear(&mut self) {
        self.steps.clear();
    }

    #[cfg(test)]
    fn set_test_step(&mut self, expected_state: Arc<StateKey>, direction: Direction) {
        self.steps = vec![PlannedStep {
            expected_state,
            direction,
        }];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
    use crate::Coord;

    fn state(turn: i32, head_x: i32) -> SimulatedGameState {
        SimulatedGameState {
            turn,
            width: 7,
            height: 7,
            food: vec![],
            hazards: vec![],
            snakes: vec![SimulatedSnake {
                id: "ours".to_string(),
                health: 100,
                body: vec![Coord { x: head_x, y: 2 }],
                alive: true,
            }],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    #[test]
    fn exact_expected_state_yields_incumbent_direction_once() {
        let expected = Arc::new(StateKey::from_beam_state(&state(2, 3)));
        let mut continuity = DecisionContinuity::default();
        continuity.set_test_step(Arc::clone(&expected), Direction::Right);

        assert_eq!(
            continuity.incumbent_for(expected.as_ref()),
            Some(Direction::Right)
        );
        assert_eq!(continuity.incumbent_for(expected.as_ref()), None);
    }

    #[test]
    fn unexpected_state_invalidates_plan() {
        let expected = Arc::new(StateKey::from_beam_state(&state(2, 3)));
        let actual = StateKey::from_beam_state(&state(2, 4));
        let mut continuity = DecisionContinuity::default();
        continuity.set_test_step(expected, Direction::Right);

        assert_eq!(continuity.incumbent_for(&actual), None);
        assert!(continuity.steps.is_empty());
    }
}
