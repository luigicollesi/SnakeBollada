use std::time::{Duration, Instant};

use crate::GameState;

const MIN_SAFETY_RESERVE_MS: u64 = 25;
const TIMEOUT_RESERVE_PERCENT: u64 = 15;
const MAX_LATENCY_RESERVE_MS: u64 = 100;
const SOFT_TARGET_PERCENT: u64 = 60;

#[derive(Debug, Clone)]
pub(crate) struct SearchBudget {
    started: Instant,
    soft_deadline: Instant,
    hard_deadline: Instant,
    safety_reserve: Duration,
}

impl SearchBudget {
    pub(crate) fn from_state_with_extra_reserve(state: &GameState, extra_reserve_ms: u64) -> Self {
        let timeout_ms = u64::from(state.game.timeout);
        let reported_latency_ms = state.you.latency.parse::<u64>().unwrap_or(0);
        let percentage_reserve = timeout_ms
            .saturating_mul(TIMEOUT_RESERVE_PERCENT)
            .saturating_div(100);
        let latency_reserve = reported_latency_ms.min(MAX_LATENCY_RESERVE_MS);
        let reserve_ms = MIN_SAFETY_RESERVE_MS
            .max(percentage_reserve)
            .saturating_add(latency_reserve)
            .saturating_add(extra_reserve_ms)
            .min(timeout_ms.saturating_sub(1));

        let started = Instant::now();
        let safety_reserve = Duration::from_millis(reserve_ms);
        let hard_usable_ms = timeout_ms.saturating_sub(reserve_ms);
        let soft_usable_ms = timeout_ms
            .saturating_mul(SOFT_TARGET_PERCENT)
            .saturating_div(100)
            .min(hard_usable_ms);

        Self {
            started,
            soft_deadline: started + Duration::from_millis(soft_usable_ms),
            hard_deadline: started + Duration::from_millis(hard_usable_ms),
            safety_reserve,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_duration(duration: Duration) -> Self {
        let started = Instant::now();
        Self {
            started,
            soft_deadline: started + duration,
            hard_deadline: started + duration,
            safety_reserve: Duration::ZERO,
        }
    }

    pub(crate) fn expired(&self) -> bool {
        self.hard_expired()
    }

    pub(crate) fn hard_expired(&self) -> bool {
        Instant::now() >= self.hard_deadline
    }

    #[cfg(test)]
    pub(crate) fn remaining_soft(&self) -> Duration {
        self.soft_deadline.saturating_duration_since(Instant::now())
    }

    #[cfg(test)]
    pub(crate) fn remaining_hard(&self) -> Duration {
        self.hard_deadline.saturating_duration_since(Instant::now())
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub(crate) fn limited_to_soft_deadline(&self) -> Self {
        Self {
            started: self.started,
            soft_deadline: self.soft_deadline,
            hard_deadline: self.soft_deadline,
            safety_reserve: self.safety_reserve,
        }
    }

    pub(crate) fn safety_reserve(&self) -> Duration {
        self.safety_reserve
    }

}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};

    fn state(timeout: u32, latency: &str) -> GameState {
        let ours = Battlesnake {
            id: "ours".to_string(),
            name: "ours".to_string(),
            health: 100,
            body: vec![Coord { x: 1, y: 1 }],
            head: Coord { x: 1, y: 1 },
            length: 1,
            latency: latency.to_string(),
            shout: None,
        };

        GameState {
            game: Game {
                id: "budget".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout,
            },
            turn: 1,
            board: Board {
                width: 7,
                height: 7,
                food: vec![],
                snakes: vec![ours.clone()],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn preserves_nonzero_safety_reserve() {
        let budget = SearchBudget::from_state_with_extra_reserve(&state(500, "10"), 0);

        assert!(budget.safety_reserve() >= Duration::from_millis(25));
        assert!(budget.remaining_hard() < Duration::from_millis(500));
    }

    #[test]
    fn soft_deadline_targets_sixty_percent_of_timeout() {
        let budget = SearchBudget::from_state_with_extra_reserve(&state(500, "0"), 0);

        assert!(budget.remaining_soft() <= Duration::from_millis(300));
        assert!(budget.remaining_soft() >= Duration::from_millis(250));
        assert!(budget.remaining_hard() > budget.remaining_soft());
    }

    #[test]
    fn extra_runtime_jitter_increases_reserve() {
        let base = SearchBudget::from_state_with_extra_reserve(&state(500, "0"), 0);
        let jittered = SearchBudget::from_state_with_extra_reserve(&state(500, "0"), 40);

        assert!(jittered.safety_reserve() > base.safety_reserve());
    }

    #[test]
    fn reported_latency_increases_reserve() {
        let low = SearchBudget::from_state_with_extra_reserve(&state(500, "0"), 0);
        let high = SearchBudget::from_state_with_extra_reserve(&state(500, "80"), 0);

        assert!(high.safety_reserve() > low.safety_reserve());
    }

    #[test]
    fn soft_limited_budget_uses_soft_deadline_as_hard_deadline() {
        let budget = SearchBudget::from_state_with_extra_reserve(&state(500, "0"), 0);
        let limited = budget.limited_to_soft_deadline();

        let limited_hard = limited.remaining_hard();
        let original_soft = budget.remaining_soft();

        assert!(
            limited_hard.saturating_sub(original_soft) <= Duration::from_millis(1),
            "soft-limited hard deadline must match the original soft deadline"
        );
        assert!(limited_hard < budget.remaining_hard());
    }
}
