//! Cartesian product of legal simultaneous snake moves.
use crate::direction::{Direction, MoveMask};
use crate::simulation::joint_action::JointAction;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::{ActorIndex, SimulatedGameState};

#[derive(Debug, Clone)]
pub(crate) struct JointActionGenerator {
    options: Vec<(ActorIndex, Vec<Direction>)>,
    indices: Vec<usize>,
    done: bool,
}

impl JointActionGenerator {
    pub(crate) fn new(state: &SimulatedGameState, our_moves: MoveMask, mobility: &MobilityAnalysis) -> Self {
        let Some(our_actor) = state.actor_index(&state.our_snake_id) else {
            return Self { options: Vec::new(), indices: Vec::new(), done: true };
        };
        let mut options = vec![(our_actor, normalized_moves(our_moves).iter().collect::<Vec<_>>())];

        let mut enemies = state.snakes.iter().enumerate()
            .filter(|(_, snake)| snake.alive && snake.id != state.our_snake_id)
            .filter_map(|(index, snake)| ActorIndex::new(index).map(|actor| (actor, snake)))
            .collect::<Vec<_>>();
        enemies.sort_by(|(_, left), (_, right)| left.id.cmp(&right.id));

        for (actor, snake) in enemies {
            let moves = mobility.deterministic_moves_for(state, &snake.id);
            let moves = if moves.is_empty() {
                mobility.in_bounds_moves_for(state, &snake.id)
            } else {
                moves
            };
            options.push((actor, normalized_moves(moves).iter().collect()));
        }

        Self { indices: vec![0; options.len()], options, done: false }
    }

    fn advance(&mut self) {
        for index in (0..self.indices.len()).rev() {
            self.indices[index] += 1;
            if self.indices[index] < self.options[index].1.len() {
                return;
            }
            self.indices[index] = 0;
        }
        self.done = true;
    }
}

impl Iterator for JointActionGenerator {
    type Item = JointAction;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let mut action = JointAction::new();
        for (index, (actor, directions)) in self.options.iter().enumerate() {
            action = action.with_move(*actor, directions[self.indices[index]]);
        }
        self.advance();
        Some(action)
    }
}

fn normalized_moves(moves: MoveMask) -> MoveMask {
    if moves.is_empty() { MoveMask::all() } else { moves }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedSnake};
    use crate::Coord;

    fn state() -> SimulatedGameState {
        SimulatedGameState {
            turn: 1, width: 7, height: 7, food: Vec::new(), hazards: Vec::new(),
            snakes: vec![
                SimulatedSnake { id: "ours".into(), health: 100, body: vec![Coord {x:1,y:1}], alive: true },
                SimulatedSnake { id: "enemy".into(), health: 100, body: vec![Coord {x:5,y:5}], alive: true },
            ],
            our_snake_id: "ours".into(),
            rules: RulesContext { name: "standard".into(), max_health: 100, hazard_damage_per_turn: 0 },
        }
    }

    #[test]
    fn enumerates_every_legal_response_without_policy_filter() {
        let board = state();
        let mobility = MobilityAnalysis::from_state(&board);
        let ours = MoveMask::from_iter([Direction::Up, Direction::Right]);
        let generator = JointActionGenerator::new(&board, ours, &mobility);
        let all = generator.collect::<Vec<_>>();
        let our_actor = board.actor_index("ours").unwrap();
        let opponent = board.actor_index("enemy").unwrap();
        assert_eq!(all.len(), 8);
        assert!(all.iter().all(|joint|
            joint.direction_for(our_actor).is_some() && joint.direction_for(opponent).is_some()));
    }
}
