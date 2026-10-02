use std::collections::HashMap;

use crate::direction::Direction;

use super::tracing::{EnemyMoveSet, OpponentMoveHypothesis};

const BASE_BIAS_MILLI: u16 = 1000;
const MIN_BIAS_MILLI: u16 = 700;
const MAX_BIAS_MILLI: u16 = 1300;
const SUPPORTED_REWARD: u16 = 40;
const UNSUPPORTED_DECAY: u16 = 15;
const HEAD_THREAT_REWARD: u16 = 50;

pub(crate) type OpponentProfiles = HashMap<String, OpponentProfile>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpponentProfile {
    pub(crate) food_bias_milli: u16,
    pub(crate) hunting_bias_milli: u16,
    pub(crate) head_threat_bias_milli: u16,
    pub(crate) observations: u16,
    pub(crate) unexpected_moves: u16,
}

impl Default for OpponentProfile {
    fn default() -> Self {
        Self {
            food_bias_milli: BASE_BIAS_MILLI,
            hunting_bias_milli: BASE_BIAS_MILLI,
            head_threat_bias_milli: BASE_BIAS_MILLI,
            observations: 0,
            unexpected_moves: 0,
        }
    }
}

impl OpponentProfile {
    pub(crate) fn observe(&mut self, moves: &EnemyMoveSet, observed: Direction) {
        self.observations = self.observations.saturating_add(1);

        let Some(hypothesis) = moves.hypothesis(observed) else {
            self.unexpected_moves = self.unexpected_moves.saturating_add(1);
            return;
        };

        let mut alternatives = moves.hypotheses.iter().copied();

        self.food_bias_milli = update_bias(
            self.food_bias_milli,
            hypothesis.support.food,
            alternatives.clone().any(|candidate| candidate.support.food),
            SUPPORTED_REWARD,
        );
        self.hunting_bias_milli = update_bias(
            self.hunting_bias_milli,
            hypothesis.support.hunting,
            alternatives
                .clone()
                .any(|candidate| candidate.support.hunting),
            SUPPORTED_REWARD,
        );
        self.head_threat_bias_milli = update_bias(
            self.head_threat_bias_milli,
            hypothesis.support.head_threat,
            alternatives.any(|candidate| candidate.support.head_threat),
            HEAD_THREAT_REWARD,
        );
    }

    pub(crate) fn adjusted_plausibility(&self, hypothesis: OpponentMoveHypothesis) -> u16 {
        let mut total = 0_u32;
        let mut count = 0_u32;

        if hypothesis.support.food {
            total = total.saturating_add(u32::from(self.food_bias_milli));
            count = count.saturating_add(1);
        }
        if hypothesis.support.hunting {
            total = total.saturating_add(u32::from(self.hunting_bias_milli));
            count = count.saturating_add(1);
        }
        if hypothesis.support.head_threat {
            total = total.saturating_add(u32::from(self.head_threat_bias_milli));
            count = count.saturating_add(1);
        }

        if count == 0 {
            return hypothesis.plausibility_milli;
        }

        let multiplier = total.saturating_div(count);
        u32::from(hypothesis.plausibility_milli)
            .saturating_mul(multiplier)
            .saturating_div(1000)
            .min(1000)
            .try_into()
            .unwrap_or(1000)
    }
}

fn update_bias(current: u16, selected: bool, available: bool, reward: u16) -> u16 {
    if selected {
        current.saturating_add(reward).min(MAX_BIAS_MILLI)
    } else if available {
        current
            .saturating_sub(UNSUPPORTED_DECAY)
            .max(MIN_BIAS_MILLI)
    } else {
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::direction::MoveMask;
    use crate::enemy::tracing::{OpponentPolicySupport, ThreatClass};

    fn set() -> EnemyMoveSet {
        EnemyMoveSet {
            legal_moves: MoveMask::from_iter([Direction::Left, Direction::Down]),
            hypotheses: vec![
                OpponentMoveHypothesis {
                    direction: Direction::Left,
                    support: OpponentPolicySupport {
                        food: true,
                        ..OpponentPolicySupport::default()
                    },
                    threat: ThreatClass::None,
                    plausibility_milli: 600,
                },
                OpponentMoveHypothesis {
                    direction: Direction::Down,
                    support: OpponentPolicySupport {
                        hunting: true,
                        head_threat: true,
                        ..OpponentPolicySupport::default()
                    },
                    threat: ThreatClass::Likely,
                    plausibility_milli: 600,
                },
            ],
        }
    }

    #[test]
    fn observing_food_abandonment_shifts_bias_toward_hunting() {
        let moves = set();
        let mut profile = OpponentProfile::default();

        profile.observe(&moves, Direction::Down);

        assert!(profile.hunting_bias_milli > BASE_BIAS_MILLI);
        assert!(profile.head_threat_bias_milli > BASE_BIAS_MILLI);
        assert!(profile.food_bias_milli < BASE_BIAS_MILLI);
    }

    #[test]
    fn profile_changes_order_weight_without_removing_hypothesis() {
        let moves = set();
        let mut profile = OpponentProfile::default();
        for _ in 0..4 {
            profile.observe(&moves, Direction::Down);
        }

        let food = moves.hypothesis(Direction::Left).unwrap();
        let hunt = moves.hypothesis(Direction::Down).unwrap();

        assert!(profile.adjusted_plausibility(hunt) > profile.adjusted_plausibility(food));
        assert_eq!(moves.hypotheses.len(), 2);
    }
}
