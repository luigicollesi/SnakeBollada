//! Competitive tail-aware flood fill used by the Hobbs-style state evaluator.
//!
//! Every snake expands one cell per cycle, without waiting for blocked cells to
//! open. A body cell becomes enterable after its tail vacates it. Future growth
//! is intentionally not predicted here: the turn resolver handles observed
//! consumption, while unknown future food spawns remain provisional.

use crate::direction::Direction;
use crate::simulation::state::{ActorIndex, SimulatedGameState};
use crate::Coord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TerritoryCellWeights {
    pub(crate) empty: u16,
    pub(crate) food: u16,
    pub(crate) hazard: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TemporalTerritory {
    pub(crate) weighted_space: Vec<u32>,
    pub(crate) claimed_cells: Vec<u32>,
    pub(crate) total_weighted_space: u32,
    owners: Vec<Option<usize>>,
    width: usize,
    height: usize,
}

impl TemporalTerritory {
    pub(crate) fn from_state(
        state: &SimulatedGameState,
        cycles: u8,
        weights: TerritoryCellWeights,
    ) -> Self {
        let width = usize::try_from(state.width).unwrap_or(0);
        let height = usize::try_from(state.height).unwrap_or(0);
        let cells = width.saturating_mul(height);
        let players = state.snakes.len();
        let mut result = Self {
            weighted_space: vec![0; players],
            claimed_cells: vec![0; players],
            total_weighted_space: 0,
            owners: vec![None; cells],
            width,
            height,
        };
        if cells == 0 {
            return result;
        }

        // A repeated tail coordinate remains blocked until its last segment
        // vacates; use the maximum release time among stacked segments.
        let mut release_at = vec![0_u16; cells];
        let mut visited = vec![false; cells];
        let mut frontiers = vec![Vec::<usize>::new(); players];

        let mut actors = state
            .snakes
            .iter()
            .enumerate()
            .filter(|(_, snake)| snake.alive && !snake.body.is_empty())
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        actors.sort_by(|left, right| {
            state.snakes[*right]
                .length()
                .cmp(&state.snakes[*left].length())
                .then_with(|| state.snakes[*left].id.cmp(&state.snakes[*right].id))
        });

        for &actor in &actors {
            let body = &state.snakes[actor].body;
            let length = body.len();
            for (index_from_head, &position) in body.iter().enumerate() {
                if let Some(cell) = index_of(position, width, height) {
                    result.owners[cell] = Some(actor);
                    let turns_to_vacate =
                        u16::try_from(length.saturating_sub(index_from_head)).unwrap_or(u16::MAX);
                    release_at[cell] = release_at[cell].max(turns_to_vacate);
                }
            }
        }

        for &actor in &actors {
            if let Some(cell) = state.snakes[actor]
                .head()
                .and_then(|position| index_of(position, width, height))
            {
                visited[cell] = true;
                frontiers[actor].push(cell);
            }
        }

        for cycle in 1..=u16::from(cycles) {
            let mut next_frontiers = vec![Vec::<usize>::new(); players];
            let mut found_frontier = false;

            for &actor in &actors {
                for &cell in &frontiers[actor] {
                    let position = Coord {
                        x: i32::try_from(cell % width).unwrap_or(i32::MAX),
                        y: i32::try_from(cell / width).unwrap_or(i32::MAX),
                    };
                    for direction in Direction::ALL {
                        let Some(neighbor) = index_of(direction.apply(position), width, height)
                        else {
                            continue;
                        };
                        if visited[neighbor] || release_at[neighbor] > cycle {
                            continue;
                        }
                        visited[neighbor] = true;
                        result.owners[neighbor] = Some(actor);
                        next_frontiers[actor].push(neighbor);
                        found_frontier = true;
                    }
                }
            }
            if !found_frontier {
                break;
            }
            frontiers = next_frontiers;
        }

        let mut food_cells = vec![false; cells];
        for &food in &state.food {
            if let Some(cell) = index_of(food, width, height) {
                food_cells[cell] = true;
            }
        }
        let mut hazard_cells = vec![false; cells];
        for &hazard in &state.hazards {
            if let Some(cell) = index_of(hazard, width, height) {
                hazard_cells[cell] = true;
            }
        }

        for (cell, owner) in result.owners.iter().enumerate() {
            let Some(actor) = owner else {
                continue;
            };
            let weight = if hazard_cells[cell] {
                u32::from(weights.hazard)
            } else if food_cells[cell] {
                u32::from(weights.food)
            } else {
                u32::from(weights.empty)
            };
            result.claimed_cells[*actor] = result.claimed_cells[*actor].saturating_add(1);
            result.weighted_space[*actor] = result.weighted_space[*actor].saturating_add(weight);
            result.total_weighted_space = result.total_weighted_space.saturating_add(weight);
        }
        result
    }

    #[cfg(test)]
    pub(crate) fn owner_at(&self, position: Coord) -> Option<ActorIndex> {
        let cell = index_of(position, self.width, self.height)?;
        self.owners[cell].and_then(ActorIndex::new)
    }

    pub(crate) fn territory_ratio_milli(&self, actor: ActorIndex) -> i64 {
        if self.total_weighted_space == 0 {
            return 0;
        }
        let ours = self
            .weighted_space
            .get(actor.as_usize())
            .copied()
            .unwrap_or(0);
        i64::from(ours)
            .saturating_mul(1000)
            .saturating_div(i64::from(self.total_weighted_space))
    }
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

    const WEIGHTS: TerritoryCellWeights = TerritoryCellWeights {
        empty: 5,
        food: 20,
        hazard: 1,
    };

    fn snake(id: &str, body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: id.to_string(),
            health: 100,
            body: body.iter().map(|&(x, y)| Coord { x, y }).collect(),
            alive: true,
        }
    }

    fn state(snakes: Vec<SimulatedSnake>) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 5,
            height: 5,
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

    #[test]
    fn releases_unstacked_tail_on_first_cycle() {
        let board = state(vec![snake("ours", &[(0, 0), (0, 1), (1, 1), (1, 0)])]);
        let first = TemporalTerritory::from_state(&board, 1, WEIGHTS);
        let second = TemporalTerritory::from_state(&board, 2, WEIGHTS);

        assert_eq!(first.claimed_cells[0], 4);
        assert!(second.claimed_cells[0] > first.claimed_cells[0]);
        assert_eq!(second.owner_at(Coord { x: 2, y: 0 }), ActorIndex::new(0));
    }

    #[test]
    fn stacked_tail_does_not_open_prematurely_or_wait_in_place() {
        let board = state(vec![snake(
            "ours",
            &[(0, 0), (0, 1), (1, 1), (1, 0), (1, 0)],
        )]);
        let result = TemporalTerritory::from_state(&board, 5, WEIGHTS);
        // Both exits remain blocked during cycle one; a stalled frontier dies.
        assert_eq!(result.claimed_cells[0], 4);
        assert_eq!(result.owner_at(Coord { x: 2, y: 0 }), None);
    }

    #[test]
    fn longer_snake_wins_same_cycle_territory_claim() {
        let board = state(vec![
            snake("ours", &[(0, 1), (0, 0), (1, 0)]),
            snake("enemy", &[(2, 1)]),
        ]);
        let result = TemporalTerritory::from_state(&board, 1, WEIGHTS);
        assert_eq!(result.owner_at(Coord { x: 1, y: 1 }), ActorIndex::new(0));
    }

    #[test]
    fn food_is_worth_four_empty_cells_and_hazards_a_fifth() {
        let mut board = state(vec![snake("ours", &[(0, 0)])]);
        let empty = TemporalTerritory::from_state(&board, 4, WEIGHTS);
        board.food.push(Coord { x: 0, y: 1 });
        let food = TemporalTerritory::from_state(&board, 4, WEIGHTS);
        assert_eq!(food.total_weighted_space - empty.total_weighted_space, 15);

        board.food.clear();
        board.hazards.push(Coord { x: 0, y: 1 });
        let hazard = TemporalTerritory::from_state(&board, 4, WEIGHTS);
        assert_eq!(empty.total_weighted_space - hazard.total_weighted_space, 4);
    }

    #[test]
    fn territory_ratio_is_comparable_between_players() {
        let board = state(vec![snake("ours", &[(0, 2)]), snake("enemy", &[(4, 2)])]);
        let result = TemporalTerritory::from_state(&board, 4, WEIGHTS);
        let ours = result.territory_ratio_milli(ActorIndex::new(0).unwrap());
        let enemy = result.territory_ratio_milli(ActorIndex::new(1).unwrap());
        assert_eq!(ours + enemy, 1000);
    }
}
