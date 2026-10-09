//! One state-based strategic score inspired by Hovering Hobbs.
//!
//! Food and hunting do not have their own transition rewards: food is valued
//! through territorial ownership, realized length, and emergency health policy.
//! A low-health state ranks below a healthy non-terminal state, as in Hobbs.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::analysis::{TemporalTerritory, TerritoryCellWeights};
use crate::direction::Direction;
use crate::simulation::state::{ActorIndex, SimulatedGameState};
use crate::Coord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HobbsScoreParams {
    pub(crate) fill_cycles: u8,
    pub(crate) cell_weights: TerritoryCellWeights,
    pub(crate) length_weight_milli: i64,
    pub(crate) length_cap: i64,
    pub(crate) low_health_duel: i32,
    pub(crate) low_health_crowded: i32,
}

impl HobbsScoreParams {
    pub(crate) const STANDARD: Self = Self {
        fill_cycles: 12,
        cell_weights: TerritoryCellWeights {
            empty: 5,
            food: 20,
            hazard: 1,
        },
        length_weight_milli: 160,
        length_cap: 3,
        low_health_duel: 60,
        low_health_crowded: 85,
    };
}

impl Default for HobbsScoreParams {
    fn default() -> Self {
        Self::STANDARD
    }
}

/// Ordering is intentional: a loss is worse than a tie; critically low
/// health ranks below an ordinary living state, and any win beats everything.
/// A closer reachable food target has a greater negative_distance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum StateScore {
    Loss,
    Tie,
    LowHealth {
        negative_food_distance: Option<i32>,
        territory_milli: i64,
    },
    Normal {
        utility_milli: i64,
    },
    Win,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HobbsEvaluation {
    pub(crate) score: StateScore,
    pub(crate) territory_milli: i64,
    pub(crate) length_bonus_milli: i64,
    pub(crate) health: i32,
}

impl HobbsEvaluation {
    fn terminal(score: StateScore, health: i32) -> Self {
        Self {
            score,
            territory_milli: 0,
            length_bonus_milli: 0,
            health,
        }
    }
}

pub(crate) fn evaluate_hobbs_state(
    state: &SimulatedGameState,
    territory: &TemporalTerritory,
    actor: ActorIndex,
    params: HobbsScoreParams,
) -> HobbsEvaluation {
    let Some(snake) = state.snake_at(actor) else {
        return HobbsEvaluation::terminal(StateScore::Loss, 0);
    };
    let living = state.snakes.iter().filter(|candidate| candidate.alive).count();
    if !snake.alive {
        return HobbsEvaluation::terminal(
            if living == 0 {
                StateScore::Tie
            } else {
                StateScore::Loss
            },
            snake.health,
        );
    }
    if living == 1 {
        return HobbsEvaluation::terminal(StateScore::Win, snake.health);
    }

    let longest_rival = state
        .snakes
        .iter()
        .filter(|candidate| candidate.alive && candidate.id != snake.id)
        .map(|candidate| candidate.length())
        .max()
        .unwrap_or(snake.length());
    let length_delta = i64::try_from(snake.length())
        .unwrap_or(i64::MAX)
        .saturating_sub(i64::try_from(longest_rival).unwrap_or(i64::MAX));
    let length_bonus_milli = length_delta
        .clamp(-params.length_cap.max(0), params.length_cap.max(0))
        .saturating_mul(params.length_weight_milli);
    let territory_milli = territory.territory_ratio_milli(actor);
    let threshold = if living >= 3 {
        params.low_health_crowded
    } else {
        params.low_health_duel
    };

    let score = if snake.health < threshold {
        StateScore::LowHealth {
            negative_food_distance: nearest_food_distance(state, actor).map(i32::saturating_neg),
            territory_milli,
        }
    } else {
        StateScore::Normal {
            utility_milli: territory_milli.saturating_add(length_bonus_milli),
        }
    };

    HobbsEvaluation {
        score,
        territory_milli,
        length_bonus_milli,
        health: snake.health,
    }
}

/// Shortest health-sensitive path to a known food. Body segments other than
/// tails are blocked; entering a hazard adds one extra unit of path cost.
/// This is deliberately a spatial estimate, not an assumption about spawns.
fn nearest_food_distance(state: &SimulatedGameState, actor: ActorIndex) -> Option<i32> {
    let head = state.snake_at(actor)?.head()?;
    let width = usize::try_from(state.width).ok()?;
    let height = usize::try_from(state.height).ok()?;
    let cells = width.checked_mul(height)?;
    let start = index_of(head, width, height)?;
    let mut blocked = vec![false; cells];

    for snake in state.snakes.iter().filter(|snake| snake.alive) {
        for &segment in snake.body.iter().take(snake.body.len().saturating_sub(1)) {
            if let Some(index) = index_of(segment, width, height) {
                blocked[index] = true;
            }
        }
    }
    blocked[start] = false;

    let mut food_cells = vec![false; cells];
    for &food in &state.food {
        if let Some(cell) = index_of(food, width, height) {
            food_cells[cell] = true;
        }
    }
    if !food_cells.iter().any(|value| *value) {
        return None;
    }
    let mut hazards = vec![false; cells];
    for &hazard in &state.hazards {
        if let Some(cell) = index_of(hazard, width, height) {
            hazards[cell] = true;
        }
    }

    let mut distances = vec![i32::MAX; cells];
    let mut heap = BinaryHeap::new();
    distances[start] = 0;
    heap.push(Reverse((0_i32, start)));
    while let Some(Reverse((distance, cell))) = heap.pop() {
        if distance != distances[cell] {
            continue;
        }
        if food_cells[cell] {
            return Some(distance);
        }
        let current = Coord {
            x: i32::try_from(cell % width).ok()?,
            y: i32::try_from(cell / width).ok()?,
        };
        for direction in Direction::ALL {
            let Some(next) = index_of(direction.apply(current), width, height) else {
                continue;
            };
            if blocked[next] && !food_cells[next] {
                continue;
            }
            let cost = 1_i32.saturating_add(i32::from(hazards[next]));
            let candidate = distance.saturating_add(cost);
            if candidate < distances[next] {
                distances[next] = candidate;
                heap.push(Reverse((candidate, next)));
            }
        }
    }
    None
}

fn index_of(position: Coord, width: usize, height: usize) -> Option<usize> {
    let x = usize::try_from(position.x).ok()?;
    let y = usize::try_from(position.y).ok()?;
    if x >= width || y >= height {
        return None;
    }
    y.checked_mul(width)?.checked_add(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::state::{RulesContext, SimulatedSnake};

    fn snake(id: &str, length: usize, health: i32, head: Coord) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health,
            body: vec![head; length],
            alive: true,
        }
    }

    fn state(snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 0,
            width: 7,
            height: 7,
            food: vec![Coord { x: 3, y: 3 }],
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

    fn evaluate(state: &SimulatedGameState, actor: usize) -> HobbsEvaluation {
        let params = HobbsScoreParams::STANDARD;
        let territory = TemporalTerritory::from_state(
            state,
            params.fill_cycles,
            params.cell_weights,
        );
        evaluate_hobbs_state(state, &territory, ActorIndex::new(actor).unwrap(), params)
    }

    #[test]
    fn length_reward_saturates_at_three_segments() {
        let ours = Coord { x: 1, y: 1 };
        let enemy = Coord { x: 5, y: 5 };
        let board = state(vec![
            snake("ours", 4, 100, ours),
            snake("enemy", 1, 100, enemy),
        ]);
        assert_eq!(evaluate(&board, 0).length_bonus_milli, 480);
        let board = state(vec![
            snake("ours", 9, 100, ours),
            snake("enemy", 1, 100, enemy),
        ]);
        assert_eq!(evaluate(&board, 0).length_bonus_milli, 480);
        assert_eq!(evaluate(&board, 1).length_bonus_milli, -480);
    }

    #[test]
    fn duel_food_policy_starts_below_sixty_health() {
        let our_head = Coord { x: 2, y: 3 };
        let enemy_head = Coord { x: 5, y: 5 };
        let board = state(vec![
            snake("ours", 1, 59, our_head),
            snake("enemy", 1, 100, enemy_head),
        ]);
        assert!(matches!(
            evaluate(&board, 0).score,
            StateScore::LowHealth {
                negative_food_distance: Some(-1),
                ..
            }
        ));
        let mut healthy = board;
        healthy.snakes[0].health = 60;
        assert!(matches!(evaluate(&healthy, 0).score, StateScore::Normal { .. }));
    }

    #[test]
    fn crowded_board_raises_food_threshold_to_eighty_five() {
        let board = state(vec![
            snake("ours", 1, 84, Coord { x: 0, y: 0 }),
            snake("enemy", 1, 100, Coord { x: 6, y: 6 }),
            snake("enemy2", 1, 100, Coord { x: 6, y: 0 }),
        ]);
        assert!(matches!(evaluate(&board, 0).score, StateScore::LowHealth { .. }));
        let mut healthy = board;
        healthy.snakes[0].health = 85;
        assert!(matches!(evaluate(&healthy, 0).score, StateScore::Normal { .. }));
    }

    #[test]
    fn terminal_outcome_has_absolute_precedence() {
        let mut board = state(vec![
            snake("ours", 1, 100, Coord { x: 0, y: 0 }),
            snake("enemy", 1, 100, Coord { x: 6, y: 6 }),
        ]);
        board.snakes[1].alive = false;
        assert_eq!(evaluate(&board, 0).score, StateScore::Win);
        board.snakes[0].alive = false;
        assert_eq!(evaluate(&board, 0).score, StateScore::Tie);
        board.snakes[1].alive = true;
        assert_eq!(evaluate(&board, 0).score, StateScore::Loss);
    }

    #[test]
    fn score_orders_win_normal_low_health_tie_and_loss() {
        assert!(StateScore::Win > StateScore::Normal { utility_milli: 2000 });
        assert!(StateScore::Normal { utility_milli: -1000 } > StateScore::LowHealth {
            negative_food_distance: Some(-1),
            territory_milli: 1000,
        });
        assert!(StateScore::LowHealth {
            negative_food_distance: Some(-2),
            territory_milli: 100,
        } > StateScore::LowHealth {
            negative_food_distance: Some(-3),
            territory_milli: 1000,
        });
        assert!(StateScore::Tie > StateScore::Loss);
    }
}
