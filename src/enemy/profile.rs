use std::collections::HashMap;

use crate::direction::Direction;

use super::intent::{IntentContrast, IntentInference, IntentSkipReason};
use super::tracing::{EnemyMoveSet, OpponentMoveHypothesis};

const BASE_BIAS_MILLI: u16 = 1000;
const MIN_BIAS_MILLI: u16 = 700;
const MAX_BIAS_MILLI: u16 = 1300;
const SUPPORTED_REWARD: u16 = 40;
const UNSUPPORTED_DECAY: u16 = 15;
const HEAD_THREAT_REWARD: u16 = 50;
const MAX_INTENT_STEP: i32 = 50;

pub(crate) type OpponentProfiles = HashMap<String, OpponentProfile>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OpponentIntentStats {
    pub(crate) learned_observations: u16,
    pub(crate) skipped_forced: u16,
    pub(crate) skipped_low_contrast: u16,
    pub(crate) skipped_incomplete_root: u16,
    pub(crate) skipped_insufficient_data: u16,
    pub(crate) downweighted_survival_emergency: u16,
    pub(crate) last_observed: Option<Direction>,
    pub(crate) last_food_contrast: i16,
    pub(crate) last_hunting_contrast: i16,
    pub(crate) last_trapping_contrast: i16,
    pub(crate) last_information_milli: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpponentProfile {
    pub(crate) food_bias_milli: u16,
    pub(crate) hunting_bias_milli: u16,
    pub(crate) trapping_bias_milli: u16,
    pub(crate) head_threat_bias_milli: u16,
    pub(crate) observations: u16,
    pub(crate) unexpected_moves: u16,
    pub(crate) intent_stats: OpponentIntentStats,
}

impl Default for OpponentProfile {
    fn default() -> Self {
        Self {
            food_bias_milli: BASE_BIAS_MILLI,
            hunting_bias_milli: BASE_BIAS_MILLI,
            trapping_bias_milli: BASE_BIAS_MILLI,
            head_threat_bias_milli: BASE_BIAS_MILLI,
            observations: 0,
            unexpected_moves: 0,
            intent_stats: OpponentIntentStats::default(),
        }
    }
}

impl OpponentProfile {
    pub(crate) fn record_intent_inference(
        &mut self,
        observed: Direction,
        inference: IntentInference,
    ) {
        self.intent_stats.last_observed = Some(observed);
        match inference {
            IntentInference::Learned {
                contrast,
                survival_emergency,
            } => {
                self.intent_stats.learned_observations =
                    self.intent_stats.learned_observations.saturating_add(1);
                if survival_emergency {
                    self.intent_stats.downweighted_survival_emergency = self
                        .intent_stats
                        .downweighted_survival_emergency
                        .saturating_add(1);
                }
                self.intent_stats.last_food_contrast = contrast.food_milli;
                self.intent_stats.last_hunting_contrast = contrast.hunting_milli;
                self.intent_stats.last_trapping_contrast = contrast.trapping_milli;
                self.intent_stats.last_information_milli = contrast.information_milli;
            }
            IntentInference::Skipped(reason) => match reason {
                IntentSkipReason::ForcedMove => {
                    self.intent_stats.skipped_forced =
                        self.intent_stats.skipped_forced.saturating_add(1);
                }
                IntentSkipReason::LowContrast => {
                    self.intent_stats.skipped_low_contrast =
                        self.intent_stats.skipped_low_contrast.saturating_add(1);
                }
                IntentSkipReason::IncompleteRoot => {
                    self.intent_stats.skipped_incomplete_root =
                        self.intent_stats.skipped_incomplete_root.saturating_add(1);
                }
                IntentSkipReason::InsufficientData => {
                    self.intent_stats.skipped_insufficient_data = self
                        .intent_stats
                        .skipped_insufficient_data
                        .saturating_add(1);
                }
            },
        }
    }

    pub(crate) fn observe_with_intent(
        &mut self,
        moves: &EnemyMoveSet,
        observed: Direction,
        intent: Option<IntentContrast>,
    ) {
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

        if let Some(intent) = intent {
            self.food_bias_milli = apply_intent_contrast(
                self.food_bias_milli,
                intent.food_milli,
                intent.information_milli,
            );
            self.hunting_bias_milli = apply_intent_contrast(
                self.hunting_bias_milli,
                intent.hunting_milli,
                intent.information_milli,
            );
            self.trapping_bias_milli = apply_intent_contrast(
                self.trapping_bias_milli,
                intent.trapping_milli,
                intent.information_milli,
            );
        }
    }

    pub(crate) fn adjusted_plausibility(&self, hypothesis: OpponentMoveHypothesis) -> u16 {
        let mut weighted_bias = 0_u32;
        let mut support_weight = 0_u32;

        if hypothesis.support.food {
            weighted_bias = weighted_bias.saturating_add(u32::from(self.food_bias_milli) * 1000);
            support_weight = support_weight.saturating_add(1000);
        }
        if hypothesis.support.hunting {
            weighted_bias = weighted_bias.saturating_add(u32::from(self.hunting_bias_milli) * 1000);
            support_weight = support_weight.saturating_add(1000);
        }
        if hypothesis.support.head_threat {
            weighted_bias =
                weighted_bias.saturating_add(u32::from(self.head_threat_bias_milli) * 1000);
            support_weight = support_weight.saturating_add(1000);
        }
        if hypothesis.support.trapping_milli > 0 {
            let trapping_weight = u32::from(hypothesis.support.trapping_milli);
            weighted_bias =
                weighted_bias.saturating_add(u32::from(self.trapping_bias_milli) * trapping_weight);
            support_weight = support_weight.saturating_add(trapping_weight);
        }

        if support_weight == 0 {
            return hypothesis.plausibility_milli;
        }

        let multiplier = weighted_bias.saturating_div(support_weight);
        u32::from(hypothesis.plausibility_milli)
            .saturating_mul(multiplier)
            .saturating_div(1000)
            .min(1000)
            .try_into()
            .unwrap_or(1000)
    }
}

fn apply_intent_contrast(current: u16, contrast: i16, information_milli: u16) -> u16 {
    let step = i32::from(contrast)
        .saturating_mul(MAX_INTENT_STEP)
        .saturating_mul(i32::from(information_milli))
        .saturating_div(1_000_000);
    i32::from(current)
        .saturating_add(step)
        .clamp(i32::from(MIN_BIAS_MILLI), i32::from(MAX_BIAS_MILLI))
        .try_into()
        .unwrap_or(if step < 0 {
            MIN_BIAS_MILLI
        } else {
            MAX_BIAS_MILLI
        })
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

        profile.observe_with_intent(&moves, Direction::Down, None);

        assert!(profile.hunting_bias_milli > BASE_BIAS_MILLI);
        assert!(profile.head_threat_bias_milli > BASE_BIAS_MILLI);
        assert!(profile.food_bias_milli < BASE_BIAS_MILLI);
    }

    #[test]
    fn contrastive_trapping_observation_updates_profile_gradually() {
        let moves = set();
        let mut profile = OpponentProfile::default();

        profile.observe_with_intent(
            &moves,
            Direction::Down,
            Some(IntentContrast {
                food_milli: -800,
                hunting_milli: 300,
                trapping_milli: 850,
                information_milli: 850,
            }),
        );

        assert!(profile.trapping_bias_milli > BASE_BIAS_MILLI);
        assert!(profile.food_bias_milli < BASE_BIAS_MILLI);
        assert!(profile.trapping_bias_milli <= BASE_BIAS_MILLI + MAX_INTENT_STEP as u16);
    }

    #[test]
    fn profile_changes_order_weight_without_removing_hypothesis() {
        let moves = set();
        let mut profile = OpponentProfile::default();
        for _ in 0..4 {
            profile.observe_with_intent(&moves, Direction::Down, None);
        }

        let food = moves.hypothesis(Direction::Left).unwrap();
        let hunt = moves.hypothesis(Direction::Down).unwrap();

        assert!(profile.adjusted_plausibility(hunt) > profile.adjusted_plausibility(food));
        assert_eq!(moves.hypotheses.len(), 2);
    }

    #[test]
    fn repeated_trapping_intent_reorders_future_move_ahead_of_food() {
        let moves = EnemyMoveSet {
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
                        trapping_milli: 900,
                        ..OpponentPolicySupport::default()
                    },
                    threat: ThreatClass::None,
                    plausibility_milli: 600,
                },
            ],
        };
        let mut profile = OpponentProfile::default();

        for _ in 0..3 {
            profile.observe_with_intent(
                &moves,
                Direction::Down,
                Some(IntentContrast {
                    food_milli: -800,
                    hunting_milli: 100,
                    trapping_milli: 850,
                    information_milli: 850,
                }),
            );
        }

        assert!(profile.trapping_bias_milli > BASE_BIAS_MILLI);
        assert!(profile.food_bias_milli < BASE_BIAS_MILLI);
        assert_eq!(
            moves.ordered_legal_moves_with_profile(Some(&profile))[0],
            Direction::Down
        );
    }

    #[test]
    fn intent_telemetry_records_learning_and_skip_reasons() {
        let mut profile = OpponentProfile::default();
        let contrast = IntentContrast {
            food_milli: -700,
            hunting_milli: 250,
            trapping_milli: 800,
            information_milli: 800,
        };

        profile.record_intent_inference(
            Direction::Down,
            IntentInference::Learned {
                contrast,
                survival_emergency: true,
            },
        );
        profile.record_intent_inference(
            Direction::Left,
            IntentInference::Skipped(IntentSkipReason::ForcedMove),
        );

        assert_eq!(profile.intent_stats.learned_observations, 1);
        assert_eq!(profile.intent_stats.downweighted_survival_emergency, 1);
        assert_eq!(profile.intent_stats.skipped_forced, 1);
        assert_eq!(profile.intent_stats.last_observed, Some(Direction::Left));
        assert_eq!(profile.intent_stats.last_trapping_contrast, 800);
        assert_eq!(profile.intent_stats.last_information_milli, 800);
    }
}
