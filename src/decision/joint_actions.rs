#![allow(dead_code)]

use crate::direction::{Direction, MoveMask};
use crate::enemy::profile::OpponentProfiles;
use crate::enemy::tracing::EnemyTracingOutput;
use crate::simulation::joint_action::JointAction;
use crate::simulation::state::{ActorIndex, SimulatedGameState};

#[derive(Debug, Clone)]
pub(crate) struct JointActionGenerator {
    options: Vec<(ActorIndex, Vec<Direction>)>,
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
        Self::new_with_profiles(state, our_moves, tracing, &OpponentProfiles::default())
    }

    pub(crate) fn new_with_profiles(
        state: &SimulatedGameState,
        our_moves: MoveMask,
        tracing: &EnemyTracingOutput,
        profiles: &OpponentProfiles,
    ) -> Self {
        Self::build(state, our_moves, tracing, profiles)
    }

    pub(crate) fn new_actor_relative_with_profiles(
        state: &SimulatedGameState,
        our_moves: MoveMask,
        tracing: &EnemyTracingOutput,
        profiles: &OpponentProfiles,
    ) -> Self {
        Self::build(state, our_moves, tracing, profiles)
    }

    fn build(
        state: &SimulatedGameState,
        our_moves: MoveMask,
        tracing: &EnemyTracingOutput,
        profiles: &OpponentProfiles,
    ) -> Self {
        let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
            return Self {
                options: vec![],
                indices: vec![],
                done: true,
                estimated_count: 0,
            };
        };

        let Some(our_index) = state.actor_index(&ours.id) else {
            return Self {
                options: vec![],
                indices: vec![],
                done: true,
                estimated_count: 0,
            };
        };
        let mut options = vec![(
            our_index,
            normalized_moves(our_moves).iter().collect::<Vec<_>>(),
        )];

        let mut enemies = state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive && snake.id != state.our_snake_id)
            .filter_map(|(index, snake)| ActorIndex::new(index).map(|actor| (actor, snake)))
            .collect::<Vec<_>>();
        enemies.sort_by(|(_, left), (_, right)| left.id.cmp(&right.id));

        for (actor, enemy) in enemies {
            let moves = tracing
                .for_actor(actor)
                .map(|set| set.ordered_legal_moves_with_profile(profiles.get(&enemy.id)))
                .unwrap_or_else(|| normalized_moves(MoveMask::all()).iter().collect());

            options.push((actor, moves));
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
        for (option_index, (actor, directions)) in self.options.iter().enumerate() {
            action = action.with_move(*actor, directions[self.indices[option_index]]);
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
    use super::*;
    use crate::enemy::tracing::{trace_actor_relative_with_mobility, EnemyMoveSet};
    use crate::evaluation::ActorVec;
    use crate::simulation::mobility::MobilityAnalysis;
    use crate::simulation::resolver::resolve_turn;
    use crate::simulation::state::{RulesContext, SimulatedSnake};
    use crate::Coord;

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
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
        }
    }

    fn actor(state: &SimulatedGameState, actor_id: &str) -> ActorIndex {
        state.actor_index(actor_id).expect("actor must exist")
    }

    fn set(moves: MoveMask) -> EnemyMoveSet {
        EnemyMoveSet {
            legal_moves: moves,
            hypotheses: vec![],
        }
    }

    fn tracing(
        state: &SimulatedGameState,
        entries: Vec<(&str, EnemyMoveSet)>,
    ) -> EnemyTracingOutput {
        EnemyTracingOutput {
            enemies: entries
                .into_iter()
                .map(|(actor_id, move_set)| (actor(state, actor_id), move_set))
                .collect::<ActorVec<_>>(),
        }
    }

    #[test]
    fn produces_cartesian_product_for_one_enemy() {
        let state = state(vec![snake("ours", &[(1, 1)]), snake("enemy", &[(5, 5)])]);
        let tracing = tracing(
            &state,
            vec![(
                "enemy",
                set(MoveMask::from_iter([Direction::Left, Direction::Down])),
            )],
        );

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
        let tracing = tracing(
            &state,
            vec![
                (
                    "enemy-a",
                    set(MoveMask::from_iter([Direction::Left, Direction::Down])),
                ),
                (
                    "enemy-b",
                    set(MoveMask::from_iter([
                        Direction::Up,
                        Direction::Right,
                        Direction::Down,
                    ])),
                ),
            ],
        );

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
        let tracing = tracing(
            &state,
            vec![
                ("enemy-a", set(MoveMask::single(Direction::Left))),
                ("enemy-b", set(MoveMask::single(Direction::Down))),
            ],
        );

        for action in JointActionGenerator::new(&state, MoveMask::single(Direction::Up), &tracing) {
            assert_eq!(action.len(), 3);
            assert!(action.direction_for(actor(&state, "ours")).is_some());
            assert!(action.direction_for(actor(&state, "enemy-a")).is_some());
            assert!(action.direction_for(actor(&state, "enemy-b")).is_some());
        }
    }

    #[test]
    fn every_generator_path_keeps_all_legal_enemy_moves() {
        let state = state(vec![snake("ours", &[(1, 1)]), snake("enemy", &[(5, 5)])]);
        let tracing = tracing(
            &state,
            vec![(
                "enemy",
                EnemyMoveSet {
                    legal_moves: MoveMask::from_iter([
                        Direction::Left,
                        Direction::Down,
                        Direction::Right,
                    ]),
                    hypotheses: vec![],
                },
            )],
        );

        let default_generator =
            JointActionGenerator::new(&state, MoveMask::single(Direction::Up), &tracing);
        let actor_relative = JointActionGenerator::new_actor_relative_with_profiles(
            &state,
            MoveMask::single(Direction::Up),
            &tracing,
            &OpponentProfiles::default(),
        );

        assert_eq!(default_generator.estimated_count(), 3);
        assert_eq!(actor_relative.estimated_count(), 3);
    }

    #[test]
    fn empty_enemy_move_set_falls_back_to_all_directions() {
        let state = state(vec![snake("ours", &[(1, 1)]), snake("enemy", &[(5, 5)])]);
        let tracing = tracing(
            &state,
            vec![(
                "enemy",
                EnemyMoveSet {
                    legal_moves: MoveMask::empty(),
                    hypotheses: vec![],
                },
            )],
        );

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
        let mobility = MobilityAnalysis::from_state(&state);
        let tracing = trace_actor_relative_with_mobility(&state, &mobility);
        let our_moves = mobility.deterministic_moves_for(&state, "ours");

        let generator = JointActionGenerator::new(&state, our_moves, &tracing);
        assert!(generator.estimated_count() > 0);

        for action in generator {
            assert_eq!(action.len(), 3);
            resolve_turn(&state, &action).expect("generated joint action must be resolvable");
        }
    }

    #[test]
    fn learned_profile_reorders_equal_threat_enemy_moves() {
        use crate::enemy::profile::OpponentProfile;
        use crate::enemy::tracing::{OpponentMoveHypothesis, OpponentPolicySupport, ThreatClass};

        let state = state(vec![snake("ours", &[(1, 1)]), snake("enemy", &[(5, 5)])]);
        let move_set = EnemyMoveSet {
            legal_moves: MoveMask::from_iter([Direction::Left, Direction::Down]),
            hypotheses: vec![
                OpponentMoveHypothesis {
                    direction: Direction::Left,
                    support: OpponentPolicySupport {
                        food: true,
                        ..OpponentPolicySupport::default()
                    },
                    threat: ThreatClass::None,
                    plausibility_milli: 600,
                },
                OpponentMoveHypothesis {
                    direction: Direction::Down,
                    support: OpponentPolicySupport {
                        hunting: true,
                        ..OpponentPolicySupport::default()
                    },
                    threat: ThreatClass::None,
                    plausibility_milli: 600,
                },
            ],
        };
        let mut profile = OpponentProfile::default();
        for _ in 0..4 {
            profile.observe_with_intent(&move_set, Direction::Down, None);
        }
        let tracing = tracing(&state, vec![("enemy", move_set)]);
        let profiles = OpponentProfiles::from([("enemy".to_string(), profile)]);

        let first = JointActionGenerator::new_with_profiles(
            &state,
            MoveMask::single(Direction::Up),
            &tracing,
            &profiles,
        )
        .next()
        .unwrap();

        assert_eq!(
            first.direction_for(actor(&state, "enemy")),
            Some(Direction::Down)
        );
    }

    #[test]
    fn enemy_hypotheses_order_dangerous_supported_moves_first() {
        use crate::enemy::tracing::{OpponentMoveHypothesis, OpponentPolicySupport, ThreatClass};

        let state = state(vec![snake("ours", &[(1, 1)]), snake("enemy", &[(5, 5)])]);
        let tracing = tracing(
            &state,
            vec![(
                "enemy",
                EnemyMoveSet {
                    legal_moves: MoveMask::from_iter([Direction::Left, Direction::Down]),
                    hypotheses: vec![
                        OpponentMoveHypothesis {
                            direction: Direction::Left,
                            support: OpponentPolicySupport {
                                food: true,
                                ..OpponentPolicySupport::default()
                            },
                            threat: ThreatClass::None,
                            plausibility_milli: 300,
                        },
                        OpponentMoveHypothesis {
                            direction: Direction::Down,
                            support: OpponentPolicySupport {
                                hunting: true,
                                head_threat: true,
                                ..OpponentPolicySupport::default()
                            },
                            threat: ThreatClass::Likely,
                            plausibility_milli: 800,
                        },
                    ],
                },
            )],
        );

        let first = JointActionGenerator::new(&state, MoveMask::single(Direction::Up), &tracing)
            .next()
            .unwrap();

        assert_eq!(
            first.direction_for(actor(&state, "enemy")),
            Some(Direction::Down)
        );
    }

    #[test]
    fn enumeration_order_is_deterministic() {
        let state = state(vec![
            snake("ours", &[(1, 1)]),
            snake("enemy-b", &[(5, 5)]),
            snake("enemy-a", &[(5, 1)]),
        ]);
        let tracing = tracing(
            &state,
            vec![
                (
                    "enemy-b",
                    set(MoveMask::from_iter([Direction::Right, Direction::Down])),
                ),
                (
                    "enemy-a",
                    set(MoveMask::from_iter([Direction::Up, Direction::Left])),
                ),
            ],
        );

        let actions = JointActionGenerator::new(
            &state,
            MoveMask::from_iter([Direction::Up, Direction::Right]),
            &tracing,
        )
        .collect::<Vec<_>>();

        assert_eq!(
            actions[0].direction_for(actor(&state, "ours")),
            Some(Direction::Up)
        );
        assert_eq!(
            actions[0].direction_for(actor(&state, "enemy-a")),
            Some(Direction::Up)
        );
        assert_eq!(
            actions[0].direction_for(actor(&state, "enemy-b")),
            Some(Direction::Right)
        );

        assert_eq!(
            actions[1].direction_for(actor(&state, "ours")),
            Some(Direction::Up)
        );
        assert_eq!(
            actions[1].direction_for(actor(&state, "enemy-a")),
            Some(Direction::Up)
        );
        assert_eq!(
            actions[1].direction_for(actor(&state, "enemy-b")),
            Some(Direction::Down)
        );
    }
}
