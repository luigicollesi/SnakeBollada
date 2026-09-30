use std::collections::{BTreeMap, HashMap};

use crate::direction::Direction;
use crate::Coord;

use super::state::{SimulatedGameState, SimulatedSnake};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JointAction {
    moves: BTreeMap<String, Direction>,
}

impl JointAction {
    pub(crate) fn new() -> Self {
        Self {
            moves: BTreeMap::new(),
        }
    }

    pub(crate) fn with_move(mut self, snake_id: impl Into<String>, direction: Direction) -> Self {
        self.moves.insert(snake_id.into(), direction);
        self
    }

    pub(crate) fn direction_for(&self, snake_id: &str) -> Option<Direction> {
        self.moves.get(snake_id).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EliminationCause {
    OutOfHealth,
    OutOfBounds,
    SelfCollision,
    BodyCollision,
    HeadToHead,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstantEvent {
    AteFood {
        snake: String,
        food: Coord,
    },
    EnemyKilled {
        enemy: String,
        cause: EliminationCause,
    },
    HeadToHeadWon {
        enemy: String,
    },
    HeadToHeadLost {
        enemy: String,
    },
    Died {
        cause: EliminationCause,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForecastDelta {
    None,
    FoodUncertainty,
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
    pub(crate) forecast_delta: ForecastDelta,
}

#[derive(Debug, Clone)]
struct PendingElimination {
    cause: EliminationCause,
    by: Option<String>,
}

pub(crate) fn resolve_turn(
    state: &SimulatedGameState,
    joint_action: &JointAction,
) -> Result<TurnResolution, ResolveError> {
    let mut next = state.clone();
    let mut events = Vec::new();

    move_snakes(&mut next, joint_action)?;
    reduce_health(&mut next);
    damage_hazards(&mut next);

    let ate_food = feed_snakes(&mut next, &mut events)?;
    eliminate_snakes(&mut next, &mut events)?;

    next.turn = next.turn.saturating_add(1);

    Ok(TurnResolution {
        state: next,
        events,
        forecast_delta: if ate_food {
            ForecastDelta::FoodUncertainty
        } else {
            ForecastDelta::None
        },
    })
}

fn move_snakes(
    state: &mut SimulatedGameState,
    joint_action: &JointAction,
) -> Result<(), ResolveError> {
    for snake in state.snakes.iter_mut().filter(|snake| snake.alive) {
        let direction = joint_action
            .direction_for(&snake.id)
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

fn damage_hazards(state: &mut SimulatedGameState) {
    let food = &state.food;
    let hazards = &state.hazards;
    let hazard_damage = state.rules.hazard_damage_per_turn;

    if hazard_damage <= 0 {
        return;
    }

    for snake in state.snakes.iter_mut().filter(|snake| snake.alive) {
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
    }
}

fn feed_snakes(
    state: &mut SimulatedGameState,
    events: &mut Vec<InstantEvent>,
) -> Result<bool, ResolveError> {
    let mut remaining_food = Vec::with_capacity(state.food.len());
    let mut any_eaten = false;

    for food in state.food.iter().copied() {
        let mut eaten = false;

        for snake in state.snakes.iter_mut().filter(|snake| snake.alive) {
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
            events.push(InstantEvent::AteFood {
                snake: snake.id.clone(),
                food,
            });
            eaten = true;
            any_eaten = true;
        }

        if !eaten {
            remaining_food.push(food);
        }
    }

    state.food = remaining_food;
    Ok(any_eaten)
}

fn eliminate_snakes(
    state: &mut SimulatedGameState,
    events: &mut Vec<InstantEvent>,
) -> Result<(), ResolveError> {
    let mut pending: HashMap<String, PendingElimination> = HashMap::new();

    for snake in state.snakes.iter().filter(|snake| snake.alive) {
        if snake.body.is_empty() {
            return Err(ResolveError::EmptyBody(snake.id.clone()));
        }

        if snake.health <= 0 {
            pending.insert(
                snake.id.clone(),
                PendingElimination {
                    cause: EliminationCause::OutOfHealth,
                    by: None,
                },
            );
            continue;
        }

        if is_out_of_bounds(snake, state.width, state.height) {
            pending.insert(
                snake.id.clone(),
                PendingElimination {
                    cause: EliminationCause::OutOfBounds,
                    by: None,
                },
            );
        }
    }

    let collision_candidates = state
        .snakes
        .iter()
        .filter(|snake| snake.alive && !pending.contains_key(&snake.id))
        .collect::<Vec<_>>();

    let mut opponents_by_length = collision_candidates.clone();
    opponents_by_length.sort_by(|left, right| {
        right
            .length()
            .cmp(&left.length())
            .then_with(|| left.id.cmp(&right.id))
    });

    for snake in &collision_candidates {
        if pending.contains_key(&snake.id) {
            continue;
        }

        if body_collision(snake, snake) {
            pending.insert(
                snake.id.clone(),
                PendingElimination {
                    cause: EliminationCause::SelfCollision,
                    by: Some(snake.id.clone()),
                },
            );
            continue;
        }

        if let Some(other) = opponents_by_length
            .iter()
            .copied()
            .find(|other| other.id != snake.id && body_collision(snake, other))
        {
            pending.insert(
                snake.id.clone(),
                PendingElimination {
                    cause: EliminationCause::BodyCollision,
                    by: Some(other.id.clone()),
                },
            );
            continue;
        }

        if let Some(other) = opponents_by_length
            .iter()
            .copied()
            .find(|other| other.id != snake.id && lost_head_to_head(snake, other))
        {
            pending.insert(
                snake.id.clone(),
                PendingElimination {
                    cause: EliminationCause::HeadToHead,
                    by: Some(other.id.clone()),
                },
            );
        }
    }

    let our_id = state.our_snake_id.clone();

    for snake in &mut state.snakes {
        let Some(elimination) = pending.get(&snake.id) else {
            continue;
        };

        snake.alive = false;

        if snake.id == our_id {
            if elimination.cause == EliminationCause::HeadToHead {
                if let Some(enemy) = &elimination.by {
                    events.push(InstantEvent::HeadToHeadLost {
                        enemy: enemy.clone(),
                    });
                }
            }
            events.push(InstantEvent::Died {
                cause: elimination.cause,
            });
        } else {
            if elimination.cause == EliminationCause::HeadToHead
                && elimination.by.as_deref() == Some(our_id.as_str())
            {
                events.push(InstantEvent::HeadToHeadWon {
                    enemy: snake.id.clone(),
                });
            }
            events.push(InstantEvent::EnemyKilled {
                enemy: snake.id.clone(),
                cause: elimination.cause,
            });
        }
    }

    Ok(())
}

fn is_out_of_bounds(snake: &SimulatedSnake, width: u32, height: u32) -> bool {
    snake.body.iter().any(|point| {
        point.x < 0
            || point.y < 0
            || point.x >= width as i32
            || point.y >= height as i32
    })
}

fn body_collision(snake: &SimulatedSnake, other: &SimulatedSnake) -> bool {
    let Some(head) = snake.head() else {
        return false;
    };

    other
        .body
        .iter()
        .skip(1)
        .any(|segment| *segment == head)
}

fn lost_head_to_head(snake: &SimulatedSnake, other: &SimulatedSnake) -> bool {
    snake.head().is_some()
        && snake.head() == other.head()
        && snake.length() <= other.length()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedGameState, SimulatedSnake,
    };

    fn snake(id: &str, health: i32, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: body
                .iter()
                .map(|(x, y)| Coord { x: *x, y: *y })
                .collect(),
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
            aggression: AggressionState::default(),
        }
    }

    #[test]
    fn movement_prepends_head_and_vacates_tail() {
        let initial = state(vec![snake("ours", 100, &[(2, 2), (2, 1), (2, 0)])]);
        let action = JointAction::new().with_move("ours", Direction::Right);

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
        let action = JointAction::new().with_move("ours", Direction::Right);

        let resolved = resolve_turn(&initial, &action).unwrap();
        let ours = resolved.state.snake("ours").unwrap();

        assert_eq!(ours.health, 100);
        assert_eq!(ours.length(), 4);
        assert_eq!(resolved.state.food, Vec::<Coord>::new());
        assert_eq!(resolved.forecast_delta, ForecastDelta::FoodUncertainty);
        assert!(resolved.events.contains(&InstantEvent::AteFood {
            snake: "ours".to_string(),
            food: Coord { x: 3, y: 2 },
        }));
    }

    #[test]
    fn snake_at_one_health_survives_by_eating() {
        let mut initial = state(vec![snake("ours", 1, &[(2, 2), (2, 1)])]);
        initial.food = vec![Coord { x: 3, y: 2 }];
        let action = JointAction::new().with_move("ours", Direction::Right);

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
        let action = JointAction::new().with_move("ours", Direction::Right);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert_eq!(resolved.state.snake("ours").unwrap().health, 100);
    }

    #[test]
    fn stacked_hazards_apply_damage_per_stack() {
        let mut initial = state(vec![snake("ours", 50, &[(2, 2), (2, 1)])]);
        initial.rules.hazard_damage_per_turn = 14;
        initial.hazards = vec![Coord { x: 3, y: 2 }, Coord { x: 3, y: 2 }];
        let action = JointAction::new().with_move("ours", Direction::Right);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert_eq!(resolved.state.snake("ours").unwrap().health, 21);
    }

    #[test]
    fn equal_length_head_to_head_eliminates_both() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 2), (2, 1), (2, 0)]),
            snake("enemy", 100, &[(4, 2), (4, 1), (4, 0)]),
        ]);
        let action = JointAction::new()
            .with_move("ours", Direction::Right)
            .with_move("enemy", Direction::Left);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(!resolved.state.snake("enemy").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::HeadToHead,
        }));
    }

    #[test]
    fn longer_snake_wins_head_to_head() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 2), (2, 1), (2, 0), (1, 0)]),
            snake("enemy", 100, &[(4, 2), (4, 1), (4, 0)]),
        ]);
        let action = JointAction::new()
            .with_move("ours", Direction::Right)
            .with_move("enemy", Direction::Left);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(resolved.state.snake("ours").unwrap().alive);
        assert!(!resolved.state.snake("enemy").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::HeadToHeadWon {
            enemy: "enemy".to_string(),
        }));
    }

    #[test]
    fn body_collision_eliminates_moving_snake() {
        let initial = state(vec![
            snake("ours", 100, &[(1, 2), (1, 1), (1, 0)]),
            snake(\n                "enemy",\n                100,\n                &[(4, 4), (3, 4), (2, 4), (2, 3), (2, 2), (2, 1)],\n            ),
        ]);
        let action = JointAction::new()
            .with_move("ours", Direction::Right)
            .with_move("enemy", Direction::Right);

        let resolved = resolve_turn(&initial, &action).unwrap();

        assert!(!resolved.state.snake("ours").unwrap().alive);
        assert!(resolved.state.snake("enemy").unwrap().alive);
        assert!(resolved.events.contains(&InstantEvent::Died {
            cause: EliminationCause::BodyCollision,
        }));
    }

    #[test]
    fn missing_move_is_rejected() {
        let initial = state(vec![
            snake("ours", 100, &[(2, 2)]),
            snake("enemy", 100, &[(5, 5)]),
        ]);
        let action = JointAction::new().with_move("ours", Direction::Up);

        assert_eq!(
            resolve_turn(&initial, &action),
            Err(ResolveError::MissingMove("enemy".to_string()))
        );
    }
}
