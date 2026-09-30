use std::time::{Duration, Instant};

use crate::GameState;

const MIN_SAFETY_RESERVE_MS: u64 = 25;
const TIMEOUT_RESERVE_PERCENT: u64 = 15;
const MAX_LATENCY_RESERVE_MS: u64 = 100;

#[derive(Debug, Clone)]
pub(crate) struct SearchBudget {
    started: Instant,
    deadline: Instant,
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
        let usable = Duration::from_millis(timeout_ms.saturating_sub(reserve_ms));

        Self {
            started,
            deadline: started + usable,
            safety_reserve,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_duration(duration: Duration) -> Self {
        let started = Instant::now();
        Self {
            started,
            deadline: started + duration,
            safety_reserve: Duration::ZERO,
        }
    }

    pub(crate) fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    pub(crate) fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub(crate) fn safety_reserve(&self) -> Duration {
        self.safety_reserve
    }

    pub(crate) fn can_afford(&self, estimated: Duration) -> bool {
        !self.expired() && self.remaining() > estimated
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
        assert!(budget.remaining() < Duration::from_millis(500));
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
}
