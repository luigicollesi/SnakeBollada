use crate::decision::evaluation::{compare_direction, DirectionEvaluation};
use crate::decision::policy::ReservedCellPolicy;
use crate::direction::MoveMask;
use crate::simulation::state::SimulatedGameState;

pub(crate) const ESCAPE_ACTIVATION_THRESHOLD_MILLI: u16 = 650;
pub(crate) const ESCAPE_RELEASE_THRESHOLD_MILLI: u16 = 300;

pub(crate) fn direction_escape_pressure_milli(evaluation: &DirectionEvaluation) -> u16 {
    if evaluation.survival.is_forced_death() {
        return 1000;
    }
    if evaluation.survival.is_forced_dead_end() {
        return 950;
    }

    let death = u32::from(evaluation.survival.death_rate_milli())
        .saturating_mul(35)
        .saturating_div(100);
    let dead_end = u32::from(evaluation.survival.dead_end_rate_milli())
        .saturating_mul(25)
        .saturating_div(100);
    let forced = u32::from(evaluation.survival.forced_rate_milli())
        .saturating_mul(15)
        .saturating_div(100);
    let constrained = u32::from(evaluation.survival.constrained_rate_milli())
        .saturating_mul(10)
        .saturating_div(100);
    let pin = u32::from(evaluation.survival.max_enemy_pin_risk_milli)
        .saturating_mul(10)
        .saturating_div(100);
    let enclosure = u32::from(evaluation.survival.max_self_enclosure_risk).saturating_mul(50);
    let mobility = match evaluation.survival.min_future_mobility {
        0 => 200,
        1 => 120,
        2 => 40,
        _ => 0,
    };

    death
        .saturating_add(dead_end)
        .saturating_add(forced)
        .saturating_add(constrained)
        .saturating_add(pin)
        .saturating_add(enclosure)
        .saturating_add(mobility)
        .min(1000)
        .try_into()
        .unwrap_or(1000)
}

pub(crate) fn root_escape_pressure_milli(
    evaluations: &[DirectionEvaluation],
    robust_safe_moves: MoveMask,
) -> u16 {
    let considered = evaluations
        .iter()
        .filter(|evaluation| {
            robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction)
        })
        .collect::<Vec<_>>();

    if considered.is_empty() {
        return 1000;
    }

    let mut pressures = considered
        .iter()
        .map(|evaluation| direction_escape_pressure_milli(evaluation))
        .collect::<Vec<_>>();
    pressures.sort_unstable();

    let best = u32::from(*pressures.first().unwrap_or(&1000));
    let average = pressures
        .iter()
        .fold(0_u32, |sum, pressure| {
            sum.saturating_add(u32::from(*pressure))
        })
        .saturating_div(pressures.len().try_into().unwrap_or(1));
    let bad = pressures
        .iter()
        .filter(|pressure| **pressure >= 550)
        .count();

    let mut pressure = best
        .saturating_mul(60)
        .saturating_div(100)
        .saturating_add(average.saturating_mul(40).saturating_div(100));

    if bad >= 2 {
        pressure = pressure.saturating_add(100);
    }
    if !robust_safe_moves.is_empty() && robust_safe_moves.len() <= 1 {
        pressure = pressure.max(650);
    }

    pressure.min(1000).try_into().unwrap_or(1000)
}

pub(crate) fn choose_escape_direction<'a>(
    evaluations: &'a [DirectionEvaluation],
    state: &SimulatedGameState,
    robust_safe_moves: MoveMask,
    policy: ReservedCellPolicy,
) -> Option<&'a DirectionEvaluation> {
    evaluations
        .iter()
        .filter(|evaluation| {
            robust_safe_moves.is_empty() || robust_safe_moves.contains(evaluation.direction)
        })
        .min_by(|left, right| {
            direction_escape_pressure_milli(left)
                .cmp(&direction_escape_pressure_milli(right))
                .then_with(|| compare_direction(left, right, state, policy))
        })
}

#[cfg(test)]
mod tests {
    use crate::decision::evaluation::{DirectionSurvivalSummary, TerminalAssessment};
    use crate::direction::Direction;

    use super::*;

    fn evaluation(
        direction: Direction,
        death: u64,
        dead_end: u64,
        forced: u64,
    ) -> DirectionEvaluation {
        DirectionEvaluation {
            direction,
            terminal: TerminalAssessment::Running,
            survival: DirectionSurvivalSummary {
                total_routes: 10,
                death_routes: death,
                dead_end_routes: dead_end,
                forced_routes: forced,
                constrained_routes: 0,
                border_exposure_ticks: 0,
                corner_exposure_ticks: 0,
                cumulative_border_cost_milli: 0,
                max_self_enclosure_risk: 0,
                max_border_structural_risk_milli: 0,
                max_border_preference_milli: 0,
                max_enemy_pin_risk_milli: 0,
                min_inward_control_milli: 1000,
                min_future_mobility: 3,
                min_reachable_space: 20,
                min_second_order_mobility: 3,
            },
            worst_strategic_utility: 0.0,
            average_strategic_utility: 0.0,
            average_food_value: 0.0,
            average_hunting_value: 0.0,
            average_leaf_food_potential: 0.0,
            average_leaf_hunting_potential: 0.0,
            guaranteed_enemy_kills: 0,
            reserved_override: false,
        }
    }

    #[test]
    fn death_and_dead_end_heavily_raise_escape_pressure() {
        let safe = evaluation(Direction::Up, 0, 0, 0);
        let pressured = evaluation(Direction::Right, 6, 5, 4);

        assert!(
            direction_escape_pressure_milli(&pressured) > direction_escape_pressure_milli(&safe)
        );
    }

    #[test]
    fn single_robust_exit_activates_escape_pressure() {
        let evaluations = [
            evaluation(Direction::Up, 0, 0, 0),
            evaluation(Direction::Right, 8, 8, 8),
        ];

        assert!(
            root_escape_pressure_milli(&evaluations, MoveMask::single(Direction::Up))
                >= ESCAPE_ACTIVATION_THRESHOLD_MILLI
        );
    }
}
