#!/usr/bin/env python3
"""Stage the Ordering V3 engine in Maua's AWS Lambda template without deploying.

Usage (from SnakeBollada dev checkout with Git + Rust + network access):
    python3 scripts/prepare_maua_v3_port.py ../battlesnake_luigi
    python3 scripts/prepare_maua_v3_port.py ../battlesnake_luigi --push

The new branch is NEVER "dev"; --push only pushes that isolated branch.
"""
from __future__ import annotations

import argparse
from pathlib import Path
import shutil
import subprocess
import sys

SOURCE_BASE = "10f9c7035a5b10f6c8efea56d61ed295b541c430"
DEST_REMOTE = "https://github.com/Maua-Dev/battlesnake_luigi.git"
BRANCH = "feature/ordering-v3-lambda-port"
CORE_FILES = ["board_mask.rs", "direction.rs", "navigation.rs", "spatial.rs",
              "strategy.rs", "runtime.rs"]
CORE_DIRS = ["analysis", "decision", "evaluation", "search", "simulation"]

def run(*args: str, cwd: Path | None = None) -> None:
    print("+", " ".join(args), flush=True)
    subprocess.run(args, cwd=cwd, check=True)

def capture(*args: str, cwd: Path | None = None) -> str:
    return subprocess.check_output(args, cwd=cwd, text=True).strip()

def replace_exact(text: str, old: str, new: str, path: str) -> str:
    if text.count(old) != 1:
        raise ValueError(f"{path}: expected one occurrence of {old!r}, got {text.count(old)}")
    return text.replace(old, new, 1)

MODELS = r'''//! Internal SnakeBollada simulation types; Lambda input lives in models.rs.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

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

impl From<&crate::models::Coord> for Coord {
    fn from(c: &crate::models::Coord) -> Self { Self { x: c.x, y: c.y } }
}
impl From<&crate::models::Battlesnake> for Battlesnake {
    fn from(s: &crate::models::Battlesnake) -> Self {
        Self {
            id: s.id.clone(),
            name: s.name.clone(),
            health: s.health,
            body: s.body.iter().map(Coord::from).collect(),
            head: Coord::from(&s.head),
            length: s.length.max(0) as u32,
            latency: s.latency.clone().unwrap_or_default(),
            shout: s.shout.clone(),
        }
    }
}
impl From<&crate::models::GameState> for GameState {
    fn from(state: &crate::models::GameState) -> Self {
        let you = Battlesnake::from(&state.you);
        let mut snakes: Vec<Battlesnake> =
            state.board.snakes.iter().map(Battlesnake::from).collect();
        // Tolerate isolated handler fixtures without "you" in board.snakes.
        if !snakes.iter().any(|snake| snake.id == you.id) {
            snakes.push(you.clone());
        }
        Self {
            game: Game {
                id: state.game.id.clone(),
                ruleset: state.game.ruleset.clone(),
                timeout: state.game.timeout,
            },
            turn: state.turn,
            board: Board {
                height: state.board.height.max(0) as u32,
                width: state.board.width.max(0) as u32,
                food: state.board.food.iter().map(Coord::from).collect(),
                snakes,
                hazards: state.board.hazards.iter().map(Coord::from).collect(),
            },
            you,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_uses_our_snake_and_preserves_board() {
        let payload = serde_json::json!({
            "game": {"id": "g", "ruleset": {"name": "standard"}, "timeout": 500},
            "turn": 1,
            "board": {"width": 11, "height": 11, "food": [], "hazards": [], "snakes": []},
            "you": {"id": "ours", "name": "ours", "health": 90,
                    "body": [{"x": 3, "y": 3}], "head": {"x": 3, "y": 3},
                    "length": 1}
        });
        let parsed: crate::models::GameState = serde_json::from_value(payload).unwrap();
        let normalized = GameState::from(&parsed);
        assert_eq!(normalized.board.snakes.len(), 1);
        assert_eq!(normalized.board.snakes[0].id, normalized.you.id);
    }
}
'''
LOGIC = r'''//! Lambda adapter for the same SnakeBollada DecisionState/FutureGraph used
//! by the native server. The Lambda handler and Terraform remain independent.
use crate::models::GameState as RequestGameState;
use crate::runtime::GameRuntime;
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::Instant;

static RUNTIME: OnceLock<GameRuntime> = OnceLock::new();
fn runtime() -> &'static GameRuntime {
    RUNTIME.get_or_init(GameRuntime::new)
}

pub fn info() -> Value {
    json!({
        "apiversion": "1",
        "author": "",
        "color": "#8B0000",
        "head": "tiger-king",
        "tail": "hook",
        "version": "ordering-v3-lambda"
    })
}
pub async fn start(input: &RequestGameState) {
    runtime().start(&crate::GameState::from(input)).await;
}
pub async fn get_move(input: &RequestGameState) -> Value {
    // Include conversion work in the request-side search deadline.
    let started = Instant::now();
    let state = crate::GameState::from(input);
    let decision = runtime().decide(&state, started).await;
    json!({"move": decision.direction.as_str()})
}
pub async fn end(input: &RequestGameState) {
    runtime().end(&crate::GameState::from(input)).await;
}
'''
CARGO = r'''[package]
name = "battlesnake"
version = "1.0.0"
edition = "2021"
description = "Ordering V3 SnakeBollada on the Maua AWS Lambda Battlesnake template"

[dependencies]
lambda_http = "1"
tokio = { version = "~1.51", features = ["macros", "rt-multi-thread", "rt", "sync", "time"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-log = "0.2"
log = "0.4"
rayon = "1.12.0"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
'''

def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--push", action="store_true", help="push isolated feature branch, never dev")
    args = parser.parse_args()
    source = Path(__file__).resolve().parents[1]
    if not (source / "src/search/hobbs_flow.rs").is_file():
        sys.exit("Run from a SnakeBollada checkout.")
    run("git", "merge-base", "--is-ancestor", SOURCE_BASE, "HEAD", cwd=source)
    dest = args.destination.resolve()
    if not dest.exists():
        run("git", "clone", DEST_REMOTE, str(dest))
    if capture("git", "status", "--porcelain", cwd=dest):
        sys.exit("Destination working tree is not clean; refusing to alter it.")
    remote = capture("git", "remote", "get-url", "origin", cwd=dest)
    if "Maua-Dev/battlesnake_luigi" not in remote:
        sys.exit(f"Unexpected target origin: {remote!r}")
    run("git", "fetch", "origin", "dev", cwd=dest)
    if BRANCH in capture("git", "branch", "--list", BRANCH, cwd=dest).splitlines():
        sys.exit(f"Branch {BRANCH} already exists; refusing to overwrite.")
    run("git", "switch", "-c", BRANCH, "origin/dev", cwd=dest)

    for dirname in CORE_DIRS:
        shutil.copytree(source / "src" / dirname, dest / "src" / dirname,
                        dirs_exist_ok=True)
    for file in CORE_FILES:
        shutil.copy2(source / "src" / file, dest / "src" / file)
    (dest / "src/v3_models.rs").write_text(MODELS, encoding="utf-8")
    (dest / "src/logic.rs").write_text(LOGIC, encoding="utf-8")
    (dest / "Cargo.toml").write_text(CARGO, encoding="utf-8")

    main_path = dest / "src/main.rs"
    main_src = main_path.read_text(encoding="utf-8")
    main_src = replace_exact(main_src, "mod logic;\nmod models;",
        """mod logic;
mod models;
mod v3_models;
mod analysis;
mod board_mask;
mod decision;
mod direction;
mod evaluation;
mod navigation;
mod runtime;
mod search;
mod simulation;
mod spatial;
mod strategy;
pub(crate) use v3_models::{Battlesnake, Board, Coord, Game, GameState};""",
        "src/main.rs")
    main_src = replace_exact(main_src, "use models::GameState;",
                             "use models::GameState as RequestGameState;", "src/main.rs")
    main_src = replace_exact(main_src,
        "Result<GameState, String>", "Result<RequestGameState, String>", "src/main.rs")
    main_src = replace_exact(main_src,
        "logic::start(&state);", "logic::start(&state).await;", "src/main.rs")
    main_src = replace_exact(main_src,
        "logic::end(&state);", "logic::end(&state).await;", "src/main.rs")
    main_src = replace_exact(main_src,
        "logic::get_move(&state)", "logic::get_move(&state).await", "src/main.rs")
    main_src = replace_exact(main_src,
        "    lambda_http::tracing::init_default_subscriber();",
        "    let _ = tracing_log::LogTracer::init();\n    lambda_http::tracing::init_default_subscriber();",
        "src/main.rs")
    main_path.write_text(main_src, encoding="utf-8")

    terraform = dest / "terraform/app/main.tf"
    tf = terraform.read_text(encoding="utf-8")
    tf = replace_exact(tf, '      RUST_LOG = "info"',
                       '      RUST_LOG = "info"\n      SNAKE_TERRITORY_MODE = "ordering_v3"',
                       "terraform/app/main.tf")
    terraform.write_text(tf, encoding="utf-8")

    run("cargo", "generate-lockfile", cwd=dest)
    run("cargo", "fmt", "--all", cwd=dest)
    run("cargo", "test", "--locked", "--all-features", cwd=dest)
    run("cargo", "build", "--locked", "--release", cwd=dest)
    run("git", "add", "Cargo.toml", "Cargo.lock", "src", "terraform/app/main.tf", cwd=dest)
    run("git", "commit", "-m", "feat(v3): stage SnakeBollada ordering V3 for Lambda", cwd=dest)
    if args.push:
        run("git", "push", "-u", "origin", BRANCH, cwd=dest)
    print(f"Prepared {BRANCH}. Merge into dev ONLY after musl CI and Lambda latency review.")

if __name__ == "__main__":
    main()
