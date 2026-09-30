#[macro_use]
extern crate rocket;

use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Instant;

use log::info;
use rocket::fairing::AdHoc;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod board_mask;
mod logic;
mod navigation;
mod runtime;
mod strategy;
mod telemetry;

use runtime::GameRegistry;
use telemetry::FileGameRecordStorage;

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct Game {
    pub(crate) id: String,
    pub(crate) ruleset: HashMap<String, Value>,
    pub(crate) timeout: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct Board {
    pub(crate) height: u32,
    pub(crate) width: u32,
    pub(crate) food: Vec<Coord>,
    pub(crate) snakes: Vec<Battlesnake>,
    pub(crate) hazards: Vec<Coord>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct Battlesnake {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) health: i32,
    pub(crate) body: Vec<Coord>,
    pub(crate) head: Coord,
    pub(crate) length: u32,
    #[serde(default)]
    pub(crate) latency: String,
    #[serde(default)]
    pub(crate) shout: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Coord {
    pub(crate) x: i32,
    pub(crate) y: i32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub(crate) struct GameState {
    pub(crate) game: Game,
    pub(crate) turn: i32,
    pub(crate) board: Board,
    pub(crate) you: Battlesnake,
}

#[get("/")]
fn handle_index() -> Json<Value> {
    Json(logic::info())
}

#[post("/start", format = "json", data = "<start_req>")]
fn handle_start(start_req: Json<GameState>, registry: &State<GameRegistry>) -> Status {
    registry.start(&start_req);
    Status::Ok
}

#[post("/move", format = "json", data = "<move_req>")]
async fn handle_move(
    move_req: Json<GameState>,
    registry: &State<GameRegistry>,
) -> Json<Value> {
    registry.observe_turn(&move_req).await;

    let started = Instant::now();
    let decision = strategy::choose_move(&move_req);
    let decision_time_us = started
        .elapsed()
        .as_micros()
        .try_into()
        .unwrap_or(u64::MAX);

    registry.record_decision(
        &move_req.game.id,
        move_req.turn,
        decision_time_us,
        &decision,
    );

    info!(
        "MOVE {}: {} ({:?}, {} us)",
        move_req.turn,
        decision.direction.as_str(),
        decision.reason,
        decision_time_us
    );

    Json(json!({ "move": decision.direction.as_str() }))
}

#[post("/end", format = "json", data = "<end_req>")]
async fn handle_end(end_req: Json<GameState>, registry: &State<GameRegistry>) -> Status {
    registry.end(&end_req).await;
    Status::Ok
}

#[launch]
fn rocket() -> _ {
    if let Ok(port) = env::var("PORT") {
        env::set_var("ROCKET_PORT", &port);
    }

    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info");
    }

    env_logger::init();
    info!("Starting SnakeBollada Battlesnake server...");

    let storage = Arc::new(FileGameRecordStorage::new("data/games"));
    let registry = GameRegistry::new(storage);

    rocket::build()
        .manage(registry)
        .attach(AdHoc::on_response("Server ID Middleware", |_, res| {
            Box::pin(async move {
                res.set_raw_header("Server", "battlesnake/snake-bollada");
            })
        }))
        .mount(
            "/",
            routes![handle_index, handle_start, handle_move, handle_end],
        )
}
