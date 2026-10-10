use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{info, warn};
use tokio::sync::{oneshot, RwLock, Semaphore};

use crate::decision::session::DecisionState;
use crate::strategy::Decision;
use crate::GameState;

#[derive(Debug, Default)]
struct GameSession {
    decision_state: Mutex<DecisionState>,
}

#[derive(Debug)]
pub(crate) struct GameRuntime {
    sessions: RwLock<HashMap<(String, String), Arc<GameSession>>>,
    // Bound heavy CPU jobs. An overloaded process returns a valid fallback
    // instead of queueing unlimited searches behind the HTTP executor.
    search_slots: Arc<Semaphore>,
}

impl Default for GameRuntime {
    fn default() -> Self {
        let slots = std::thread::available_parallelism()
            .map_or(1, |count| count.get())
            .clamp(1, 4);
        Self {
            sessions: RwLock::new(HashMap::new()),
            search_slots: Arc::new(Semaphore::new(slots)),
        }
    }
}

impl GameRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn start(&self, state: &GameState) {
        let mut sessions = self.sessions.write().await;

        let key = (state.game.id.clone(), state.you.id.clone());
        if sessions.contains_key(&key) {
            warn!(
                "duplicate /start for game {} snake {}",
                state.game.id, state.you.id
            );
            return;
        }

        sessions.insert(key, Arc::new(GameSession::default()));
        info!("GAME START {}", state.game.id);
    }

    pub(crate) async fn decide(&self, state: &GameState, started: Instant) -> Decision {
        // The handler has already begun consuming the Battlesnake timeout.
        // Reserve time for JSON serialization, networking, and runtime jitter.
        let response_deadline =
            Duration::from_millis(u64::from(state.game.timeout).saturating_sub(100).max(1));
        let remaining = response_deadline.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            warn!("MOVE {} request already past response deadline", state.turn);
            return crate::strategy::choose_move_baseline(state);
        }

        let session = {
            let sessions = self.sessions.read().await;
            sessions
                .get(&(state.game.id.clone(), state.you.id.clone()))
                .cloned()
        };
        if session.is_none() {
            warn!(
                "received /move without active session for {}; using stateless search",
                state.game.id
            );
        }
        let permit = match self.search_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                warn!(
                    "MOVE {} CPU search capacity exhausted; safe fallback",
                    state.turn
                );
                return crate::strategy::choose_move_baseline(state);
            }
        };
        let owned = state.clone();
        let (tx, rx) = oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if let Some(session) = session {
                let mut decision_state = session
                    .decision_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // A delayed queued request must not run an obsolete search.
                if started.elapsed()
                    >= Duration::from_millis(
                        u64::from(owned.game.timeout).saturating_sub(100).max(1),
                    )
                {
                    let _ = tx.send(crate::strategy::choose_move_baseline(&owned));
                    return;
                }
                let decision = decision_state.decide_with_start(&owned, started);
                // Send the HTTP decision *before* O(N) graph compaction.
                // If the caller already timed out, it returned a fallback that
                // may differ; the forecast graph must not assume our decision.
                if tx.send(decision).is_ok() {
                    decision_state.post_response_maintenance();
                } else {
                    decision_state.invalidate_undelivered_decision();
                }
            } else {
                let decision = crate::strategy::choose_move(&owned);
                let _ = tx.send(decision);
            }
        });

        match tokio::time::timeout(remaining, rx).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_)) => {
                warn!("MOVE {} CPU worker ended without a decision", state.turn);
                crate::strategy::choose_move_baseline(state)
            }
            Err(_) => {
                warn!(
                    "MOVE {} exceeded internal response deadline after {} ms; safe fallback",
                    state.turn,
                    started.elapsed().as_millis()
                );
                crate::strategy::choose_move_baseline(state)
            }
        }
    }

    pub(crate) async fn end(&self, state: &GameState) {
        let removed = self
            .sessions
            .write()
            .await
            .remove(&(state.game.id.clone(), state.you.id.clone()));

        if removed.is_some() {
            info!("GAME OVER {}", state.game.id);
        } else {
            warn!("duplicate or unknown /end for game {}", state.game.id);
        }
    }

    #[cfg(test)]
    async fn active_game_ids(&self) -> Vec<String> {
        let mut ids = self
            .sessions
            .read()
            .await
            .keys()
            .map(|(game, snake)| format!("{game}:{snake}"))
            .collect::<Vec<_>>();
        ids.sort();
        ids
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};

    fn state(game_id: &str) -> GameState {
        let ours = Battlesnake {
            id: "ours".to_string(),
            name: "ours".to_string(),
            health: 100,
            body: vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }],
            head: Coord { x: 1, y: 1 },
            length: 2,
            latency: String::new(),
            shout: None,
        };

        GameState {
            game: Game {
                id: game_id.to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 0,
            board: Board {
                height: 11,
                width: 11,
                food: vec![],
                snakes: vec![ours.clone()],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[tokio::test]
    async fn start_creates_active_session() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;

        assert_eq!(runtime.active_game_ids().await, vec!["game-a:ours"]);
    }

    #[tokio::test]
    async fn starts_keep_independent_sessions_for_multiple_games() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.start(&state("game-b")).await;

        assert_eq!(
            runtime.active_game_ids().await,
            vec!["game-a:ours".to_string(), "game-b:ours".to_string()]
        );
    }

    #[tokio::test]
    async fn selfplay_sessions_do_not_share_decisions_or_graphs() {
        let runtime = GameRuntime::new();
        let first = state("shared-game");
        let mut second = first.clone();
        second.you.id = "other-snake".to_string();
        second.board.snakes.push(second.you.clone());

        runtime.start(&first).await;
        runtime.start(&second).await;
        assert_eq!(
            runtime.active_game_ids().await,
            vec!["shared-game:other-snake", "shared-game:ours"]
        );

        let sessions = runtime.sessions.read().await;
        let one = sessions
            .get(&("shared-game".to_string(), "ours".to_string()))
            .unwrap();
        let two = sessions
            .get(&("shared-game".to_string(), "other-snake".to_string()))
            .unwrap();
        assert!(
            !Arc::ptr_eq(one, two),
            "same game, independent search session"
        );
        drop(sessions);

        runtime.end(&first).await;
        assert_eq!(
            runtime.active_game_ids().await,
            vec!["shared-game:other-snake"]
        );
    }

    #[tokio::test]
    async fn duplicate_start_keeps_existing_session() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.start(&state("game-a")).await;

        assert_eq!(runtime.active_game_ids().await, vec!["game-a:ours"]);
    }

    #[tokio::test]
    async fn end_discards_only_target_game_session() {
        let runtime = GameRuntime::new();
        let game_a = state("game-a");

        runtime.start(&game_a).await;
        runtime.start(&state("game-b")).await;
        runtime.end(&game_a).await;

        assert_eq!(runtime.active_game_ids().await, vec!["game-b:ours"]);
    }

    #[tokio::test]
    async fn duplicate_move_returns_same_decision() {
        let runtime = GameRuntime::new();
        let board = state("game-duplicate");
        runtime.start(&board).await;
        let first = runtime.decide(&board, Instant::now()).await;
        let again = runtime.decide(&board, Instant::now()).await;
        assert_eq!(first.direction, again.direction);
        assert_eq!(first.reason, again.reason);
    }

    #[tokio::test]
    async fn expired_request_returns_without_cpu_work() {
        let runtime = GameRuntime::new();
        let board = state("game-expired");
        runtime.start(&board).await;
        let started = Instant::now() - Duration::from_secs(1);
        let before = Instant::now();
        let decision = runtime.decide(&board, started).await;
        assert!(before.elapsed() < Duration::from_millis(100));
        assert!(crate::strategy::direction_stays_in_bounds(
            &board,
            decision.direction
        ));
    }

    #[tokio::test]
    async fn exhausted_cpu_capacity_returns_safe_move() {
        let runtime = GameRuntime::new();
        let board = state("game-overloaded");
        runtime.start(&board).await;
        let slots = runtime.search_slots.clone();
        let count = slots.available_permits();
        let _held = slots.acquire_many_owned(count as u32).await.unwrap();
        let decision = runtime.decide(&board, Instant::now()).await;
        assert!(crate::strategy::direction_stays_in_bounds(
            &board,
            decision.direction
        ));
    }

    #[tokio::test]
    async fn unrelated_end_does_not_delete_other_sessions() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.end(&state("game-b")).await;

        assert_eq!(runtime.active_game_ids().await, vec!["game-a:ours"]);
    }
}
