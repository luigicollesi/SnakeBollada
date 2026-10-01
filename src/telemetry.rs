use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use log::{error, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::fs;
use tokio::sync::mpsc;

use crate::direction::Direction;
use crate::strategy::{
    CacheInvalidationReason, Decision, DecisionReason, DepthSearchStats, DirectionOutcomeSummary,
    STRATEGY_VERSION,
};
use crate::{Battlesnake, Coord, GameState};

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(tag = "status", content = "direction", rename_all = "snake_case")]
pub(crate) enum ObservedMove {
    Known(Direction),
    EliminatedUnknown,
    NotAvailable,
}

#[derive(Debug)]
pub(crate) enum TelemetryEvent {
    Start(StartRecord),
    TurnSnapshot(TurnSnapshot),
    OurDecision(Box<DecisionRecord>),
    End(EndRecord),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct StartRecord {
    pub(crate) game_id: String,
    pub(crate) ruleset: HashMap<String, Value>,
    pub(crate) timeout_ms: u32,
    pub(crate) board_width: u32,
    pub(crate) board_height: u32,
    pub(crate) our_snake_id: String,
    pub(crate) started_at_ms: u64,
    pub(crate) initial_snapshot: TurnSnapshot,
}

impl StartRecord {
    pub(crate) fn from_state(state: &GameState) -> Self {
        Self {
            game_id: state.game.id.clone(),
            ruleset: state.game.ruleset.clone(),
            timeout_ms: state.game.timeout,
            board_width: state.board.width,
            board_height: state.board.height,
            our_snake_id: state.you.id.clone(),
            started_at_ms: now_ms(),
            initial_snapshot: TurnSnapshot::from_state(state, HashMap::new()),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TurnSnapshot {
    pub(crate) turn: i32,
    pub(crate) captured_at_ms: u64,
    pub(crate) food: Vec<Coord>,
    pub(crate) hazards: Vec<Coord>,
    pub(crate) snakes: Vec<SnakeSnapshot>,
    pub(crate) observed_moves: HashMap<String, ObservedMove>,
}

impl TurnSnapshot {
    pub(crate) fn from_state(
        state: &GameState,
        observed_moves: HashMap<String, ObservedMove>,
    ) -> Self {
        Self {
            turn: state.turn,
            captured_at_ms: now_ms(),
            food: state.board.food.clone(),
            hazards: state.board.hazards.clone(),
            snakes: state.board.snakes.iter().map(SnakeSnapshot::from).collect(),
            observed_moves,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct SnakeSnapshot {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) health: i32,
    pub(crate) length: u32,
    pub(crate) head: Coord,
    pub(crate) body: Vec<Coord>,
}

impl From<&Battlesnake> for SnakeSnapshot {
    fn from(snake: &Battlesnake) -> Self {
        Self {
            id: snake.id.clone(),
            name: snake.name.clone(),
            health: snake.health,
            length: snake.length,
            head: snake.head,
            body: snake.body.clone(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct SearchRecord {
    pub(crate) completed_depth: u8,
    pub(crate) nodes: u32,
    pub(crate) edges: u32,
    pub(crate) transposition_hits: u32,
    pub(crate) cache_reused: bool,
    pub(crate) cache_invalidation: CacheInvalidationReason,
    pub(crate) elapsed_us: u64,
    pub(crate) safety_reserve_us: u64,
    pub(crate) runtime_jitter_reserve_us: u64,
    pub(crate) dag_nodes_evaluated: u32,
    pub(crate) dag_memo_hits: u32,
    pub(crate) dag_deterministic_evaluations: u32,
    pub(crate) dag_provisional_evaluations: u32,
    pub(crate) aggression_milli: u16,
    pub(crate) enemy_moves_observed: u16,
    pub(crate) enemy_moves_legal_covered: u16,
    pub(crate) enemy_moves_plausible_covered: u16,
    pub(crate) food_spawn_invalidations: u32,
    pub(crate) food_mutation_invalidations: u32,
    pub(crate) depth_stats: [DepthSearchStats; 6],
    pub(crate) direction_outcomes: [DirectionOutcomeSummary; 4],
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct DecisionRecord {
    pub(crate) turn: i32,
    pub(crate) chosen_move: Direction,
    pub(crate) decision_time_us: u64,
    pub(crate) strategy_version: String,
    pub(crate) reason: DecisionReason,
    pub(crate) target_food: Option<Coord>,
    pub(crate) path_distance: Option<u16>,
    pub(crate) reachable_cells: u32,
    pub(crate) search: SearchRecord,
}

impl DecisionRecord {
    pub(crate) fn from_decision(turn: i32, decision_time_us: u64, decision: &Decision) -> Self {
        Self {
            turn,
            chosen_move: decision.direction,
            decision_time_us,
            strategy_version: STRATEGY_VERSION.to_string(),
            reason: decision.reason,
            target_food: decision.target_food,
            path_distance: decision.path_distance,
            reachable_cells: decision.reachable_cells,
            search: SearchRecord {
                completed_depth: decision.search.completed_depth,
                nodes: decision.search.nodes,
                edges: decision.search.edges,
                transposition_hits: decision.search.transposition_hits,
                cache_reused: decision.search.cache_reused,
                cache_invalidation: decision.search.cache_invalidation,
                elapsed_us: decision.search.elapsed_us,
                safety_reserve_us: decision.search.safety_reserve_us,
                runtime_jitter_reserve_us: decision.search.runtime_jitter_reserve_us,
                dag_nodes_evaluated: decision.search.dag_nodes_evaluated,
                dag_memo_hits: decision.search.dag_memo_hits,
                dag_deterministic_evaluations: decision.search.dag_deterministic_evaluations,
                dag_provisional_evaluations: decision.search.dag_provisional_evaluations,
                aggression_milli: decision.search.aggression_milli,
                enemy_moves_observed: decision.search.enemy_moves_observed,
                enemy_moves_legal_covered: decision.search.enemy_moves_legal_covered,
                enemy_moves_plausible_covered: decision.search.enemy_moves_plausible_covered,
                food_spawn_invalidations: decision.search.food_spawn_invalidations,
                food_mutation_invalidations: decision.search.food_mutation_invalidations,
                depth_stats: decision.search.depth_stats,
                direction_outcomes: decision.search.direction_outcomes,
            },
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct EndRecord {
    pub(crate) ended_at_ms: u64,
    pub(crate) final_snapshot: TurnSnapshot,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct TurnRecord {
    pub(crate) turn: i32,
    pub(crate) snapshot: Option<TurnSnapshot>,
    pub(crate) decision: Option<DecisionRecord>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct GameRecord {
    pub(crate) schema_version: u32,
    pub(crate) game_id: String,
    pub(crate) ruleset: HashMap<String, Value>,
    pub(crate) timeout_ms: u32,
    pub(crate) board_width: u32,
    pub(crate) board_height: u32,
    pub(crate) our_snake_id: String,
    pub(crate) started_at_ms: u64,
    pub(crate) ended_at_ms: Option<u64>,
    pub(crate) turns: Vec<TurnRecord>,
}

impl GameRecord {
    fn from_start(start: StartRecord) -> Self {
        let initial_turn = start.initial_snapshot.turn;

        Self {
            schema_version: 5,
            game_id: start.game_id,
            ruleset: start.ruleset,
            timeout_ms: start.timeout_ms,
            board_width: start.board_width,
            board_height: start.board_height,
            our_snake_id: start.our_snake_id,
            started_at_ms: start.started_at_ms,
            ended_at_ms: None,
            turns: vec![TurnRecord {
                turn: initial_turn,
                snapshot: Some(start.initial_snapshot),
                decision: None,
            }],
        }
    }

    fn upsert_snapshot(&mut self, snapshot: TurnSnapshot) {
        let turn = snapshot.turn;
        self.turn_mut(turn).snapshot = Some(snapshot);
    }

    fn upsert_decision(&mut self, decision: DecisionRecord) {
        let turn = decision.turn;
        self.turn_mut(turn).decision = Some(decision);
    }

    fn turn_mut(&mut self, turn: i32) -> &mut TurnRecord {
        if let Some(index) = self.turns.iter().position(|record| record.turn == turn) {
            return &mut self.turns[index];
        }

        self.turns.push(TurnRecord {
            turn,
            snapshot: None,
            decision: None,
        });
        self.turns.sort_by_key(|record| record.turn);

        let index = self
            .turns
            .iter()
            .position(|record| record.turn == turn)
            .expect("inserted turn exists");

        &mut self.turns[index]
    }
}

#[async_trait]
pub(crate) trait GameRecordStorage: Send + Sync {
    async fn save(&self, record: &GameRecord) -> io::Result<()>;
}

#[derive(Debug, Clone)]
pub(crate) struct FileGameRecordStorage {
    root: PathBuf,
}

impl FileGameRecordStorage {
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, game_id: &str) -> PathBuf {
        let safe_id = game_id
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();

        self.root.join(format!("{safe_id}.json"))
    }
}

#[async_trait]
impl GameRecordStorage for FileGameRecordStorage {
    async fn save(&self, record: &GameRecord) -> io::Result<()> {
        fs::create_dir_all(&self.root).await?;

        let target = self.path_for(&record.game_id);
        let temporary = temporary_path(&target);
        let payload = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;

        fs::write(&temporary, payload).await?;
        fs::rename(&temporary, &target).await?;

        Ok(())
    }
}

pub(crate) async fn run_recorder(
    mut receiver: mpsc::Receiver<TelemetryEvent>,
    storage: Arc<dyn GameRecordStorage>,
) {
    let mut game_record: Option<GameRecord> = None;

    while let Some(event) = receiver.recv().await {
        match event {
            TelemetryEvent::Start(start) => {
                if game_record.is_some() {
                    warn!("received duplicate telemetry start event");
                    continue;
                }

                game_record = Some(GameRecord::from_start(start));
            }
            TelemetryEvent::TurnSnapshot(snapshot) => {
                if let Some(record) = game_record.as_mut() {
                    record.upsert_snapshot(snapshot);
                } else {
                    warn!("received turn snapshot before telemetry start");
                }
            }
            TelemetryEvent::OurDecision(decision) => {
                if let Some(record) = game_record.as_mut() {
                    record.upsert_decision(*decision);
                } else {
                    warn!("received decision before telemetry start");
                }
            }
            TelemetryEvent::End(end) => {
                let Some(mut record) = game_record.take() else {
                    warn!("received telemetry end before telemetry start");
                    break;
                };

                record.ended_at_ms = Some(end.ended_at_ms);
                record.upsert_snapshot(end.final_snapshot);

                if let Err(save_error) = storage.save(&record).await {
                    error!(
                        "failed to persist telemetry for game {}: {save_error}",
                        record.game_id
                    );
                }

                break;
            }
        }
    }
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn temporary_path(target: &Path) -> PathBuf {
    let mut value = target.as_os_str().to_os_string();
    value.push(".tmp");
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_game_ids_for_file_names() {
        let storage = FileGameRecordStorage::new("data/games");
        assert_eq!(
            storage.path_for("../bad/game"),
            PathBuf::from("data/games/.._bad_game.json")
        );
    }
}
