use log::{info, warn};
use tokio::sync::Mutex;

use crate::decision::session::DecisionState;
use crate::strategy::Decision;
use crate::GameState;

#[derive(Debug)]
struct ActiveGame {
    game_id: String,
    decision_state: DecisionState,
}

#[derive(Debug, Default)]
pub(crate) struct GameRuntime {
    active: Mutex<Option<ActiveGame>>,
}

impl GameRuntime {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn start(&self, state: &GameState) {
        let mut active = self.active.lock().await;

        if let Some(current) = active.as_ref() {
            if current.game_id == state.game.id {
                warn!("duplicate /start for game {}", state.game.id);
                return;
            }

            warn!(
                "replacing active game {} with {}",
                current.game_id, state.game.id
            );
        }

        *active = Some(ActiveGame {
            game_id: state.game.id.clone(),
            decision_state: DecisionState::default(),
        });

        info!("GAME START {}", state.game.id);
    }

    pub(crate) async fn decide(&self, state: &GameState) -> Decision {
        let mut active = self.active.lock().await;

        let Some(session) = active.as_mut() else {
            warn!(
                "received /move without active session for {}; using stateless fallback",
                state.game.id
            );
            return crate::strategy::choose_move(state);
        };

        if session.game_id != state.game.id {
            warn!(
                "received /move for {} while {} is active; using stateless fallback",
                state.game.id, session.game_id
            );
            return crate::strategy::choose_move(state);
        }

        session.decision_state.decide(state)
    }

    pub(crate) async fn end(&self, state: &GameState) {
        let mut active = self.active.lock().await;

        match active.as_ref() {
            Some(session) if session.game_id == state.game.id => {
                *active = None;
                info!("GAME OVER {}", state.game.id);
            }
            Some(session) => {
                warn!(
                    "received /end for {} while {} is active; keeping current session",
                    state.game.id, session.game_id
                );
            }
            None => {
                warn!("duplicate or unknown /end for game {}", state.game.id);
            }
        }
    }

    #[cfg(test)]
    async fn active_game_id(&self) -> Option<String> {
        self.active
            .lock()
            .await
            .as_ref()
            .map(|session| session.game_id.clone())
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
    async fn start_creates_single_active_session() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;

        assert_eq!(runtime.active_game_id().await.as_deref(), Some("game-a"));
    }

    #[tokio::test]
    async fn new_start_replaces_previous_session() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.start(&state("game-b")).await;

        assert_eq!(runtime.active_game_id().await.as_deref(), Some("game-b"));
    }

    #[tokio::test]
    async fn end_discards_active_tree_session() {
        let runtime = GameRuntime::new();
        let game = state("game-a");

        runtime.start(&game).await;
        runtime.end(&game).await;

        assert_eq!(runtime.active_game_id().await, None);
    }

    #[tokio::test]
    async fn unrelated_end_does_not_delete_active_session() {
        let runtime = GameRuntime::new();

        runtime.start(&state("game-a")).await;
        runtime.end(&state("game-b")).await;

        assert_eq!(runtime.active_game_id().await.as_deref(), Some("game-a"));
    }
}
