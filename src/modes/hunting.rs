#![allow(dead_code)]

use crate::analysis::TacticalStateAnalysis;
use crate::direction::Direction;
use crate::enemy::tracing::EnemyTracingOutput;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HuntingCandidate {
    pub(crate) target: String,
    pub(crate) first_move: Direction,
    pub(crate) target_moves_before: u8,
    pub(crate) contested_cell: Coord,
    pub(crate) length_advantage: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HuntingModeOutput {
    pub(crate) candidates: Vec<HuntingCandidate>,
}

pub(crate) fn analyze(
    state: &SimulatedGameState,
    tactical: &TacticalStateAnalysis,
    tracing: &EnemyTracingOutput,
) -> HuntingModeOutput {
    let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
        return HuntingModeOutput::default();
    };
    let Some(our_head) = ours.head() else {
        return HuntingModeOutput::default();
    };

    let mut candidates = Vec::new();

    for enemy in state
        .snakes
        .iter()
        .filter(|snake| snake.alive && snake.id != state.our_snake_id)
    {
        let Some(enemy_head) = enemy.head() else {
            continue;
        };
        let Some(move_set) = tracing.for_enemy(&enemy.id) else {
            continue;
        };

        let length_advantage = ours.length() as i32 - enemy.length() as i32;
        if length_advantage <= 0 {
            continue;
        }

        let target_moves = if !move_set.plausible_moves.is_empty() {
            move_set.plausible_moves
        } else {
            move_set.legal_moves
        };

        for enemy_direction in target_moves.iter() {
            let contested_cell = enemy_direction.apply(enemy_head);
            if !tactical.threat_map.is_favorable(contested_cell) {
                continue;
            }

            for our_direction in tactical.ours.deterministic_moves.iter() {
                if our_direction.apply(our_head) != contested_cell {
                    continue;
                }

                candidates.push(HuntingCandidate {
                    target: enemy.id.clone(),
                    first_move: our_direction,
                    target_moves_before: move_set.plausible_moves.len(),
                    contested_cell,
                    length_advantage,
                });
            }
        }
    }

    candidates.sort_by(|left, right| {
        left.target
            .cmp(&right.target)
            .then_with(|| left.first_move.rank().cmp(&right.first_move.rank()))
            .then_with(|| left.contested_cell.cmp(&right.contested_cell))
    });
    candidates.dedup();

    HuntingModeOutput { candidates }
}

#[cfg(test)]
mod tests {
    use crate::analysis::StateAnalysis;
    use crate::enemy::tracing::trace;
    use crate::forecast::ForecastCertainty;
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};

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
    fn favorable_contested_cell_creates_hunting_candidate() {
        let state = state(vec![
            snake("ours", &[(2, 1), (1, 1), (1, 0), (0, 0)]),
            snake("enemy", &[(2, 3), (3, 3)]),
        ]);
        let analysis = StateAnalysis::from_simulated(&state, ForecastCertainty::Deterministic);
        let tracing = trace(&state, &analysis);
        let tactical = TacticalStateAnalysis::from_state(&state, &tracing);

        let output = analyze(&state, &tactical, &tracing);

        assert!(output.candidates.iter().any(|candidate| {
            candidate.target == "enemy"
                && candidate.first_move == Direction::Up
                && candidate.contested_cell == Coord { x: 2, y: 2 }
        }));
    }

    #[test]
    fn equal_length_contest_is_not_offensive_candidate() {
        let state = state(vec![
            snake("ours", &[(2, 1), (1, 1), (1, 0)]),
            snake("enemy", &[(2, 3), (3, 3), (3, 2)]),
        ]);
        let analysis = StateAnalysis::from_simulated(&state, ForecastCertainty::Deterministic);
        let tracing = trace(&state, &analysis);
        let tactical = TacticalStateAnalysis::from_state(&state, &tracing);

        assert!(analyze(&state, &tactical, &tracing).candidates.is_empty());
    }
}
