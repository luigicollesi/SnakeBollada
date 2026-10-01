#![allow(dead_code)]

use crate::analysis::TacticalStateAnalysis;
use crate::direction::Direction;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BorderFobicSnapshot {
    pub(crate) fear_milli: u16,
    pub(crate) head_edge_distance: u16,
    pub(crate) body_on_edge: u16,
    pub(crate) body_near_edge: u16,
    pub(crate) leading_edge_chain: u16,
    pub(crate) inward_safe_moves: u8,
    pub(crate) corner_contact: bool,
    pub(crate) preference_milli: u16,
    pub(crate) structural_risk_milli: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct BorderFobicAnalysis {
    ours: Option<BorderFobicSnapshot>,
}

impl BorderFobicAnalysis {
    pub(crate) fn from_parts(state: &SimulatedGameState, tactical: &TacticalStateAnalysis) -> Self {
        let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
            return Self::default();
        };
        let Some(head) = ours.head() else {
            return Self::default();
        };

        let fear_milli = length_fear_milli(state, ours.length());
        let head_edge_distance = edge_distance(state, head);
        let body_on_edge = ours
            .body
            .iter()
            .filter(|segment| edge_distance(state, **segment) == 0)
            .count()
            .try_into()
            .unwrap_or(u16::MAX);
        let body_near_edge = ours
            .body
            .iter()
            .filter(|segment| edge_distance(state, **segment) <= 1)
            .count()
            .try_into()
            .unwrap_or(u16::MAX);
        let leading_edge_chain = ours
            .body
            .iter()
            .take_while(|segment| edge_distance(state, **segment) == 0)
            .count()
            .try_into()
            .unwrap_or(u16::MAX);
        let inward_safe_moves = tactical
            .ours
            .safe_moves
            .iter()
            .filter(|direction| edge_distance(state, direction.apply(head)) > head_edge_distance)
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

        let length = u32::try_from(ours.length()).unwrap_or(u32::MAX).max(1);
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

        Self {
            ours: Some(BorderFobicSnapshot {
                fear_milli,
                head_edge_distance,
                body_on_edge,
                body_near_edge,
                leading_edge_chain,
                inward_safe_moves,
                corner_contact,
                preference_milli,
                structural_risk_milli,
            }),
        }
    }

    pub(crate) fn ours(&self) -> Option<&BorderFobicSnapshot> {
        self.ours.as_ref()
    }

    pub(crate) fn move_preference_milli(
        &self,
        state: &SimulatedGameState,
        direction: Direction,
    ) -> u16 {
        let Some(snapshot) = self.ours else {
            return 0;
        };
        let Some(head) = state
            .snake(&state.our_snake_id)
            .and_then(|snake| snake.head())
        else {
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
    use crate::analysis::StateAnalysis;
    use crate::enemy::tracing::trace;
    use crate::simulation::state::{AggressionState, RulesContext, SimulatedSnake};

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
            aggression: AggressionState::default(),
        }
    }

    fn analyze(state: &SimulatedGameState) -> BorderFobicAnalysis {
        let state_analysis = StateAnalysis::from_simulated(state);
        let tracing = trace(state, &state_analysis);
        let tactical = TacticalStateAnalysis::from_state(state, &tracing);
        BorderFobicAnalysis::from_parts(state, &tactical)
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
}
