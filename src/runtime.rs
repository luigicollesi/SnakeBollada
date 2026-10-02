use std::collections::HashMap;
use std::sync::Arc;

use log::{info, warn};
use tokio::sync::{Mutex, RwLock};

use crate::decision::session::DecisionState;
use crate::strategy::Decision;
use crate::GameState;

#[derive(Debug, Default)]
struct GameSession {
    decision_state: Mutex<DecisionState>,
}

#[derive(Debug, Default)]
pub(crate) struct GameRuntime {
    sessions: RwLock<HashMap<String, Arc<GameSession>>>,
}

impl GameRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn start(&self, state: &GameState) {
        let mut sessions = self.sessions.write().await;

        if sessions.contains_key(&state.game.id) {
            warn!("duplicate /start for game {}", state.game.id);
            return;
        }

        sessions.insert(state.game.id.clone(), Arc::new(GameSession::default()));
        info!("GAME START {}", state.game.id);
    }

    pub(crate) async fn decide(&self, state: &GameState) -> Decision {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&state.game.id).cloned()
        };

        let Some(session) = session else {
            warn!(
                "received /move without active session for {}; using stateless fallback",
                state.game.id
            );
            return crate::strategy::choose_move(state);
        };

        let mut decision_state = session.decision_state.lock().await;
        decision_state.decide(state)
    }

    pub(crate) async fn end(&self, state: &GameState) {
        let removed = self.sessions.write().await.remove(&state.game.id);

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
            .cloned()
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

        assert_eq!(runtime.active_game_ids().await, vec!["game-a"]);
    }

    #[tokio::test]
    async fn starts_keep_independent_sessions_for_multiple_games() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.start(&state("game-b")).await;

        assert_eq!(
            runtime.active_game_ids().await,
            vec!["game-a".to_string(), "game-b".to_string()]
        );
    }

    #[tokio::test]
    async fn duplicate_start_keeps_existing_session() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.start(&state("game-a")).await;

        assert_eq!(runtime.active_game_ids().await, vec!["game-a"]);
    }

    #[tokio::test]
    async fn end_discards_only_target_game_session() {
        let runtime = GameRuntime::new();
        let game_a = state("game-a");

        runtime.start(&game_a).await;
        runtime.start(&state("game-b")).await;
        runtime.end(&game_a).await;

        assert_eq!(runtime.active_game_ids().await, vec!["game-b"]);
    }

    #[tokio::test]
    async fn unrelated_end_does_not_delete_other_sessions() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.end(&state("game-b")).await;

        assert_eq!(runtime.active_game_ids().await, vec!["game-a"]);
    }
}
