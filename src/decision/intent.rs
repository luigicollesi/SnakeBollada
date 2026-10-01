use crate::modes::hunting::{HuntingPlanCandidate, HuntingPlanKind};
use crate::Coord;

pub(crate) const HUNT_INTENT_LOCK_TURNS: i32 = 4;
pub(crate) const HUNT_INTENT_STALE_LIMIT: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DecisionIntent {
    Food(FoodIntent),
    Hunt(HuntIntent),
}

impl DecisionIntent {
    pub(crate) fn food_target(&self) -> Option<Coord> {
        match self {
            Self::Food(intent) => Some(intent.target),
            Self::Hunt(_) => None,
        }
    }

    pub(crate) fn hunt(&self) -> Option<&HuntIntent> {
        match self {
            Self::Food(_) => None,
            Self::Hunt(intent) => Some(intent),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FoodIntent {
    pub(crate) target: Coord,
    pub(crate) started_turn: i32,
}

impl FoodIntent {
    pub(crate) const fn new(target: Coord, started_turn: i32) -> Self {
        Self {
            target,
            started_turn,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HuntIntent {
    pub(crate) target: String,
    pub(crate) kind: HuntingPlanKind,
    pub(crate) started_turn: i32,
    pub(crate) lock_until_turn: i32,
    pub(crate) stale_turns: u8,
    pub(crate) last_score_milli: u16,
}

impl HuntIntent {
    pub(crate) fn new(plan: &HuntingPlanCandidate, started_turn: i32) -> Self {
        Self {
            target: plan.target.clone(),
            kind: plan.kind,
            started_turn,
            lock_until_turn: started_turn.saturating_add(HUNT_INTENT_LOCK_TURNS),
            stale_turns: 0,
            last_score_milli: plan.score_milli,
        }
    }

    pub(crate) fn is_locked(&self, turn: i32) -> bool {
        turn <= self.lock_until_turn
    }

    pub(crate) fn record_plan(&mut self, score_milli: Option<u16>) {
        if let Some(score_milli) = score_milli {
            self.last_score_milli = score_milli;
            self.stale_turns = 0;
        } else {
            self.stale_turns = self.stale_turns.saturating_add(1);
        }
    }

    pub(crate) fn should_release(&self, turn: i32) -> bool {
        !self.is_locked(turn) && self.stale_turns >= HUNT_INTENT_STALE_LIMIT
    }
}

pub(crate) fn committable_hunt_plan(plan: &HuntingPlanCandidate) -> bool {
    match plan.kind {
        HuntingPlanKind::HeadPressure => false,
        HuntingPlanKind::EdgePin => plan.score_milli >= 300,
        HuntingPlanKind::ChokeCut | HuntingPlanKind::TerritorySqueeze => {
            plan.score_milli >= 400
        }
        HuntingPlanKind::PartialWrap => plan.score_milli >= 460,
        HuntingPlanKind::FullEnclosure => plan.score_milli >= 650,
        HuntingPlanKind::StarvationSiege => plan.score_milli >= 500,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hunt_intent_has_minimum_lock_window() {
        let plan = HuntingPlanCandidate {
            target: "enemy".to_string(),
            kind: HuntingPlanKind::PartialWrap,
            score_milli: 700,
            length_advantage: 2,
        };
        let intent = HuntIntent::new(&plan, 10);

        assert!(intent.is_locked(10));
        assert!(intent.is_locked(14));
        assert!(!intent.is_locked(15));
    }

    #[test]
    fn hunt_intent_needs_repeated_staleness_after_lock() {
        let plan = HuntingPlanCandidate {
            target: "enemy".to_string(),
            kind: HuntingPlanKind::TerritorySqueeze,
            score_milli: 600,
            length_advantage: 1,
        };
        let mut intent = HuntIntent::new(&plan, 10);
        intent.record_plan(None);
        intent.record_plan(None);

        assert!(!intent.should_release(14));
        assert!(intent.should_release(15));
    }
}
