#![allow(dead_code)]

use crate::analysis::{
    EnclosureAnalysis, EnclosureRisk, StrategicPosture, TacticalStateAnalysis, TerritoryAnalysis,
};
use crate::direction::Direction;
use crate::enemy::tracing::EnemyTracingOutput;
use crate::simulation::state::SimulatedGameState;
use crate::Coord;

pub(crate) const EDGE_PIN_MIN_LENGTH: usize = 6;
pub(crate) const TERRITORY_HUNT_MIN_LENGTH: usize = 7;
pub(crate) const PARTIAL_WRAP_MIN_LENGTH: usize = 9;
pub(crate) const FULL_ENCLOSURE_SUPPORTED_MIN_LENGTH: usize = 12;
pub(crate) const FULL_ENCLOSURE_OPEN_MIN_LENGTH: usize = 14;
pub(crate) const STARVATION_SIEGE_MIN_LENGTH: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HuntingCandidate {
    pub(crate) target: String,
    pub(crate) first_move: Direction,
    pub(crate) target_moves_before: u8,
    pub(crate) contested_cell: Coord,
    pub(crate) length_advantage: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HuntingPlanKind {
    HeadPressure,
    EdgePin,
    ChokeCut,
    TerritorySqueeze,
    PartialWrap,
    FullEnclosure,
    StarvationSiege,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HuntingPlanCandidate {
    pub(crate) target: String,
    pub(crate) kind: HuntingPlanKind,
    pub(crate) score_milli: u16,
    pub(crate) length_advantage: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HuntingModeOutput {
    pub(crate) candidates: Vec<HuntingCandidate>,
    pub(crate) plans: Vec<HuntingPlanCandidate>,
}

impl HuntingModeOutput {
    pub(crate) fn best_plan_score(&self) -> f32 {
        self.plans
            .iter()
            .map(|plan| f32::from(plan.score_milli) / 1000.0)
            .reduce(f32::max)
            .unwrap_or(0.0)
    }
}

pub(crate) fn analyze(
    state: &SimulatedGameState,
    tactical: &TacticalStateAnalysis,
    tracing: &EnemyTracingOutput,
    territory: &TerritoryAnalysis,
    enclosure: &EnclosureAnalysis,
) -> HuntingModeOutput {
    let Some(ours) = state.snake(&state.our_snake_id).filter(|snake| snake.alive) else {
        return HuntingModeOutput::default();
    };
    let Some(our_head) = ours.head() else {
        return HuntingModeOutput::default();
    };

    let posture = StrategicPosture::from_state(state);
    let mut candidates = Vec::new();
    let mut plans = Vec::new();
    let board_cells = state.width.saturating_mul(state.height).max(1);

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
        let Some(enemy_territory) = territory.for_snake(&enemy.id) else {
            continue;
        };
        let enemy_competitive = territory.competitive_for_snake(&enemy.id);
        let our_competitive = territory.competitive_for_snake(&state.our_snake_id);
        let Some(enemy_enclosure) = enclosure.for_snake(&enemy.id) else {
            continue;
        };

        let length_advantage = ours.length() as i32 - enemy.length() as i32;

        if length_advantage > 0 {
            let candidates_before = candidates.len();
            collect_head_candidates(
                &mut candidates,
                &enemy.id,
                enemy_head,
                our_head,
                length_advantage,
                move_set,
                tactical,
            );
            let new_head_candidates = candidates.len().saturating_sub(candidates_before);
            if new_head_candidates > 0 {
                let dominance_bonus = our_competitive.map_or(0, |snapshot| {
                    snapshot
                        .favorable_head_frontier
                        .min(3)
                        .saturating_mul(60)
                        .saturating_add(
                            snapshot
                                .dominance_frontier_cells
                                .min(4)
                                .saturating_mul(25),
                        )
                });
                plans.push(HuntingPlanCandidate {
                    target: enemy.id.clone(),
                    kind: HuntingPlanKind::HeadPressure,
                    score_milli: (220
                        + length_advantage.max(0) as u16 * 35
                        + dominance_bonus)
                        .min(520),
                    length_advantage,
                });
            }
        }

        if ours.length() >= EDGE_PIN_MIN_LENGTH
            && length_advantage > 0
            && enemy_territory.edge_distance <= 2
        {
            let edge_bonus = match enemy_territory.edge_distance {
                0 => 140,
                1 => 90,
                _ => 40,
            };
            plans.push(HuntingPlanCandidate {
                target: enemy.id.clone(),
                kind: HuntingPlanKind::EdgePin,
                score_milli: (260 + edge_bonus + length_advantage as u16 * 20).min(520),
                length_advantage,
            });
        }

        if ours.length() >= TERRITORY_HUNT_MIN_LENGTH
            && (posture.hunt_drive_milli >= 200 || posture.unique_largest)
        {
            if let Some(choke) = enemy_territory
                .nearest_choke()
                .filter(|choke| choke.distance <= 6)
            {
                let gain_bonus = choke
                    .cut_gain
                    .saturating_mul(300)
                    .saturating_div(board_cells)
                    .min(260) as u16;
                plans.push(HuntingPlanCandidate {
                    target: enemy.id.clone(),
                    kind: HuntingPlanKind::ChokeCut,
                    score_milli: (320 + gain_bonus).min(650),
                    length_advantage,
                });
            }

            let competitive_pressure = match (our_competitive, enemy_competitive) {
                (Some(ours), Some(enemy)) => {
                    ours.control_ratio_milli > enemy.control_ratio_milli.saturating_add(50)
                        || enemy.control_ratio_milli <= 450
                }
                _ => false,
            };
            if enemy_enclosure.risk >= EnclosureRisk::Pressure
                || enemy_enclosure.space_to_length_milli <= 3000
                || competitive_pressure
            {
                let ratio_bonus = 3000_u32
                    .saturating_sub(enemy_enclosure.space_to_length_milli)
                    .saturating_div(8)
                    .min(280) as u16;
                let risk_bonus = u16::from(enemy_enclosure.risk.rank()) * 80;
                let control_bonus = enemy_competitive.map_or(0, |snapshot| {
                    1000_u16
                        .saturating_sub(snapshot.control_ratio_milli)
                        .saturating_div(3)
                });
                let frontier_bonus = our_competitive.map_or(0, |snapshot| {
                    snapshot
                        .winning_frontier
                        .saturating_sub(snapshot.losing_frontier)
                        .min(6)
                        .saturating_mul(20)
                });
                plans.push(HuntingPlanCandidate {
                    target: enemy.id.clone(),
                    kind: HuntingPlanKind::TerritorySqueeze,
                    score_milli: (260 + ratio_bonus + risk_bonus + control_bonus + frontier_bonus)
                        .min(880),
                    length_advantage,
                });
            }
        }

        if ours.length() >= PARTIAL_WRAP_MIN_LENGTH
            && length_advantage >= 1
            && enemy_enclosure.risk >= EnclosureRisk::Pressure
        {
            let boundary_bonus = u16::from(enemy_enclosure.boundary_support) * 60;
            plans.push(HuntingPlanCandidate {
                target: enemy.id.clone(),
                kind: HuntingPlanKind::PartialWrap,
                score_milli: (460 + boundary_bonus + u16::from(enemy_enclosure.risk.rank()) * 60)
                    .min(820),
                length_advantage,
            });
        }

        let supported_full_enclosure = ours.length() >= FULL_ENCLOSURE_SUPPORTED_MIN_LENGTH
            && enemy_enclosure.boundary_support > 0;
        let open_full_enclosure = ours.length() >= FULL_ENCLOSURE_OPEN_MIN_LENGTH;
        if (supported_full_enclosure || open_full_enclosure)
            && enemy_enclosure.risk >= EnclosureRisk::Constrained
        {
            plans.push(HuntingPlanCandidate {
                target: enemy.id.clone(),
                kind: HuntingPlanKind::FullEnclosure,
                score_milli: (690
                    + u16::from(enemy_enclosure.risk.rank()) * 70
                    + u16::from(enemy_enclosure.boundary_support) * 45)
                    .min(960),
                length_advantage,
            });
        }

        if ours.length() >= STARVATION_SIEGE_MIN_LENGTH
            && enemy.health <= 25
            && enemy_enclosure.risk >= EnclosureRisk::Pressure
        {
            plans.push(HuntingPlanCandidate {
                target: enemy.id.clone(),
                kind: HuntingPlanKind::StarvationSiege,
                score_milli: (520 + (25 - enemy.health).max(0) as u16 * 8).min(820),
                length_advantage,
            });
        }
    }

    candidates.sort_by(|left, right| {
        left.target
            .cmp(&right.target)
            .then_with(|| left.first_move.rank().cmp(&right.first_move.rank()))
            .then_with(|| left.contested_cell.cmp(&right.contested_cell))
    });
    candidates.dedup();

    plans.sort_by(|left, right| {
        right
            .score_milli
            .cmp(&left.score_milli)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.target.cmp(&right.target))
    });
    plans.dedup_by(|left, right| left.target == right.target && left.kind == right.kind);

    HuntingModeOutput { candidates, plans }
}

fn collect_head_candidates(
    candidates: &mut Vec<HuntingCandidate>,
    enemy_id: &str,
    enemy_head: Coord,
    our_head: Coord,
    length_advantage: i32,
    move_set: &crate::enemy::tracing::EnemyMoveSet,
    tactical: &TacticalStateAnalysis,
) {
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
                target: enemy_id.to_string(),
                first_move: our_direction,
                target_moves_before: move_set.plausible_moves.len(),
                contested_cell,
                length_advantage,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::{EnclosureAnalysis, StateAnalysis, TerritoryAnalysis};
    use crate::enemy::tracing::trace;
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

    fn state(snakes: Vec<SimulatedSnake>, aggression: f32) -> SimulatedGameState {
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
            aggression: AggressionState {
                fruits_eaten: 4,
                value: aggression,
            },
        }
    }

    fn analyze_state(state: &SimulatedGameState) -> HuntingModeOutput {
        let analysis = StateAnalysis::from_simulated(state);
        let tracing = trace(state, &analysis);
        let tactical = TacticalStateAnalysis::from_state(state, &tracing);
        let territory = TerritoryAnalysis::from_state(state);
        let enclosure = EnclosureAnalysis::from_parts(state, &territory, &tactical);
        analyze(state, &tactical, &tracing, &territory, &enclosure)
    }

    #[test]
    fn favorable_contested_cell_creates_hunting_candidate() {
        let state = state(
            vec![
                snake("ours", &[(2, 1), (1, 1), (1, 0), (0, 0)]),
                snake("enemy", &[(2, 3), (3, 3)]),
            ],
            0.2,
        );

        let output = analyze_state(&state);

        assert!(output.candidates.iter().any(|candidate| {
            candidate.target == "enemy"
                && candidate.first_move == Direction::Up
                && candidate.contested_cell == Coord { x: 2, y: 2 }
        }));
    }

    #[test]
    fn equal_length_contest_is_not_offensive_candidate() {
        let state = state(
            vec![
                snake("ours", &[(2, 1), (1, 1), (1, 0)]),
                snake("enemy", &[(2, 3), (3, 3), (3, 2)]),
            ],
            0.2,
        );

        assert!(analyze_state(&state).candidates.is_empty());
    }

    #[test]
    fn territorial_hunting_starts_at_length_seven() {
        let short = state(
            vec![
                snake("ours", &[(1, 1), (1, 0), (0, 0), (0, 1), (0, 2), (1, 2)]),
                snake("enemy", &[(5, 5), (5, 4), (5, 3)]),
            ],
            0.2,
        );
        let long = state(
            vec![
                snake(
                    "ours",
                    &[(1, 1), (1, 0), (0, 0), (0, 1), (0, 2), (1, 2), (2, 2)],
                ),
                snake("enemy", &[(5, 5), (5, 4), (5, 3)]),
            ],
            0.2,
        );

        assert!(!analyze_state(&short).plans.iter().any(|plan| matches!(
            plan.kind,
            HuntingPlanKind::ChokeCut | HuntingPlanKind::TerritorySqueeze
        )));
        assert!(analyze_state(&long).plans.iter().any(|plan| matches!(
            plan.kind,
            HuntingPlanKind::HeadPressure
                | HuntingPlanKind::EdgePin
                | HuntingPlanKind::ChokeCut
                | HuntingPlanKind::TerritorySqueeze
        )));
    }

    #[test]
    fn competitive_control_strengthens_territory_squeeze() {
        let state = state(
            vec![
                snake(
                    "ours",
                    &[
                        (1, 3),
                        (1, 2),
                        (1, 1),
                        (0, 1),
                        (0, 2),
                        (0, 3),
                        (0, 4),
                        (1, 4),
                    ],
                ),
                snake("enemy", &[(5, 3), (5, 2), (5, 1)]),
            ],
            0.5,
        );

        let output = analyze_state(&state);
        assert!(output.plans.iter().any(|plan| {
            plan.kind == HuntingPlanKind::TerritorySqueeze && plan.score_milli >= 400
        }));
    }

    #[test]
    fn full_enclosure_requires_large_body() {
        let medium = state(
            vec![
                snake(
                    "ours",
                    &[
                        (1, 1),
                        (1, 0),
                        (0, 0),
                        (0, 1),
                        (0, 2),
                        (0, 3),
                        (0, 4),
                        (1, 4),
                        (2, 4),
                        (2, 3),
                        (2, 2),
                    ],
                ),
                snake("enemy", &[(6, 1), (6, 0), (5, 0)]),
            ],
            0.5,
        );

        assert!(!analyze_state(&medium)
            .plans
            .iter()
            .any(|plan| plan.kind == HuntingPlanKind::FullEnclosure));
    }
}
