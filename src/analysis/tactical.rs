#![allow(dead_code)]

use std::collections::HashMap;

use crate::board_mask::BoardMask;
use crate::direction::{Direction, MoveMask};
use crate::enemy::tracing::EnemyTracingOutput;
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone)]
pub(crate) struct ThreatMap {
    lethal: BoardMask,
    favorable: BoardMask,
}

impl ThreatMap {
    pub(crate) fn from_state(
        state: &SimulatedGameState,
        tracing: &EnemyTracingOutput,
    ) -> Self {
        let width = state.width as u16;
        let height = state.height as u16;
        let mut lethal = BoardMask::new(width, height);
        let mut favorable = BoardMask::new(width, height);

        let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
            return Self { lethal, favorable };
        };

        for enemy in state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != state.our_snake_id)
        {
            let Some(head) = enemy.head() else {
                continue;
            };
            let moves = tracing
                .for_enemy(&enemy.id)
                .map(|set| set.search_moves())
                .unwrap_or_else(MoveMask::all);

            for direction in moves.iter() {
                let target = direction.apply(head);
                if enemy.length() >= ours.length() {
                    lethal.set(target, true);
                } else {
                    favorable.set(target, true);
                }
            }
        }

        Self { lethal, favorable }
    }

    pub(crate) fn is_lethal(&self, coord: crate::Coord) -> bool {
        self.lethal.contains(coord)
    }

    pub(crate) fn is_favorable(&self, coord: crate::Coord) -> bool {
        self.favorable.contains(coord)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnakeMobilitySnapshot {
    pub(crate) deterministic_moves: MoveMask,
    pub(crate) safe_moves: MoveMask,
    pub(crate) best_reachable_space: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnemyTacticalSnapshot {
    pub(crate) snake_id: String,
    pub(crate) legal_moves: MoveMask,
    pub(crate) plausible_moves: MoveMask,
    pub(crate) best_reachable_space: u32,
    pub(crate) length: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct TacticalStateAnalysis {
    pub(crate) ours: SnakeMobilitySnapshot,
    pub(crate) enemies: HashMap<String, EnemyTacticalSnapshot>,
    pub(crate) threat_map: ThreatMap,
}

impl TacticalStateAnalysis {
    pub(crate) fn from_state(
        state: &SimulatedGameState,
        tracing: &EnemyTracingOutput,
    ) -> Self {
        let mobility = MobilityAnalysis::from_state(state);
        let threat_map = ThreatMap::from_state(state, tracing);

        let deterministic_moves =
            mobility.deterministic_moves_for(state, &state.our_snake_id);

        let safe_moves = state
            .snake(&state.our_snake_id)
            .and_then(|snake| snake.head())
            .map(|head| {
                MoveMask::from_iter(deterministic_moves.iter().filter(|direction| {
                    !threat_map.is_lethal(direction.apply(head))
                }))
            })
            .unwrap_or_else(MoveMask::empty);

        let best_reachable_space = deterministic_moves
            .iter()
            .map(|direction| {
                mobility.reachable_space(state, &state.our_snake_id, direction)
            })
            .max()
            .unwrap_or(0);

        let mut enemies = HashMap::new();
        for enemy in state
            .snakes
            .iter()
            .filter(|snake| snake.alive && snake.id != state.our_snake_id)
        {
            let Some(move_set) = tracing.for_enemy(&enemy.id) else {
                continue;
            };

            let best_reachable_space = move_set
                .search_moves()
                .iter()
                .map(|direction| mobility.reachable_space(state, &enemy.id, direction))
                .max()
                .unwrap_or(0);

            enemies.insert(
                enemy.id.clone(),
                EnemyTacticalSnapshot {
                    snake_id: enemy.id.clone(),
                    legal_moves: move_set.legal_moves,
                    plausible_moves: move_set.plausible_moves,
                    best_reachable_space,
                    length: enemy.length(),
                },
            );
        }

        Self {
            ours: SnakeMobilitySnapshot {
                deterministic_moves,
                safe_moves,
                best_reachable_space,
            },
            enemies,
            threat_map,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::StateAnalysis;
    use crate::enemy::tracing::trace;
    use crate::forecast::ForecastCertainty;
    use crate::simulation::state::{
        AggressionState, RulesContext, SimulatedSnake,
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
    fn equal_or_longer_enemy_marks_contested_destination_lethal() {
        let state = state(vec![
            snake("ours", &[(2, 1), (1, 1), (1, 0)]),
            snake("enemy", &[(2, 3), (3, 3), (3, 2)]),
        ]);
        let analysis =
            StateAnalysis::from_simulated(&state, ForecastCertainty::Deterministic);
        let tracing = trace(&state, &analysis);
        let tactical = TacticalStateAnalysis::from_state(&state, &tracing);

        let contested = Coord { x: 2, y: 2 };
        assert!(tactical.threat_map.is_lethal(contested));
        assert!(!tactical.ours.safe_moves.contains(Direction::Up));
    }

    #[test]
    fn shorter_enemy_marks_destination_favorable_not_lethal() {
        let state = state(vec![
            snake("ours", &[(2, 1), (1, 1), (1, 0), (0, 0)]),
            snake("enemy", &[(2, 3), (3, 3)]),
        ]);
        let analysis =
            StateAnalysis::from_simulated(&state, ForecastCertainty::Deterministic);
        let tracing = trace(&state, &analysis);
        let tactical = TacticalStateAnalysis::from_state(&state, &tracing);

        let contested = Coord { x: 2, y: 2 };
        assert!(tactical.threat_map.is_favorable(contested));
        assert!(!tactical.threat_map.is_lethal(contested));
    }
}
