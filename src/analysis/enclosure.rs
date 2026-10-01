#![allow(dead_code)]

use std::collections::HashMap;

use crate::analysis::{TacticalStateAnalysis, TerritoryAnalysis};
use crate::simulation::state::SimulatedGameState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EnclosureRisk {
    Safe,
    Pressure,
    Constrained,
    Critical,
}

impl EnclosureRisk {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::Safe => 0,
            Self::Pressure => 1,
            Self::Constrained => 2,
            Self::Critical => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EnclosureSnapshot {
    pub(crate) snake_id: String,
    pub(crate) risk: EnclosureRisk,
    pub(crate) space_to_length_milli: u32,
    pub(crate) escape_frontier: u8,
    pub(crate) edge_distance: u16,
    pub(crate) useful_chokes: u8,
    pub(crate) boundary_support: u8,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EnclosureAnalysis {
    snakes: HashMap<String, EnclosureSnapshot>,
}

impl EnclosureAnalysis {
    pub(crate) fn from_parts(
        state: &SimulatedGameState,
        territory: &TerritoryAnalysis,
        tactical: &TacticalStateAnalysis,
    ) -> Self {
        Self::from_parts_with_scope(state, territory, tactical, false)
    }

    pub(crate) fn from_parts_actor_relative(
        state: &SimulatedGameState,
        territory: &TerritoryAnalysis,
        tactical: &TacticalStateAnalysis,
    ) -> Self {
        Self::from_parts_with_scope(state, territory, tactical, true)
    }

    fn from_parts_with_scope(
        state: &SimulatedGameState,
        territory: &TerritoryAnalysis,
        tactical: &TacticalStateAnalysis,
        all_legal_enemy_moves: bool,
    ) -> Self {
        let mut snakes = HashMap::new();

        for snake in state.snakes.iter().filter(|snake| snake.alive) {
            let Some(territory_snapshot) = territory.for_snake(&snake.id) else {
                continue;
            };

            let moves = if snake.id == state.our_snake_id {
                tactical.ours.safe_moves.len()
            } else {
                tactical
                    .enemies
                    .get(&snake.id)
                    .map(|enemy| {
                        if all_legal_enemy_moves || enemy.plausible_moves.is_empty() {
                            enemy.legal_moves.len()
                        } else {
                            enemy.plausible_moves.len()
                        }
                    })
                    .unwrap_or(0)
            };

            let ratio = territory_snapshot.space_to_length_milli(snake.length());
            let useful_chokes = territory_snapshot
                .useful_chokes
                .len()
                .try_into()
                .unwrap_or(u8::MAX);

            let risk = classify_risk(
                territory_snapshot.reachable_space,
                snake.length(),
                ratio,
                moves,
                territory_snapshot.escape_frontier,
                territory_snapshot.edge_distance,
                useful_chokes,
            );

            snakes.insert(
                snake.id.clone(),
                EnclosureSnapshot {
                    snake_id: snake.id.clone(),
                    risk,
                    space_to_length_milli: ratio,
                    escape_frontier: territory_snapshot.escape_frontier,
                    edge_distance: territory_snapshot.edge_distance,
                    useful_chokes,
                    boundary_support: territory_snapshot.boundary_support(),
                },
            );
        }

        Self { snakes }
    }

    pub(crate) fn for_snake(&self, snake_id: &str) -> Option<&EnclosureSnapshot> {
        self.snakes.get(snake_id)
    }

    pub(crate) fn ours<'a>(&'a self, state: &SimulatedGameState) -> Option<&'a EnclosureSnapshot> {
        self.for_snake(&state.our_snake_id)
    }
}

fn classify_risk(
    reachable_space: u32,
    length: usize,
    ratio_milli: u32,
    moves: u8,
    escape_frontier: u8,
    edge_distance: u16,
    useful_chokes: u8,
) -> EnclosureRisk {
    let length = u32::try_from(length).unwrap_or(u32::MAX);

    if reachable_space <= length || (ratio_milli <= 1250 && moves <= 1) || (moves == 0) {
        return EnclosureRisk::Critical;
    }

    if ratio_milli <= 1600
        || (moves <= 1 && ratio_milli <= 2200)
        || (escape_frontier == 0 && ratio_milli <= 2200)
    {
        return EnclosureRisk::Constrained;
    }

    if ratio_milli <= 2400
        || (moves <= 2 && (edge_distance <= 1 || useful_chokes > 0))
        || (escape_frontier <= 1 && useful_chokes > 0)
    {
        return EnclosureRisk::Pressure;
    }

    EnclosureRisk::Safe
}

#[cfg(test)]
mod tests {
    use crate::analysis::{StateAnalysis, TacticalStateAnalysis};
    use crate::enemy::tracing::trace;
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};
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

    fn analyze(state: &SimulatedGameState) -> EnclosureAnalysis {
        let state_analysis = StateAnalysis::from_simulated(state);
        let tracing = trace(state, &state_analysis);
        let tactical = TacticalStateAnalysis::from_state(state, &tracing);
        let territory = TerritoryAnalysis::from_state(state);
        EnclosureAnalysis::from_parts(state, &territory, &tactical)
    }

    #[test]
    fn open_space_is_safe() {
        let state = state(vec![
            snake("ours", &[(1, 1), (1, 0), (0, 0)]),
            snake("enemy", &[(5, 5), (5, 4), (5, 3)]),
        ]);

        assert_eq!(
            analyze(&state).ours(&state).unwrap().risk,
            EnclosureRisk::Safe
        );
    }

    #[test]
    fn tiny_region_is_critical() {
        let state = state(vec![
            snake("ours", &[(1, 1), (1, 0), (0, 0), (0, 1), (0, 2), (1, 2)]),
            snake(
                "enemy",
                &[(3, 3), (3, 2), (3, 1), (2, 1), (2, 2), (2, 3), (2, 4)],
            ),
        ]);

        let ours = analyze(&state).ours(&state).unwrap().clone();
        assert!(ours.risk >= EnclosureRisk::Pressure);
    }
}
