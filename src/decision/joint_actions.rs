#![allow(dead_code)]

use crate::direction::{Direction, MoveMask};
use crate::enemy::tracing::EnemyTracingOutput;
use crate::simulation::joint_action::JointAction;
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone)]
pub(crate) struct JointActionGenerator {
    options: Vec<(String, Vec<Direction>)>,
    indices: Vec<usize>,
    done: bool,
    estimated_count: usize,
}

impl JointActionGenerator {
    pub(crate) fn new(
        state: &SimulatedGameState,
        our_moves: MoveMask,
        tracing: &EnemyTracingOutput,
    ) -> Self {
        let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
            return Self {
                options: vec![],
                indices: vec![],
                done: true,
                estimated_count: 0,
            };
        };

        let mut options = vec![(
            ours.id.clone(),
            normalized_moves(our_moves).iter().collect::<Vec<_>>(),
        )];

        let mut enemies = state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != state.our_snake_id)
            .collect::<Vec<_>>();
        enemies.sort_by(|left, right| left.id.cmp(&right.id));

        for enemy in enemies {
            let moves = tracing
                .for_enemy(&enemy.id)
                .map(|set| set.search_moves())
                .unwrap_or_else(MoveMask::all);

            options.push((enemy.id.clone(), normalized_moves(moves).iter().collect()));
        }

        let estimated_count = options.iter().fold(1_usize, |count, (_, moves)| {
            count.saturating_mul(moves.len())
        });

        Self {
            indices: vec![0; options.len()],
            options,
            done: false,
            estimated_count,
        }
    }

    pub(crate) fn estimated_count(&self) -> usize {
        self.estimated_count
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
        for (option_index, (snake_id, directions)) in self.options.iter().enumerate() {
            action = action.with_move(snake_id.clone(), directions[self.indices[option_index]]);
        }

        self.advance();
        Some(action)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.done {
            return (0, Some(0));
        }

        (0, Some(self.estimated_count))
    }
}

fn normalized_moves(moves: MoveMask) -> MoveMask {
    if moves.is_empty() {
        MoveMask::all()
    } else {
        moves
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::analysis::StateAnalysis;
    use crate::enemy::tracing::{trace, EnemyMoveSet};
    use crate::forecast::ForecastCertainty;
    use crate::simulation::mobility::MobilityAnalysis;
    use crate::simulation::resolver::resolve_turn;
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedSnake,
    };
    use crate::Coord;

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: body
                .iter()
                .map(|(x, y)| Coord { x: *x, y: *y })
                .collect(),
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

    fn set(moves: MoveMask) -> EnemyMoveSet {
        EnemyMoveSet {
            legal_moves: moves,
            plausible_moves: moves,
            eliminations: vec![],
        }
    }

    #[test]
    fn produces_cartesian_product_for_one_enemy() {
        let state = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy", &[(5, 5)]),
        ]);
        let tracing = EnemyTracingOutput {
            enemies: HashMap::from([(
                "enemy".to_string(),
                set(MoveMask::from_iter([Direction::Left, Direction::Down])),
            )]),
        };

        let generator = JointActionGenerator::new(
            &state,
            MoveMask::from_iter([Direction::Up, Direction::Right]),
            &tracing,
        );

        assert_eq!(generator.estimated_count(), 4);
        assert_eq!(generator.collect::<Vec<_>>().len(), 4);
    }

    #[test]
    fn produces_cartesian_product_for_multiple_enemies() {
        let state = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy-b", &[(5, 5)]),
            snake("enemy-a", &[(5, 1)]),
        ]);
        let tracing = EnemyTracingOutput {
            enemies: HashMap::from([
                (
                    "enemy-a".to_string(),
                    set(MoveMask::from_iter([Direction::Left, Direction::Down])),
                ),
                (
                    "enemy-b".to_string(),
                    set(MoveMask::from_iter([
                        Direction::Up,
                        Direction::Right,
                        Direction::Down,
                    ])),
                ),
            ]),
        };

        let generator = JointActionGenerator::new(
            &state,
            MoveMask::from_iter([Direction::Up, Direction::Right]),
            &tracing,
        );

        assert_eq!(generator.estimated_count(), 12);
        assert_eq!(generator.collect::<Vec<_>>().len(), 12);
    }

    #[test]
    fn every_action_contains_a_move_for_every_living_snake() {
        let state = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy-a", &[(5, 1)]),
            snake("enemy-b", &[(5, 5)]),
        ]);
        let tracing = EnemyTracingOutput {
            enemies: HashMap::from([
                ("enemy-a".to_string(), set(MoveMask::single(Direction::Left))),
                ("enemy-b".to_string(), set(MoveMask::single(Direction::Down))),
            ]),
        };

        for action in JointActionGenerator::new(
            &state,
            MoveMask::single(Direction::Up),
            &tracing,
        ) {
            assert_eq!(action.len(), 3);
            assert!(action.direction_for("ours").is_some());
            assert!(action.direction_for("enemy-a").is_some());
            assert!(action.direction_for("enemy-b").is_some());
        }
    }

    #[test]
    fn empty_enemy_move_set_falls_back_to_all_directions() {
        let state = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy", &[(5, 5)]),
        ]);
        let tracing = EnemyTracingOutput {
            enemies: HashMap::from([(
                "enemy".to_string(),
                EnemyMoveSet {
                    legal_moves: MoveMask::empty(),
                    plausible_moves: MoveMask::empty(),
                    eliminations: vec![],
                },
            )]),
        };

        let generator =
            JointActionGenerator::new(&state, MoveMask::single(Direction::Up), &tracing);

        assert_eq!(generator.estimated_count(), 4);
    }

    #[test]
    fn tracing_generator_and_resolver_form_a_complete_pipeline() {
        let state = state(vec![
            snake("ours", &[(1, 1), (1, 0)]),
            snake("enemy-a", &[(5, 1), (5, 0)]),
            snake("enemy-b", &[(5, 5), (5, 4)]),
        ]);
        let analysis =
            StateAnalysis::from_simulated(&state, ForecastCertainty::Deterministic);
        let tracing = trace(&state, &analysis);
        let mobility = MobilityAnalysis::from_state(&state);
        let our_moves = mobility.deterministic_moves_for(&state, "ours");

        let generator = JointActionGenerator::new(&state, our_moves, &tracing);
        assert!(generator.estimated_count() > 0);

        for action in generator {
            assert_eq!(action.len(), 3);
            resolve_turn(&state, &action)
                .expect("generated joint action must be resolvable");
        }
    }

    #[test]
    fn enumeration_order_is_deterministic() {
        let state = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy-b", &[(5, 5)]),
            snake("enemy-a", &[(5, 1)]),
        ]);
        let tracing = EnemyTracingOutput {
            enemies: HashMap::from([
                (
                    "enemy-b".to_string(),
                    set(MoveMask::from_iter([Direction::Right, Direction::Down])),
                ),
                (
                    "enemy-a".to_string(),
                    set(MoveMask::from_iter([Direction::Up, Direction::Left])),
                ),
            ]),
        };

        let actions = JointActionGenerator::new(
            &state,
            MoveMask::from_iter([Direction::Up, Direction::Right]),
            &tracing,
        )
        .collect::<Vec<_>>();

        assert_eq!(actions[0].direction_for("ours"), Some(Direction::Up));
        assert_eq!(actions[0].direction_for("enemy-a"), Some(Direction::Up));
        assert_eq!(actions[0].direction_for("enemy-b"), Some(Direction::Right));

        assert_eq!(actions[1].direction_for("ours"), Some(Direction::Up));
        assert_eq!(actions[1].direction_for("enemy-a"), Some(Direction::Up));
        assert_eq!(actions[1].direction_for("enemy-b"), Some(Direction::Down));
    }
}
