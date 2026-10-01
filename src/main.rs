#[macro_use]
extern crate rocket;

use std::collections::HashMap;
use std::env;

use log::info;
use rocket::fairing::AdHoc;
use rocket::http::Status;
use rocket::serde::json::Json;
use rocket::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

mod analysis;
mod board_mask;
mod decision;
mod direction;
mod enemy;
mod evaluation;
mod forecast;
mod logic;
mod modes;
mod navigation;
mod runtime;
mod search;
mod simulation;
mod spatial;
mod strategy;

use runtime::GameRuntime;

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
async fn handle_start(start_req: Json<GameState>, runtime: &State<GameRuntime>) -> Status {
    runtime.start(&start_req).await;
    Status::Ok
}

#[post("/move", format = "json", data = "<move_req>")]
async fn handle_move(move_req: Json<GameState>, runtime: &State<GameRuntime>) -> Json<Value> {
    let decision = runtime.decide(&move_req).await;

    let chosen_outcome = decision.search.direction_outcomes[usize::from(decision.direction.rank())];
    let beam_shadow = decision.search.beam_shadow;
    let beam_direction = beam_shadow
        .direction
        .map_or("-", |direction| direction.as_str());
    info!(
        "MOVE {}: {} ({:?}) completed_depth={} analyzed_depth={} nodes={} edges={} phase={:?} hunt={} food={} dom={} control={} border={} pin={} escape={} escape_selected={} edge_ticks={} edge_cost={} beam_shadow={} beam_move={} beam_depth={} beam_attempted={} beam_agree={} beam_value={} beam_us={}",
        move_req.turn,
        decision.direction.as_str(),
        decision.reason,
        decision.search.completed_depth,
        decision.search.analyzed_depth,
        decision.search.nodes,
        decision.search.edges,
        decision.search.strategic_phase,
        decision.search.hunt_drive_milli,
        decision.search.food_urgency_milli,
        decision.search.size_dominance_milli,
        decision.search.control_ratio_milli,
        decision.search.border_risk_milli,
        decision.search.enemy_pin_risk_milli,
        decision.search.escape_pressure_milli,
        decision.search.escape_selected,
        chosen_outcome.average_border_ticks_milli,
        chosen_outcome.average_border_cost_milli,
        beam_shadow.enabled,
        beam_direction,
        beam_shadow.completed_depth,
        beam_shadow.attempted_depth,
        beam_shadow.agreed_with_legacy,
        beam_shadow.best_value,
        beam_shadow.elapsed_us,
    );

    Json(json!({
        "move": decision.direction.as_str(),
        "shout": debug_shout(decision.search.analyzed_depth),
    }))
}

fn debug_shout(analyzed_depth: u8) -> String {
    let unit = if analyzed_depth == 1 {
        "lance"
    } else {
        "lances"
    };
    format!("DEBUG: {analyzed_depth} {unit} à frente")
}

#[post("/end", format = "json", data = "<end_req>")]
async fn handle_end(end_req: Json<GameState>, runtime: &State<GameRuntime>) -> Status {
    runtime.end(&end_req).await;
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

    let runtime = GameRuntime::new();

    rocket::build()
        .manage(runtime)
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

#[cfg(test)]
mod tests {
    use super::debug_shout;

    #[test]
    fn debug_shout_reports_single_future_ply() {
        assert_eq!(debug_shout(1), "DEBUG: 1 lance à frente");
    }

    #[test]
    fn debug_shout_reports_deepest_analyzed_ply() {
        assert_eq!(debug_shout(7), "DEBUG: 7 lances à frente");
    }
}
