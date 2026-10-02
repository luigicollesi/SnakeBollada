#![allow(dead_code)]

use std::collections::HashMap;

use rayon::prelude::*;

use crate::analysis::TerritoryAnalysis;
use crate::direction::{Direction, MoveMask};
use crate::simulation::mobility::MobilityAnalysis;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BorderFobicSnapshot {
    pub(crate) fear_milli: u16,
    pub(crate) head_edge_distance: u16,
    pub(crate) body_on_edge: u16,
    pub(crate) leading_edge_chain: u16,
    pub(crate) safe_move_count: u8,
    pub(crate) inward_safe_moves: u8,
    pub(crate) corner_contact: bool,
    pub(crate) preference_milli: u16,
    pub(crate) structural_risk_milli: u16,
    pub(crate) inward_control_milli: u16,
    pub(crate) enemy_pin_risk_milli: u16,
    pub(crate) escape_pressure_milli: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BorderFobicAnalysis {
    our_id: String,
    snakes: HashMap<String, BorderFobicSnapshot>,
}

impl BorderFobicAnalysis {
    pub(crate) fn from_parts_with_territory_actor_relative(
        state: &SimulatedGameState,
        mobility: &MobilityAnalysis,
        territory: &TerritoryAnalysis,
    ) -> Self {
        let snakes = state
            .snakes
            .par_iter()
            .filter(|snake| snake.alive)
            .filter_map(|snake| {
                let head = snake.head()?;
                let safe_moves = mobility.deterministic_moves_for(state, &snake.id);
                let fear_milli = length_fear_milli(state, snake.length());
                let head_edge_distance = edge_distance(state, head);
                let body_on_edge = snake
                    .body
                    .iter()
                    .filter(|segment| edge_distance(state, **segment) == 0)
                    .count()
                    .try_into()
                    .unwrap_or(u16::MAX);
                let leading_edge_chain = snake
                    .body
                    .iter()
                    .take_while(|segment| edge_distance(state, **segment) == 0)
                    .count()
                    .try_into()
                    .unwrap_or(u16::MAX);
                let safe_move_count = safe_moves.len().min(3);
                let inward_safe_moves = safe_moves
                    .iter()
                    .filter(|direction| {
                        edge_distance(state, direction.apply(head)) > head_edge_distance
                    })
                    .count()
                    .try_into()
                    .unwrap_or(u8::MAX);
                let corner_contact = is_corner(state, head);

                let preference_base = match head_edge_distance {
                    0 => 1000_u32,
                    1 => 350,
                    _ => 0,
                };
                let preference_milli = scale_by_fear(preference_base, fear_milli);

                let length = u32::try_from(snake.length()).unwrap_or(u32::MAX).max(1);
                let edge_ratio = u32::from(body_on_edge)
                    .saturating_mul(1000)
                    .saturating_div(length);
                let chain_ratio = u32::from(leading_edge_chain)
                    .saturating_mul(1000)
                    .saturating_div(length);
                let exit_penalty = if head_edge_distance == 0 {
                    match inward_safe_moves {
                        0 => 450,
                        1 => 180,
                        _ => 0,
                    }
                } else {
                    0
                };
                let corner_penalty = if corner_contact { 180 } else { 0 };

                let structural_base = edge_ratio
                    .saturating_mul(30)
                    .saturating_div(100)
                    .saturating_add(chain_ratio.saturating_mul(40).saturating_div(100))
                    .saturating_add(exit_penalty)
                    .saturating_add(corner_penalty)
                    .min(1000);
                let structural_risk_milli = scale_by_fear(structural_base, fear_milli);
                let inward_control_milli = inward_control_milli(
                    state,
                    territory,
                    &snake.id,
                    safe_moves,
                    head,
                    head_edge_distance,
                );
                let enemy_pin_risk_milli = if head_edge_distance > 1 {
                    0
                } else {
                    let proximity = if head_edge_distance == 0 {
                        1000_u32
                    } else {
                        500
                    };
                    1000_u32
                        .saturating_sub(u32::from(inward_control_milli))
                        .saturating_mul(proximity)
                        .saturating_div(1000)
                        .min(1000)
                        .try_into()
                        .unwrap_or(1000)
                };
                let escape_pressure_milli = border_escape_pressure_milli(
                    head_edge_distance,
                    safe_move_count,
                    inward_safe_moves,
                    enemy_pin_risk_milli,
                    corner_contact,
                );

                Some((
                    snake.id.clone(),
                    BorderFobicSnapshot {
                        fear_milli,
                        head_edge_distance,
                        body_on_edge,
                        leading_edge_chain,
                        safe_move_count,
                        inward_safe_moves,
                        corner_contact,
                        preference_milli,
                        structural_risk_milli,
                        inward_control_milli,
                        enemy_pin_risk_milli,
                        escape_pressure_milli,
                    },
                ))
            })
            .collect::<HashMap<_, _>>();

        Self {
            our_id: state.our_snake_id.clone(),
            snakes,
        }
    }

    pub(crate) fn for_snake(&self, snake_id: &str) -> Option<&BorderFobicSnapshot> {
        self.snakes.get(snake_id)
    }

    pub(crate) fn ours(&self) -> Option<&BorderFobicSnapshot> {
        self.for_snake(&self.our_id)
    }

    pub(crate) fn move_preference_milli(
        &self,
        state: &SimulatedGameState,
        direction: Direction,
    ) -> u16 {
        self.move_preference_for(state, &state.our_snake_id, direction)
    }

    pub(crate) fn move_preference_for(
        &self,
        state: &SimulatedGameState,
        snake_id: &str,
        direction: Direction,
    ) -> u16 {
        let Some(snapshot) = self.for_snake(snake_id) else {
            return 0;
        };
        let Some(head) = state.snake(snake_id).and_then(|snake| snake.head()) else {
            return 0;
        };
        let target_distance = edge_distance(state, direction.apply(head));

        if target_distance > snapshot.head_edge_distance {
            return snapshot.preference_milli.saturating_div(4);
        }

        let base = match target_distance {
            0 => 1000_u32,
            1 => 350,
            _ => 0,
        };
        let mut preference = scale_by_fear(base, snapshot.fear_milli);
        if snapshot.head_edge_distance == 0 && target_distance == 0 {
            preference = preference.saturating_add(snapshot.fear_milli.saturating_div(5));
        }
        preference.min(1000)
    }
}

fn inward_control_milli(
    state: &SimulatedGameState,
    territory: &TerritoryAnalysis,
    actor_id: &str,
    safe_moves: MoveMask,
    head: Coord,
    head_edge_distance: u16,
) -> u16 {
    let inward = safe_moves
        .iter()
        .filter(|direction| edge_distance(state, direction.apply(head)) > head_edge_distance)
        .collect::<Vec<_>>();

    if inward.is_empty() {
        return 0;
    }

    inward
        .into_iter()
        .map(|direction| {
            let first = direction.apply(head);
            let first_score = control_score(territory, actor_id, first);
            let second = direction.apply(first);
            let second_score = if edge_distance(state, second) > edge_distance(state, first) {
                control_score(territory, actor_id, second)
            } else {
                first_score
            };

            u32::from(first_score)
                .saturating_mul(700)
                .saturating_add(u32::from(second_score).saturating_mul(300))
                .saturating_div(1000)
                .try_into()
                .unwrap_or(0)
        })
        .max()
        .unwrap_or(0)
}

fn control_score(territory: &TerritoryAnalysis, actor_id: &str, coord: Coord) -> u16 {
    match territory.competitive_owner_at(coord) {
        Some(owner) if owner == actor_id => 1000,
        Some(_) => 100,
        None if territory.competitive_is_contested_at(coord) => 450,
        None => 700,
    }
}

fn length_fear_milli(state: &SimulatedGameState, length: usize) -> u16 {
    let min_dimension = state.width.min(state.height).max(1);
    let full_fear_length = min_dimension.saturating_mul(2);
    let start = 4_u32;
    let progress = u32::try_from(length)
        .unwrap_or(u32::MAX)
        .saturating_sub(start);
    let span = full_fear_length.saturating_sub(start).max(1);
    let growth = progress.saturating_mul(1000).saturating_div(span).min(1000);

    150_u32
        .saturating_add(growth.saturating_mul(850).saturating_div(1000))
        .min(1000)
        .try_into()
        .unwrap_or(1000)
}

fn border_escape_pressure_milli(
    head_edge_distance: u16,
    safe_move_count: u8,
    inward_safe_moves: u8,
    enemy_pin_risk_milli: u16,
    corner_contact: bool,
) -> u16 {
    if head_edge_distance > 1 {
        return 0;
    }

    let mobility_pressure = match (head_edge_distance, safe_move_count) {
        (0, 0..=1) => 1000_u32,
        (0, 2) => 600,
        (0, _) => 400,
        (1, 0..=1) => 850,
        (1, 2) => 500,
        (1, _) => 150,
        _ => 0,
    };
    let inward_pressure = match (head_edge_distance, inward_safe_moves) {
        (0, 0) => 1000_u32,
        (0, 1) => 450,
        (0, _) => 100,
        (1, 0) => 700,
        (1, 1) => 300,
        (1, _) => 50,
        _ => 0,
    };
    let pin = u32::from(enemy_pin_risk_milli);
    let primary = mobility_pressure.max(inward_pressure).max(pin);
    let secondary = [mobility_pressure, inward_pressure, pin]
        .into_iter()
        .filter(|value| *value < primary)
        .max()
        .unwrap_or(0);
    let mut combined = primary
        .saturating_add(secondary.saturating_mul(200).saturating_div(1000))
        .min(1000);

    if head_edge_distance == 0 && safe_move_count <= 2 && enemy_pin_risk_milli >= 250 {
        combined = combined.max(850);
    }
    if head_edge_distance == 0 && safe_move_count <= 1 {
        combined = 1000;
    }
    if corner_contact && (safe_move_count <= 2 || enemy_pin_risk_milli >= 300) {
        combined = combined.max(900);
    }

    combined.try_into().unwrap_or(1000)
}

fn scale_by_fear(base: u32, fear_milli: u16) -> u16 {
    base.saturating_mul(u32::from(fear_milli))
        .saturating_div(1000)
        .min(1000)
        .try_into()
        .unwrap_or(1000)
}

fn edge_distance(state: &SimulatedGameState, coord: Coord) -> u16 {
    if coord.x < 0 || coord.y < 0 || coord.x >= state.width as i32 || coord.y >= state.height as i32
    {
        return 0;
    }

    let right = state.width as i32 - 1 - coord.x;
    let top = state.height as i32 - 1 - coord.y;
    coord
        .x
        .min(coord.y)
        .min(right)
        .min(top)
        .max(0)
        .try_into()
        .unwrap_or(0)
}

fn is_corner(state: &SimulatedGameState, coord: Coord) -> bool {
    let right = state.width as i32 - 1;
    let top = state.height as i32 - 1;
    (coord.x == 0 || coord.x == right) && (coord.y == 0 || coord.y == top)
}

#[cfg(test)]
mod tests {
    use crate::simulation::state::{RulesContext, SimulatedSnake};

    use super::*;

    fn snake(body: &[(i32, i32)]) -> SimulatedSnake {
        SimulatedSnake {
            id: "ours".to_string(),
            health: 100,
            body: body.iter().map(|(x, y)| Coord { x: *x, y: *y }).collect(),
            alive: true,
        }
    }

    fn state(body: &[(i32, i32)]) -> SimulatedGameState {
        SimulatedGameState {
            turn: 1,
            width: 11,
            height: 11,
            food: vec![],
            hazards: vec![],
            snakes: vec![snake(body)],
            our_snake_id: "ours".to_string(),
            rules: RulesContext {
                name: "standard".to_string(),
                max_health: 100,
                hazard_damage_per_turn: 0,
            },
        }
    }

    fn analyze(state: &SimulatedGameState) -> BorderFobicAnalysis {
        let mobility = MobilityAnalysis::from_state(state);
        let territory = TerritoryAnalysis::from_state(state);
        BorderFobicAnalysis::from_parts_with_territory_actor_relative(state, &mobility, &territory)
    }

    #[test]
    fn fear_grows_with_length() {
        let short = state(&[(0, 5), (0, 4), (0, 3), (0, 2)]);
        let long = state(&[
            (0, 5),
            (0, 4),
            (0, 3),
            (0, 2),
            (0, 1),
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
            (5, 0),
            (6, 0),
        ]);

        assert!(
            analyze(&long).ours().unwrap().fear_milli > analyze(&short).ours().unwrap().fear_milli
        );
    }

    #[test]
    fn hugging_edge_is_riskier_than_same_length_interior_body() {
        let edge = state(&[
            (0, 5),
            (0, 4),
            (0, 3),
            (0, 2),
            (0, 1),
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
        ]);
        let interior = state(&[
            (3, 5),
            (3, 4),
            (3, 3),
            (3, 2),
            (3, 1),
            (4, 1),
            (5, 1),
            (6, 1),
            (7, 1),
            (7, 2),
        ]);

        let edge_risk = analyze(&edge).ours().unwrap().structural_risk_milli;
        let interior_risk = analyze(&interior).ours().unwrap().structural_risk_milli;

        assert!(edge_risk > interior_risk);
    }

    #[test]
    fn enemy_controlled_inward_cells_raise_pin_risk() {
        let mut state = state(&[(0, 3), (0, 2), (0, 1), (0, 0), (1, 0), (2, 0)]);
        state.snakes.push(SimulatedSnake {
            id: "enemy".to_string(),
            health: 100,
            body: vec![
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
                Coord { x: 3, y: 4 },
                Coord { x: 4, y: 4 },
            ],
            alive: true,
        });

        let snapshot = *analyze(&state).ours().unwrap();

        assert!(snapshot.enemy_pin_risk_milli > 0);
        assert!(snapshot.inward_control_milli < 1000);
    }

    #[test]
    fn enemy_pin_with_single_inward_lane_creates_escape_emergency() {
        let mut state = state(&[(0, 3), (0, 2), (0, 1), (0, 0), (1, 0), (2, 0)]);
        state.snakes.push(SimulatedSnake {
            id: "enemy".to_string(),
            health: 100,
            body: vec![
                Coord { x: 2, y: 3 },
                Coord { x: 2, y: 4 },
                Coord { x: 3, y: 4 },
                Coord { x: 4, y: 4 },
            ],
            alive: true,
        });

        let snapshot = *analyze(&state).ours().unwrap();

        assert_eq!(snapshot.head_edge_distance, 0);
        assert!(snapshot.safe_move_count <= 2);
        assert!(snapshot.inward_safe_moves <= 1);
        assert!(snapshot.enemy_pin_risk_milli > 0);
        assert!(snapshot.escape_pressure_milli >= 850);
    }

    #[test]
    fn edge_without_enemy_pin_is_not_an_escape_emergency() {
        let state = state(&[(0, 5), (0, 4), (0, 3), (0, 2)]);
        let snapshot = *analyze(&state).ours().unwrap();

        assert_eq!(snapshot.head_edge_distance, 0);
        assert_eq!(snapshot.safe_move_count, 2);
        assert!(snapshot.enemy_pin_risk_milli < 250);
        assert!(snapshot.escape_pressure_milli < 800);
    }

    #[test]
    fn long_edge_chain_creates_structural_risk() {
        let state = state(&[
            (0, 5),
            (0, 4),
            (0, 3),
            (0, 2),
            (0, 1),
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 0),
        ]);
        let snapshot = *analyze(&state).ours().unwrap();

        assert!(snapshot.leading_edge_chain >= 5);
        assert!(snapshot.structural_risk_milli > 0);
    }

    #[test]
    fn border_analysis_is_available_for_enemy_perspective() {
        let mut state = state(&[(5, 5), (5, 4), (5, 3)]);
        state.snakes.push(SimulatedSnake {
            id: "enemy".to_string(),
            health: 100,
            body: vec![
                Coord { x: 0, y: 5 },
                Coord { x: 0, y: 4 },
                Coord { x: 0, y: 3 },
                Coord { x: 0, y: 2 },
                Coord { x: 0, y: 1 },
            ],
            alive: true,
        });

        let analysis = analyze(&state);
        let enemy = analysis.for_snake("enemy").unwrap();

        assert_eq!(enemy.head_edge_distance, 0);
        assert!(enemy.body_on_edge >= 4);
        assert!(enemy.structural_risk_milli > 0);
    }
}
