//! Opt-in conservative root move ordering; never changes Minimax scores.
//!
//! The baseline order stays the stable tie-breaker for uncertain, partially
//! explored, or food-conditional predictions. A proven losing root action
//! can be deprioritized even when other root actions remain unknown.
//! Territorial margins are *heuristic* selected-path tie-breakers only when
//! all directions are deterministically safe at the same completed horizon.

use std::collections::HashMap;

use crate::analysis::AdversarialEscapeVerdict;
use crate::direction::Direction;
use crate::search::forecast::ForecastCertainty;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RootV2Hint {
    pub(crate) verdict: AdversarialEscapeVerdict,
    pub(crate) certainty: Option<ForecastCertainty>,
    /// The same common horizon is required for every compared direction.
    pub(crate) horizon: u8,
    /// Set only for a path from an exhaustive same-depth MAX/MIN search.
    /// These margins are not adversarial territorial lower bounds.
    pub(crate) comparable_margin: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SafetyClass {
    Verified,
    Unknown,
    ForcedLoss,
}

impl SafetyClass {
    fn of(hint: RootV2Hint, common_horizon: u8) -> Self {
        if hint.horizon != common_horizon
            || hint.certainty != Some(ForecastCertainty::Deterministic)
        {
            return Self::Unknown;
        }
        match hint.verdict {
            AdversarialEscapeVerdict::SurvivesHorizon => Self::Verified,
            AdversarialEscapeVerdict::ForcedLossWithinHorizon => Self::ForcedLoss,
            AdversarialEscapeVerdict::Unknown => Self::Unknown,
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Verified => 0,
            Self::Unknown => 1,
            Self::ForcedLoss => 2,
        }
    }
}

/// Preserve prior baseline order within each certainty class. Never compare
/// margins sampled from different horizons, partial MIN replies or food
/// scenarios. The inputs are already ordered by the canonical baseline.
pub(crate) fn reorder_root_v2(
    directions: &mut [Direction],
    hints: &HashMap<Direction, RootV2Hint>,
    common_horizon: u8,
) -> bool {
    if directions.len() < 2
        || common_horizon == 0
        || !directions
            .iter()
            .all(|direction| hints.contains_key(direction))
    {
        return false;
    }
    let safe_territory_tiebreak = directions.iter().all(|direction| {
        let hint = hints[direction];
        SafetyClass::of(hint, common_horizon) == SafetyClass::Verified
            && hint.comparable_margin.is_some()
    });

    let original = directions.to_vec();
    // Stable sort deliberately retains canonical baseline ordering for ties.
    directions.sort_by(|left, right| {
        let left_hint = hints[left];
        let right_hint = hints[right];
        let classes = SafetyClass::of(left_hint, common_horizon)
            .priority()
            .cmp(&SafetyClass::of(right_hint, common_horizon).priority());
        if safe_territory_tiebreak {
            classes.then_with(|| {
                right_hint
                    .comparable_margin
                    .cmp(&left_hint.comparable_margin)
            })
        } else {
            classes
        }
    });
    directions != original
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint(
        verdict: AdversarialEscapeVerdict,
        certainty: Option<ForecastCertainty>,
        horizon: u8,
        margin: Option<i64>,
    ) -> RootV2Hint {
        RootV2Hint {
            verdict,
            certainty,
            horizon,
            comparable_margin: margin,
        }
    }

    #[test]
    fn all_unknown_is_exact_baseline_fallback() {
        let mut directions = vec![Direction::Left, Direction::Up, Direction::Right];
        let before = directions.clone();
        let hints = HashMap::from([
            (
                Direction::Left,
                hint(AdversarialEscapeVerdict::Unknown, None, 3, None),
            ),
            (
                Direction::Up,
                hint(AdversarialEscapeVerdict::Unknown, None, 3, None),
            ),
            (
                Direction::Right,
                hint(AdversarialEscapeVerdict::Unknown, None, 3, None),
            ),
        ]);
        assert!(!reorder_root_v2(&mut directions, &hints, 3));
        assert_eq!(directions, before);
    }

    #[test]
    fn proven_loss_is_deprioritized_without_punishing_unknown_routes() {
        let mut directions = vec![Direction::Left, Direction::Up, Direction::Right];
        let hints = HashMap::from([
            (
                Direction::Left,
                hint(
                    AdversarialEscapeVerdict::ForcedLossWithinHorizon,
                    Some(ForecastCertainty::Deterministic),
                    2,
                    None,
                ),
            ),
            (
                Direction::Up,
                hint(AdversarialEscapeVerdict::Unknown, None, 2, None),
            ),
            (
                Direction::Right,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::Deterministic),
                    2,
                    None,
                ),
            ),
        ]);
        assert!(reorder_root_v2(&mut directions, &hints, 2));
        assert_eq!(
            directions,
            vec![Direction::Right, Direction::Up, Direction::Left]
        );
    }

    #[test]
    fn provisional_food_never_supplies_a_proven_priority() {
        let mut directions = vec![Direction::Left, Direction::Right];
        let hints = HashMap::from([
            (
                Direction::Left,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::FoodProvisional),
                    2,
                    Some(999),
                ),
            ),
            (
                Direction::Right,
                hint(AdversarialEscapeVerdict::Unknown, None, 2, None),
            ),
        ]);
        assert!(!reorder_root_v2(&mut directions, &hints, 2));
        assert_eq!(directions, vec![Direction::Left, Direction::Right]);
    }

    #[test]
    fn mixed_horizons_cannot_promote_incomparable_evidence() {
        let mut directions = vec![Direction::Left, Direction::Right];
        let hints = HashMap::from([
            (
                Direction::Left,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::Deterministic),
                    2,
                    Some(10),
                ),
            ),
            (
                Direction::Right,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::Deterministic),
                    3,
                    Some(100),
                ),
            ),
        ]);
        assert!(!reorder_root_v2(&mut directions, &hints, 2));
        assert_eq!(directions, vec![Direction::Left, Direction::Right]);
    }

    #[test]
    fn same_horizon_certified_safe_paths_can_use_margin_as_tiebreak() {
        let mut directions = vec![Direction::Left, Direction::Right];
        let hints = HashMap::from([
            (
                Direction::Left,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::Deterministic),
                    2,
                    Some(-200),
                ),
            ),
            (
                Direction::Right,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::Deterministic),
                    2,
                    Some(-100),
                ),
            ),
        ]);
        assert!(reorder_root_v2(&mut directions, &hints, 2));
        assert_eq!(directions, vec![Direction::Right, Direction::Left]);
    }

    #[test]
    fn partial_margin_sample_never_outranks_baseline() {
        let mut directions = vec![Direction::Left, Direction::Right];
        let hints = HashMap::from([
            (
                Direction::Left,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::Deterministic),
                    2,
                    None,
                ),
            ),
            (
                Direction::Right,
                hint(
                    AdversarialEscapeVerdict::SurvivesHorizon,
                    Some(ForecastCertainty::Deterministic),
                    2,
                    Some(999),
                ),
            ),
        ]);
        assert!(!reorder_root_v2(&mut directions, &hints, 2));
    }

    #[test]
    fn absent_evidence_is_full_fallback_not_a_partial_sort() {
        let mut directions = vec![Direction::Left, Direction::Right];
        let mut hints = HashMap::new();
        hints.insert(
            Direction::Left,
            hint(
                AdversarialEscapeVerdict::ForcedLossWithinHorizon,
                Some(ForecastCertainty::Deterministic),
                1,
                None,
            ),
        );
        assert!(!reorder_root_v2(&mut directions, &hints, 1));
        assert_eq!(directions, vec![Direction::Left, Direction::Right]);
    }
}
