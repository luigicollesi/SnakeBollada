use std::collections::HashMap;
use std::sync::Arc;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use log::{error, info, trace, warn};
use tokio::sync::{mpsc, RwLock};

use crate::board_mask::BoardMask;
use crate::decision::session::DecisionState;
use crate::direction::Direction;
use crate::strategy::Decision;
use crate::telemetry::{
    now_ms, run_recorder, DecisionRecord, EndRecord, GameRecordStorage, ObservedMove, StartRecord,
    TelemetryEvent, TurnSnapshot,
};
use crate::{Coord, GameState};

const TELEMETRY_CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Clone)]
struct SnakeLiveState {
    head: Coord,
    occupied: BoardMask,
}

#[derive(Debug, Clone)]
struct LiveGameState {
    our_snake_id: String,
    our_cells: BoardMask,
    opponent_cells: BoardMask,
    snakes: HashMap<String, SnakeLiveState>,
}

impl LiveGameState {
    fn from_request(state: &GameState) -> Self {
        let width = state.board.width as u16;
        let height = state.board.height as u16;
        let mut our_cells = BoardMask::new(width, height);
        let mut opponent_cells = BoardMask::new(width, height);
        let mut snakes = HashMap::new();

        for snake in &state.board.snakes {
            let occupied = BoardMask::from_coords(width, height, snake.body.iter().copied());

            if snake.id == state.you.id {
                our_cells.union_with(&occupied);
            } else {
                opponent_cells.union_with(&occupied);
            }

            snakes.insert(
                snake.id.clone(),
                SnakeLiveState {
                    head: snake.head,
                    occupied,
                },
            );
        }

        Self {
            our_snake_id: state.you.id.clone(),
            our_cells,
            opponent_cells,
            snakes,
        }
    }

    fn derive_observed_moves(&self, next: &GameState) -> HashMap<String, ObservedMove> {
        let mut observed = HashMap::new();

        for (snake_id, previous) in &self.snakes {
            if snake_id == &self.our_snake_id {
                continue;
            }

            let movement = next
                .board
                .snakes
                .iter()
                .find(|snake| snake.id == *snake_id)
                .map_or(ObservedMove::EliminatedUnknown, |current| {
                    Direction::from_heads(previous.head, current.head)
                        .map(ObservedMove::Known)
                        .unwrap_or(ObservedMove::NotAvailable)
                });

            observed.insert(snake_id.clone(), movement);
        }

        observed
    }

    fn occupancy_summary(&self) -> (u32, u32, u32) {
        let individually_tracked = self
            .snakes
            .values()
            .map(|snake| snake.occupied.count_ones())
            .sum();

        (
            self.our_cells.count_ones(),
            self.opponent_cells.count_ones(),
            individually_tracked,
        )
    }
}

#[derive(Clone)]
struct GameHandle {
    live_state: Arc<RwLock<LiveGameState>>,
    decision_state: Arc<RwLock<DecisionState>>,
    telemetry_tx: mpsc::Sender<TelemetryEvent>,
}

pub(crate) struct GameRegistry {
    games: DashMap<String, GameHandle>,
    storage: Arc<dyn GameRecordStorage>,
}

impl GameRegistry {
    pub(crate) fn new(storage: Arc<dyn GameRecordStorage>) -> Self {
        Self {
            games: DashMap::new(),
            storage,
        }
    }

    pub(crate) fn start(&self, state: &GameState) {
        let game_id = state.game.id.clone();

        match self.games.entry(game_id.clone()) {
            Entry::Occupied(_) => {
                warn!("duplicate /start for game {game_id}");
            }
            Entry::Vacant(entry) => {
                let (telemetry_tx, telemetry_rx) = mpsc::channel(TELEMETRY_CHANNEL_CAPACITY);
                let live_state = LiveGameState::from_request(state);

                entry.insert(GameHandle {
                    live_state: Arc::new(RwLock::new(live_state)),
                    decision_state: Arc::new(RwLock::new(DecisionState::default())),
                    telemetry_tx: telemetry_tx.clone(),
                });

                tokio::spawn(run_recorder(telemetry_rx, Arc::clone(&self.storage)));

                if let Err(send_error) =
                    telemetry_tx.try_send(TelemetryEvent::Start(StartRecord::from_state(state)))
                {
                    error!("failed to enqueue start telemetry: {send_error}");
                }

                info!("GAME START {game_id}");
            }
        }
    }

    pub(crate) async fn observe_turn(&self, state: &GameState) {
        let handle = match self.handle(&state.game.id) {
            Some(handle) => handle,
            None => {
                warn!(
                    "received /move without known session for {}; recovering",
                    state.game.id
                );
                self.start(state);

                let Some(recovered) = self.handle(&state.game.id) else {
                    error!("failed to recover game session {}", state.game.id);
                    return;
                };
                recovered
            }
        };

        let observed_moves = {
            let mut live_state = handle.live_state.write().await;
            let observed = live_state.derive_observed_moves(state);
            let next_state = LiveGameState::from_request(state);
            let (our_cells, opponent_cells, tracked_cells) = next_state.occupancy_summary();

            trace!(
                "game {} turn {} occupancy ours={} opponents={} tracked={}",
                state.game.id,
                state.turn,
                our_cells,
                opponent_cells,
                tracked_cells
            );

            *live_state = next_state;
            observed
        };

        let snapshot = TurnSnapshot::from_state(state, observed_moves);
        if let Err(send_error) = handle
            .telemetry_tx
            .try_send(TelemetryEvent::TurnSnapshot(snapshot))
        {
            warn!(
                "dropping turn snapshot telemetry for game {}: {send_error}",
                state.game.id
            );
        }
    }

    pub(crate) async fn decide(&self, state: &GameState) -> Decision {
        let Some(handle) = self.handle(&state.game.id) else {
            warn!(
                "cannot decide for unknown game {}; using stateless fallback",
                state.game.id
            );
            return crate::strategy::choose_move(state);
        };

        handle.decision_state.write().await.decide(state)
    }

    pub(crate) fn record_decision(
        &self,
        game_id: &str,
        turn: i32,
        decision_time_us: u64,
        decision: &Decision,
    ) {
        let Some(handle) = self.handle(game_id) else {
            warn!("cannot record decision for unknown game {game_id}");
            return;
        };

        let record = DecisionRecord::from_decision(turn, decision_time_us, decision);
        if let Err(send_error) = handle
            .telemetry_tx
            .try_send(TelemetryEvent::OurDecision(record))
        {
            warn!("dropping decision telemetry for game {game_id}: {send_error}");
        }
    }

    pub(crate) async fn end(&self, state: &GameState) {
        let Some((_, handle)) = self.games.remove(&state.game.id) else {
            warn!("duplicate or unknown /end for game {}", state.game.id);
            return;
        };

        let observed_moves = {
            let live_state = handle.live_state.read().await;
            live_state.derive_observed_moves(state)
        };

        let end = EndRecord {
            ended_at_ms: now_ms(),
            final_snapshot: TurnSnapshot::from_state(state, observed_moves),
        };

        if let Err(send_error) = handle.telemetry_tx.send(TelemetryEvent::End(end)).await {
            warn!(
                "failed to send final telemetry for game {}: {send_error}",
                state.game.id
            );
        }

        info!("GAME OVER {}", state.game.id);
    }

    fn handle(&self, game_id: &str) -> Option<GameHandle> {
        self.games.get(game_id).map(|entry| entry.value().clone())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Game};

    fn snake(id: &str, body: Vec<Coord>) -> Battlesnake {
        Battlesnake {
            id: id.to_string(),
            name: id.to_string(),
            health: 100,
            head: body[0],
            length: body.len() as u32,
            body,
            latency: String::new(),
            shout: None,
        }
    }

    fn state(enemy_head: Coord) -> GameState {
        let ours = snake("ours", vec![Coord { x: 1, y: 1 }, Coord { x: 1, y: 0 }]);
        let enemy = snake("enemy", vec![enemy_head, Coord { x: 5, y: 4 }]);

        GameState {
            game: Game {
                id: "game".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 1,
            board: Board {
                height: 11,
                width: 11,
                food: vec![],
                snakes: vec![ours.clone(), enemy],
                hazards: vec![],
            },
            you: ours,
        }
    }

    #[test]
    fn tracks_our_and_opponent_cells() {
        let live = LiveGameState::from_request(&state(Coord { x: 5, y: 5 }));
        let summary = live.occupancy_summary();

        assert_eq!(summary.0, 2);
        assert_eq!(summary.1, 2);
    }

    #[test]
    fn derives_observed_enemy_direction() {
        let live = LiveGameState::from_request(&state(Coord { x: 5, y: 5 }));
        let next = state(Coord { x: 6, y: 5 });

        assert_eq!(
            live.derive_observed_moves(&next).get("enemy"),
            Some(&ObservedMove::Known(Direction::Right))
        );
    }
}
