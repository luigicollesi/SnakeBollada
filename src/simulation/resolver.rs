use crate::Coord;

use super::state::{ActorIndex, SimulatedGameState, SimulatedSnake};

use super::joint_action::JointAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EliminationCause {
    OutOfHealth,
    Hazard,
    OutOfBounds,
    SelfCollision,
    BodyCollision,
    HeadToHead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EliminationAttribution {
    SelfInflicted,
    Actor(ActorIndex),
    Environment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstantEvent {
    AteFood {
        actor: ActorIndex,
        food: Coord,
    },
    EnemyKilled {
        enemy: ActorIndex,
        cause: EliminationCause,
        attribution: EliminationAttribution,
    },
    Died {
        cause: EliminationCause,
        attribution: EliminationAttribution,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolveError {
    MissingMove(String),
    EmptyBody(String),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TurnResolution {
    pub(crate) state: SimulatedGameState,
    pub(crate) events: Vec<InstantEvent>,
}

#[derive(Debug, Clone)]
struct PendingElimination {
    cause: EliminationCause,
    by: Option<ActorIndex>,
}

pub(crate) fn resolve_turn(
    state: &SimulatedGameState,
    joint_action: &JointAction,
) -> Result<TurnResolution, ResolveError> {
    let mut next = state.clone();
    let mut events = Vec::new();

    move_snakes(&mut next, joint_action)?;
    reduce_health(&mut next);
    damage_hazards(&mut next, &mut events);

    feed_snakes(&mut next, &mut events)?;
    eliminate_snakes(&mut next, &mut events)?;

    next.turn = next.turn.saturating_add(1);

    Ok(TurnResolution {
        state: next,
        events,
    })
}

fn move_snakes(
    state: &mut SimulatedGameState,
    joint_action: &JointAction,
) -> Result<(), ResolveError> {
    for (index, snake) in state
        .snakes
        .iter_mut()
        .enumerate()
        .filter(|(_, snake)| snake.alive)
    {
        let actor =
            ActorIndex::new(index).ok_or_else(|| ResolveError::MissingMove(snake.id.clone()))?;
        let direction = joint_action
            .direction_for(actor)
            .ok_or_else(|| ResolveError::MissingMove(snake.id.clone()))?;
        let head = snake
            .head()
            .ok_or_else(|| ResolveError::EmptyBody(snake.id.clone()))?;
        let new_head = direction.apply(head);

        let mut moved = Vec::with_capacity(snake.body.len());
        moved.push(new_head);
        if snake.body.len() > 1 {
            moved.extend_from_slice(&snake.body[..snake.body.len() - 1]);
        }
        snake.body = moved;
    }

    Ok(())
}

fn reduce_health(state: &mut SimulatedGameState) {
    for snake in state.snakes.iter_mut().filter(|snake| snake.alive) {
        snake.health = snake.health.saturating_sub(1);
    }
}

fn damage_hazards(state: &mut SimulatedGameState, events: &mut Vec<InstantEvent>) {
    let food = &state.food;
    let hazards = &state.hazards;
    let hazard_damage = state.rules.hazard_damage_per_turn;
    let our_actor = state.actor_index(&state.our_snake_id);

    if hazard_damage <= 0 {
        return;
    }

    for (index, snake) in state
        .snakes
        .iter_mut()
        .enumerate()
        .filter(|(_, snake)| snake.alive)
    {
        let Some(actor) = ActorIndex::new(index) else {
            continue;
        };
        let Some(head) = snake.head() else {
            continue;
        };

        if food.contains(&head) {
            continue;
        }

        let stack_count = hazards.iter().filter(|hazard| **hazard == head).count();
        if stack_count == 0 {
            continue;
        }

        let total_damage = hazard_damage.saturating_mul(stack_count as i32);
        snake.health = snake.health.saturating_sub(total_damage).max(0);

        if snake.health <= 0 {
            snake.alive = false;
            if Some(actor) == our_actor {
                events.push(InstantEvent::Died {
                    cause: EliminationCause::Hazard,
                    attribution: EliminationAttribution::Environment,
                });
            } else {
                events.push(InstantEvent::EnemyKilled {
                    enemy: actor,
                    cause: EliminationCause::Hazard,
                    attribution: EliminationAttribution::Environment,
                });
            }
        }
    }
}

fn feed_snakes(
    state: &mut SimulatedGameState,
    events: &mut Vec<InstantEvent>,
) -> Result<(), ResolveError> {
    let mut remaining_food = Vec::with_capacity(state.food.len());

    for food in state.food.iter().copied() {
        let mut eaten = false;

        for (index, snake) in state
            .snakes
            .iter_mut()
            .enumerate()
            .filter(|(_, snake)| snake.alive)
        {
            let actor = ActorIndex::new(index)
                .ok_or_else(|| ResolveError::MissingMove(snake.id.clone()))?;
            let head = snake
                .head()
                .ok_or_else(|| ResolveError::EmptyBody(snake.id.clone()))?;

            if head != food {
                continue;
            }

            let tail = snake
                .body
                .last()
                .copied()
                .ok_or_else(|| ResolveError::EmptyBody(snake.id.clone()))?;
            snake.body.push(tail);
            snake.health = state.rules.max_health;
            events.push(InstantEvent::AteFood { actor, food });
            eaten = true;
        }

        if !eaten {
            remaining_food.push(food);
        }
    }

    state.food = remaining_food;
    Ok(())
}

fn eliminate_snakes(
    state: &mut SimulatedGameState,
    events: &mut Vec<InstantEvent>,
) -> Result<(), ResolveError> {
    let mut pending = vec![None; state.snakes.len()];

    for (index, snake) in state
        .snakes
        .iter()
        .enumerate()
        .filter(|(_, snake)| snake.alive)
    {
        let actor =
            ActorIndex::new(index).ok_or_else(|| ResolveError::EmptyBody(snake.id.clone()))?;
        if snake.body.is_empty() {
            return Err(ResolveError::EmptyBody(snake.id.clone()));
        }

        if snake.health <= 0 {
            pending[actor.as_usize()] = Some(PendingElimination {
                cause: EliminationCause::OutOfHealth,
                by: None,
            });
            continue;
        }

        if is_out_of_bounds(snake, state.width, state.height) {
            pending[actor.as_usize()] = Some(PendingElimination {
                cause: EliminationCause::OutOfBounds,
                by: None,
            });
        }
    }

    let collision_candidates = state
        .snakes
        .iter()
        .enumerate()
        .filter_map(|(index, snake)| {
            let actor = ActorIndex::new(index)?;
            (snake.alive && pending[actor.as_usize()].is_none()).then_some((actor, snake))
        })
        .collect::<Vec<_>>();

    let mut opponents_by_length = collision_candidates.clone();
    opponents_by_length.sort_by(|(_, left), (_, right)| {
        right
            .length()
            .cmp(&left.length())
            .then_with(|| left.id.cmp(&right.id))
    });

    for (actor, snake) in &collision_candidates {
        if pending[actor.as_usize()].is_some() {
            continue;
        }

        if body_collision(snake, snake) {
            pending[actor.as_usize()] = Some(PendingElimination {
                cause: EliminationCause::SelfCollision,
                by: Some(*actor),
            });
            continue;
        }

        if let Some((other_actor, _)) = opponents_by_length
            .iter()
            .copied()
            .find(|(_, other)| other.id != snake.id && body_collision(snake, other))
        {
            pending[actor.as_usize()] = Some(PendingElimination {
                cause: EliminationCause::BodyCollision,
                by: Some(other_actor),
            });
            continue;
        }

        if let Some((other_actor, _)) = opponents_by_length
            .iter()
            .copied()
            .find(|(_, other)| other.id != snake.id && lost_head_to_head(snake, other))
        {
            pending[actor.as_usize()] = Some(PendingElimination {
                cause: EliminationCause::HeadToHead,
                by: Some(other_actor),
            });
        }
    }

    let our_actor = state.actor_index(&state.our_snake_id);

    for (index, snake) in state.snakes.iter_mut().enumerate() {
        let Some(actor) = ActorIndex::new(index) else {
            continue;
        };
        let Some(elimination) = pending[actor.as_usize()].as_ref() else {
            continue;
        };

        snake.alive = false;

        let attribution = match elimination.cause {
            EliminationCause::SelfCollision | EliminationCause::OutOfBounds => {
                EliminationAttribution::SelfInflicted
            }
            EliminationCause::OutOfHealth | EliminationCause::Hazard => {
                EliminationAttribution::Environment
            }
            EliminationCause::BodyCollision | EliminationCause::HeadToHead => {
                match elimination.by {
                    Some(by) if by == actor => EliminationAttribution::SelfInflicted,
                    Some(by) => EliminationAttribution::Actor(by),
                    None => EliminationAttribution::Environment,
                }
            }
        };

        if Some(actor) == our_actor {
            events.push(InstantEvent::Died {
                cause: elimination.cause,
                attribution,
            });
        } else {
            events.push(InstantEvent::EnemyKilled {
                enemy: actor,
                cause: elimination.cause,
                attribution,
            });
        }
    }

    Ok(())
}

fn is_out_of_bounds(snake: &SimulatedSnake, width: u32, height: u32) -> bool {
    snake.body.iter().any(|point| {
        point.x < 0 || point.y < 0 || point.x >= width as i32 || point.y >= height as i32
    })
}

fn body_collision(snake: &SimulatedSnake, other: &SimulatedSnake) -> bool {
    let Some(head) = snake.head() else {
        return false;
    };

    other.body.iter().skip(1).any(|segment| *segment == head)
}

fn lost_head_to_head(snake: &SimulatedSnake, other: &SimulatedSnake) -> bool {
    snake.head().is_some() && snake.head() == other.head() && snake.length() <= other.length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direction::Direction;
    use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};

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
            turn: 4,
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

    fn action(state: &SimulatedGameState, moves: &[(&str, Direction)]) -> JointAction {
        moves
            .iter()
            .fold(JointAction::new(), |action, (actor_id, direction)| {
                let actor = state.actor_index(actor_id).expect("actor must exist");
                action.with_move(actor, *direction)
            })
    }

    fn actor(state: &SimulatedGameState, actor_id: &str) -> ActorIndex {
        state.actor_index(actor_id).expect("actor must exist")
    }

    #[test]
    fn movement_prepends_head_and_vacates_tail() {
        let initial = state(vec![snake("ours", 100, &[(2, 2), (2, 1), (2, 0)])]);
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();
        let ours = resolved.state.snake("ours").unwrap();

        assert_eq!(
            ours.body,
            vec![
                Coord { x: 3, y: 2 },
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 }
            ]
        );
        assert_eq!(ours.health, 99);
        assert_eq!(resolved.state.turn, 5);
    }

    #[test]
    fn food_restores_health_and_grows_tail() {
        let mut initial = state(vec![snake("ours", 25, &[(2, 2), (2, 1), (2, 0)])]);
        initial.food = vec![Coord { x: 3, y: 2 }];
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();
        let ours = resolved.state.snake("ours").unwrap();

        assert_eq!(ours.health, 100);
        assert_eq!(ours.length(), 4);
        assert_eq!(resolved.state.food, Vec::<Coord>::new());
        assert!(resolved.events.contains(&InstantEvent::AteFood {
            actor: initial.actor_index("ours").unwrap(),
            food: Coord { x: 3, y: 2 },
        }));
    }

    #[test]
    fn our_food_is_recorded_without_strategy_side_state() {
        let mut initial = state(vec![snake("ours", 50, &[(2, 2), (2, 1)])]);
        initial.food = vec![Coord { x: 3, y: 2 }];
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();
        assert!(
            resolved.state.snake("ours").unwrap().length()
                > initial.snake("ours").unwrap().length()
        );
    }

    #[test]
    fn enemy_food_does_not_change_our_snake_state() {
        let mut initial = state(vec![
            snake("ours", 50, &[(2, 2), (2, 1)]),
            snake("enemy", 50, &[(4, 2), (4, 1)]),
        ]);
        initial.food = vec![Coord { x: 5, y: 2 }];
        let action = action(
            &initial,
            &[("ours", Direction::Up), ("enemy", Direction::Right)],
        );

        let our_before = initial.snake("ours").unwrap().body.clone();
        let resolved = resolve_turn(&initial, &action).unwrap();
        assert_eq!(
            resolved.state.snake("ours").unwrap().body.len(),
            our_before.len()
        );
    }

    #[test]
    fn snake_at_one_health_survives_by_eating() {
        let mut initial = state(vec![snake("ours", 1, &[(2, 2), (2, 1)])]);
        initial.food = vec![Coord { x: 3, y: 2 }];
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(resolved.state.snake("ours").unwrap().alive);
        assert_eq!(resolved.state.snake("ours").unwrap().health, 100);
    }

    #[test]
    fn food_on_hazard_prevents_hazard_damage_for_that_turn() {
        let mut initial = state(vec![snake("ours", 50, &[(2, 2), (2, 1)])]);
        initial.rules.hazard_damage_per_turn = 14;
        initial.food = vec![Coord { x: 3, y: 2 }];
        initial.hazards = vec![Coord { x: 3, y: 2 }];
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert_eq!(resolved.state.snake("ours").unwrap().health, 100);
    }

    #[test]
    fn fatal_hazard_eliminates_before_feed_with_hazard_cause() {
        let mut initial = state(vec![snake("ours", 15, &[(2, 2), (2, 1)])]);
        initial.rules.hazard_damage_per_turn = 14;
        initial.hazards = vec![Coord { x: 3, y: 2 }];
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::Hazard,
            attribution: EliminationAttribution::Environment,
        }));
    }

    #[test]
    fn enemy_environment_death_is_not_attributed_to_us() {
        let mut initial = state(vec![
            snake("ours", 100, &[(1, 1), (1, 0)]),
            snake("enemy", 15, &[(3, 1), (3, 0)]),
        ]);
        initial.rules.hazard_damage_per_turn = 14;
        initial.hazards = vec![Coord { x: 4, y: 1 }];
        let action = action(
            &initial,
            &[("ours", Direction::Up), ("enemy", Direction::Right)],
        );

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(resolved.events.contains(&InstantEvent::EnemyKilled {
            enemy: actor(&initial, "enemy"),
            cause: EliminationCause::Hazard,
            attribution: EliminationAttribution::Environment,
        }));
    }

    #[test]
    fn stacked_hazards_apply_damage_per_stack() {
        let mut initial = state(vec![snake("ours", 50, &[(2, 2), (2, 1)])]);
        initial.rules.hazard_damage_per_turn = 14;
        initial.hazards = vec![Coord { x: 3, y: 2 }, Coord { x: 3, y: 2 }];
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert_eq!(resolved.state.snake("ours").unwrap().health, 21);
    }

    #[test]
    fn starvation_without_food_is_out_of_health() {
        let initial = state(vec![snake("ours", 1, &[(2, 2), (2, 1)])]);
        let action = action(&initial, &[("ours", Direction::Right)]);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::OutOfHealth,
            attribution: EliminationAttribution::Environment,
        }));
    }

    #[test]
    fn moving_out_of_bounds_is_eliminated() {
        let initial = state(vec![snake("ours", 100, &[(0, 2), (0, 1)])]);
        let action = action(&initial, &[("ours", Direction::Left)]);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::OutOfBounds,
            attribution: EliminationAttribution::SelfInflicted,
        }));
    }

    #[test]
    fn self_collision_is_eliminated() {
        let initial = state(vec![snake(
            "ours",
            100,
            &[(2, 2), (2, 1), (1, 1), (1, 2), (1, 3), (2, 3)],
        )]);
        let action = action(&initial, &[("ours", Direction::Left)]);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::SelfCollision,
            attribution: EliminationAttribution::SelfInflicted,
        }));
    }

    #[test]
    fn equal_length_head_to_head_eliminates_both() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 2), (2, 1), (2, 0)]),
            snake("enemy", 100, &[(4, 2), (4, 1), (4, 0)]),
        ]);
        let action = action(
            &initial,
            &[("ours", Direction::Right), ("enemy", Direction::Left)],
        );

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(!resolved.state.snake("enemy").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::HeadToHead,
            attribution: EliminationAttribution::Actor(actor(&initial, "enemy")),
        }));
    }

    #[test]
    fn longer_snake_wins_head_to_head() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 2), (2, 1), (2, 0), (1, 0)]),
            snake("enemy", 100, &[(4, 2), (4, 1), (4, 0)]),
        ]);
        let action = action(
            &initial,
            &[("ours", Direction::Right), ("enemy", Direction::Left)],
        );

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(resolved.state.snake("ours").unwrap().alive);
        assert!(!resolved.state.snake("enemy").unwrap().alive);
    }

    #[test]
    fn three_way_head_to_head_longest_survives_and_gets_attribution() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 3), (2, 2), (2, 1), (1, 1), (1, 0)]),
            snake("enemy-a", 100, &[(4, 3), (4, 2), (4, 1), (5, 1)]),
            snake("enemy-b", 100, &[(3, 4), (3, 5), (2, 5)]),
        ]);
        let action = action(
            &initial,
            &[
                ("ours", Direction::Right),
                ("enemy-a", Direction::Left),
                ("enemy-b", Direction::Down),
            ],
        );

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(resolved.state.snake("ours").unwrap().alive);
        assert!(!resolved.state.snake("enemy-a").unwrap().alive);
        assert!(!resolved.state.snake("enemy-b").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::EnemyKilled {
            enemy: actor(&initial, "enemy-a"),
            cause: EliminationCause::HeadToHead,
            attribution: EliminationAttribution::Actor(actor(&initial, "ours")),
        }));
        assert!(resolved.events.contains(&InstantEvent::EnemyKilled {
            enemy: actor(&initial, "enemy-b"),
            cause: EliminationCause::HeadToHead,
            attribution: EliminationAttribution::Actor(actor(&initial, "ours")),
        }));
    }

    #[test]
    fn three_way_equal_longest_head_to_head_eliminates_all() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 3), (2, 2), (2, 1), (1, 1)]),
            snake("enemy-a", 100, &[(4, 3), (4, 2), (4, 1), (5, 1)]),
            snake("enemy-b", 100, &[(3, 4), (3, 5), (2, 5)]),
        ]);
        let action = action(
            &initial,
            &[
                ("ours", Direction::Right),
                ("enemy-a", Direction::Left),
                ("enemy-b", Direction::Down),
            ],
        );

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(!resolved.state.snake("enemy-a").unwrap().alive);
        assert!(!resolved.state.snake("enemy-b").unwrap().alive);
    }

    #[test]
    fn collision_with_snake_that_also_dies_still_counts_this_turn() {
        let initial = state(vec![
            snake("ours", 100, &[(1, 2), (1, 1), (1, 0)]),
            snake("enemy", 100, &[(3, 2), (2, 2), (2, 1)]),
        ]);
        let action = action(
            &initial,
            &[("ours", Direction::Right), ("enemy", Direction::Left)],
        );

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(!resolved.state.snake("enemy").unwrap().alive);
    }

    #[test]
    fn body_collision_eliminates_moving_snake() {
        let initial = state(vec![
            snake("ours", 100, &[(1, 2), (1, 1), (1, 0)]),
            snake(
                "enemy",
                100,
                &[(4, 4), (3, 4), (2, 4), (2, 3), (2, 2), (2, 1)],
            ),
        ]);
        let action = action(
            &initial,
            &[("ours", Direction::Right), ("enemy", Direction::Right)],
        );

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(resolved.state.snake("enemy").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::BodyCollision,
            attribution: EliminationAttribution::Actor(actor(&initial, "enemy")),
        }));
    }

    #[test]
    fn missing_move_is_rejected() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 2)]),
            snake("enemy", 100, &[(5, 5)]),
        ]);
        let action = action(&initial, &[("ours", Direction::Up)]);

        assert_eq!(
            resolve_turn(&initial, &action),
            Err(ResolveError::MissingMove("enemy".to_string()))
        );
    }
}
