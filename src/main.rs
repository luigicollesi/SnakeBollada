#[macro_use]
extern crate rocket;

use std::collections::HashMap;
use std::env;
use std::thread;
use std::time::Instant;

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
mod evaluation;
mod logic;
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
    let request_started = Instant::now();
    let decision = runtime.decide(&move_req, request_started).await;

    let hobbs = decision.search.hobbs;
    info!(
        "MOVE {}: {} ({:?}) depth={} nodes={} edges={} transpositions={} elapsed_us={} reserve_us={} score={:?} guard={:?} provisional={} terminal_confirmed={} root_directions={} batches={} parallel_batches={} actions={} new_nodes={} resolve_us={} build_us={} merge_us={}",
        move_req.turn,
        decision.direction.as_str(),
        decision.reason,
        decision.search.completed_depth,
        decision.search.nodes,
        decision.search.edges,
        decision.search.transposition_hits,
        decision.search.elapsed_us,
        decision.search.safety_reserve_us,
        hobbs.score,
        hobbs.guard,
        hobbs.forecast_provisional,
        hobbs.terminal_confirmed,
        hobbs.root_directions,
        hobbs.action_batches,
        hobbs.parallel_action_batches,
        hobbs.resolved_actions,
        hobbs.new_nodes_built,
        hobbs.resolve_us,
        hobbs.node_build_us,
        hobbs.merge_us
    );

    log::debug!(
        target: "request_timing",
        "move_handler turn={} elapsed_us={} timeout_ms={}",
        move_req.turn, request_started.elapsed().as_micros(), move_req.game.timeout,
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

fn init_search_parallelism() -> usize {
    let available = thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1)
        .max(1);
    let configured = env::var("SNAKE_SEARCH_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|threads| *threads > 0);
    let threads = configured.unwrap_or(available);

    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|index| format!("snake-search-{index}"))
        .build_global();

    rayon::current_num_threads()
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
    let search_threads = init_search_parallelism();
    info!(
        "Starting SnakeBollada Battlesnake server with {} search threads...",
        search_threads
    );

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
