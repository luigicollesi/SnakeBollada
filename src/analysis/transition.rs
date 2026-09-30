#![allow(dead_code)]

use crate::analysis::{EnemyTacticalSnapshot, TacticalStateAnalysis};
use crate::direction::Direction;
use crate::simulation::resolver::{InstantEvent, TurnResolution};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransitionAnalysis {
    pub(crate) events: Vec<InstantEvent>,
}

pub(crate) fn analyze_transition(
    before: &TacticalStateAnalysis,
    resolution: &TurnResolution,
    after: &TacticalStateAnalysis,
) -> TransitionAnalysis {
    let mut events = resolution.events.clone();

    derive_survival_events(&mut events, resolution, after);
    derive_hunting_events(&mut events, before, resolution, after);

    TransitionAnalysis { events }
}

fn derive_survival_events(
    events: &mut Vec<InstantEvent>,
    resolution: &TurnResolution,
    after: &TacticalStateAnalysis,
) {
    let ours_alive = resolution
        .state
        .snake(&resolution.state.our_snake_id)
        .is_some_and(|snake| snake.alive);

    if !ours_alive
        || events
            .iter()
            .any(|event| matches!(event, InstantEvent::Died { .. }))
    {
        return;
    }

    match after.ours.safe_moves.len() {
        0 => events.push(InstantEvent::SelfDeadEnd),
        1 => events.push(InstantEvent::SelfConstrained { remaining_moves: 1 }),
        2 => events.push(InstantEvent::SelfConstrained { remaining_moves: 2 }),
        _ => {}
    }
}

fn derive_hunting_events(
    events: &mut Vec<InstantEvent>,
    before: &TacticalStateAnalysis,
    resolution: &TurnResolution,
    after: &TacticalStateAnalysis,
) {
    for (enemy_id, before_enemy) in &before.enemies {
        if enemy_killed(events, enemy_id) {
            continue;
        }

        let Some(after_enemy) = after.enemies.get(enemy_id) else {
            continue;
        };

        let caused_by_ours =
            our_body_contributes(&resolution.state, enemy_id, after_enemy);

        if after_enemy.legal_moves.is_empty() {
            events.push(InstantEvent::EnemyTrapped {
                enemy: enemy_id.clone(),
                caused_by_ours,
            });
            continue;
        }

        if before_enemy.plausible_moves.len() > 1 && after_enemy.plausible_moves.len() == 1 {
            events.push(InstantEvent::EnemyForced {
                enemy: enemy_id.clone(),
                remaining_moves: 1,
                caused_by_ours,
            });
        }
    }
}

fn our_body_contributes(
    state: &crate::simulation::state::SimulatedGameState,
    enemy_id: &str,
    enemy: &EnemyTacticalSnapshot,
) -> bool {
    let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
        return false;
    };
    let Some(enemy_snake) = state.snake(enemy_id).filter(|snake| snake.alive) else {
        return false;
    };
    let Some(enemy_head) = enemy_snake.head() else {
        return false;
    };

    let retained_our_body = &ours.body[..ours.body.len().saturating_sub(1)];

    Direction::ALL.into_iter().any(|direction| {
        !enemy.legal_moves.contains(direction)
            && retained_our_body.contains(&direction.apply(enemy_head))
    })
}

fn enemy_killed(events: &[InstantEvent], enemy_id: &str) -> bool {
    events.iter().any(|event| {
        matches!(
            event,
            InstantEvent::EnemyKilled { enemy, .. } if enemy == enemy_id
        )
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::direction::{Direction, MoveMask};
    use crate::enemy::tracing::{EnemyMoveSet, EnemyTracingOutput};
    use crate::simulation::resolver::{EliminationAttribution, EliminationCause, ForecastDelta};
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedGameState, SimulatedSnake,
    };
    use crate::Coord;

    use super::*;

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
            turn: 2,
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

    fn tracing(enemy_id: &str, legal: MoveMask, plausible: MoveMask) -> EnemyTracingOutput {
        EnemyTracingOutput {
            enemies: HashMap::from([(
                enemy_id.to_string(),
                EnemyMoveSet {
                    legal_moves: legal,
                    plausible_moves: plausible,
                    eliminations: vec![],
                },
            )]),
        }
    }

    #[test]
    fn transition_marks_self_forced_when_only_one_robust_move_remains() {
        let state = state(vec![snake("ours", &[(0, 0)]), snake("enemy", &[(2, 0)])]);
        let enemy_tracing = tracing(
            "enemy",
            MoveMask::single(Direction::Left),
            MoveMask::single(Direction::Left),
        );
        let tactical = TacticalStateAnalysis::from_state(&state, &enemy_tracing);
        let resolution = TurnResolution {
            state: state.clone(),
            events: vec![],
            forecast_delta: ForecastDelta::None,
        };

        let analyzed = analyze_transition(&tactical, &resolution, &tactical);

        assert!(analyzed
            .events
            .contains(&InstantEvent::SelfConstrained { remaining_moves: 1 }));
    }

    #[test]
    fn transition_marks_enemy_forced_on_mobility_reduction() {
        let state = state(vec![
            snake("ours", &[(1, 1), (1, 0), (0, 0)]),
            snake("enemy", &[(5, 5), (5, 4)]),
        ]);
        let before_tracing = tracing(
            "enemy",
            MoveMask::from_iter([Direction::Up, Direction::Left]),
            MoveMask::from_iter([Direction::Up, Direction::Left]),
        );
        let after_tracing = tracing(
            "enemy",
            MoveMask::single(Direction::Left),
            MoveMask::single(Direction::Left),
        );
        let before = TacticalStateAnalysis::from_state(&state, &before_tracing);
        let after = TacticalStateAnalysis::from_state(&state, &after_tracing);
        let resolution = TurnResolution {
            state: state.clone(),
            events: vec![],
            forecast_delta: ForecastDelta::None,
        };

        let analyzed = analyze_transition(&before, &resolution, &after);

        assert!(analyzed.events.contains(&InstantEvent::EnemyForced {
            enemy: "enemy".to_string(),
            remaining_moves: 1,
            caused_by_ours: false,
        }));
    }

    #[test]
    fn transition_marks_enemy_trapped_when_no_legal_moves_remain() {
        let state = state(vec![
            snake("ours", &[(1, 1), (1, 0), (0, 0)]),
            snake("enemy", &[(5, 5), (5, 4)]),
        ]);
        let before_tracing = tracing(
            "enemy",
            MoveMask::from_iter([Direction::Up, Direction::Left]),
            MoveMask::from_iter([Direction::Up, Direction::Left]),
        );
        let after_tracing = tracing("enemy", MoveMask::empty(), MoveMask::empty());
        let before = TacticalStateAnalysis::from_state(&state, &before_tracing);
        let after = TacticalStateAnalysis::from_state(&state, &after_tracing);
        let resolution = TurnResolution {
            state: state.clone(),
            events: vec![],
            forecast_delta: ForecastDelta::None,
        };

        let analyzed = analyze_transition(&before, &resolution, &after);

        assert!(analyzed.events.contains(&InstantEvent::EnemyTrapped {
            enemy: "enemy".to_string(),
            caused_by_ours: false,
        }));
    }

    #[test]
    fn same_edge_can_carry_survival_and_hunting_events() {
        let state = state(vec![snake("ours", &[(0, 0)]), snake("enemy", &[(2, 0)])]);
        let before_tracing = tracing(
            "enemy",
            MoveMask::from_iter([Direction::Up, Direction::Left]),
            MoveMask::from_iter([Direction::Up, Direction::Left]),
        );
        let after_tracing = tracing(
            "enemy",
            MoveMask::single(Direction::Left),
            MoveMask::single(Direction::Left),
        );
        let before = TacticalStateAnalysis::from_state(&state, &before_tracing);
        let after = TacticalStateAnalysis::from_state(&state, &after_tracing);
        let resolution = TurnResolution {
            state: state.clone(),
            events: vec![],
            forecast_delta: ForecastDelta::None,
        };

        let analyzed = analyze_transition(&before, &resolution, &after);

        assert!(analyzed
            .events
            .contains(&InstantEvent::SelfConstrained { remaining_moves: 1 }));
        assert!(analyzed.events.contains(&InstantEvent::EnemyForced {
            enemy: "enemy".to_string(),
            remaining_moves: 1,
            caused_by_ours: false,
        }));
    }

    #[test]
    fn our_retained_body_marks_tactical_pressure_as_causal() {
        let state = state(vec![
            snake("ours", &[(3, 2), (3, 1), (3, 0)]),
            snake("enemy", &[(2, 2), (2, 1)]),
        ]);
        let after_enemy = EnemyTacticalSnapshot {
            snake_id: "enemy".to_string(),
            legal_moves: MoveMask::from_iter([Direction::Up, Direction::Left, Direction::Down]),
            plausible_moves: MoveMask::from_iter([Direction::Up, Direction::Left, Direction::Down]),
            best_reachable_space: 10,
            length: 2,
        };

        assert!(our_body_contributes(&state, "enemy", &after_enemy));
    }

    #[test]
    fn enemy_kill_supersedes_forced_and_trapped_derivation() {
        let state = state(vec![
            snake("ours", &[(1, 1), (1, 0), (0, 0)]),
            snake("enemy", &[(5, 5), (5, 4)]),
        ]);
        let before_tracing = tracing(
            "enemy",
            MoveMask::from_iter([Direction::Up, Direction::Left]),
            MoveMask::from_iter([Direction::Up, Direction::Left]),
        );
        let after_tracing = tracing(
            "enemy",
            MoveMask::single(Direction::Left),
            MoveMask::single(Direction::Left),
        );
        let before = TacticalStateAnalysis::from_state(&state, &before_tracing);
        let after = TacticalStateAnalysis::from_state(&state, &after_tracing);
        let resolution = TurnResolution {
            state: state.clone(),
            events: vec![InstantEvent::EnemyKilled {
                enemy: "enemy".to_string(),
                cause: EliminationCause::HeadToHead,
                attribution: EliminationAttribution::OurSnake,
            }],
            forecast_delta: ForecastDelta::None,
        };

        let analyzed = analyze_transition(&before, &resolution, &after);

        assert_eq!(
            analyzed
                .events
                .iter()
                .filter(|event| matches!(event, InstantEvent::EnemyForced { .. }))
                .count(),
            0
        );
        assert_eq!(
            analyzed
                .events
                .iter()
                .filter(|event| matches!(event, InstantEvent::EnemyTrapped { .. }))
                .count(),
            0
        );
    }
}
