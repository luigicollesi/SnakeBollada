// This file is the starting point for SnakeBollada's decision engine.
//
// It intentionally begins close to the official Battlesnake Rust starter.
// We will evolve this incrementally and benchmark each strategy change.

use log::info;
use rand::seq::IndexedRandom;
use serde_json::{json, Value};
use std::collections::HashMap;

use crate::{Battlesnake, Board, Game};

pub fn info() -> Value {
    info!("INFO");

    json!({
        "apiversion": "1",
        "author": "luigicollesi",
        "color": "#888888",
        "head": "default",
        "tail": "default",
    })
}

pub fn start(_game: &Game, _turn: &i32, _board: &Board, _you: &Battlesnake) {
    info!("GAME START");
}

pub fn end(_game: &Game, _turn: &i32, _board: &Board, _you: &Battlesnake) {
    info!("GAME OVER");
}

pub fn get_move(_game: &Game, turn: &i32, _board: &Board, you: &Battlesnake) -> Value {
    let mut is_move_safe: HashMap<_, _> = vec![
        ("up", true),
        ("down", true),
        ("left", true),
        ("right", true),
    ]
    .into_iter()
    .collect();

    // Starter behavior: never reverse directly into the neck.
    let my_head = &you.body[0];
    let my_neck = &you.body[1];

    if my_neck.x < my_head.x {
        is_move_safe.insert("left", false);
    } else if my_neck.x > my_head.x {
        is_move_safe.insert("right", false);
    } else if my_neck.y < my_head.y {
        is_move_safe.insert("down", false);
    } else if my_neck.y > my_head.y {
        is_move_safe.insert("up", false);
    }

    // Next implementation steps:
    // 1. Prevent out-of-bounds moves.
    // 2. Prevent collisions with our own body.
    // 3. Prevent collisions with other Battlesnakes.
    // 4. Account for head-to-head threats.
    // 5. Evaluate reachable space.

    let safe_moves = is_move_safe
        .into_iter()
        .filter(|&(_, safe)| safe)
        .map(|(direction, _)| direction)
        .collect::<Vec<_>>();

    let chosen = safe_moves
        .choose(&mut rand::rng())
        .expect("at least one non-reversing move should exist");

    info!("MOVE {}: {}", turn, chosen);
    json!({ "move": chosen })
}

#[cfg(test)]
mod tests {
    use super::info;

    #[test]
    fn info_uses_battlesnake_api_v1() {
        let metadata = info();
        assert_eq!(metadata["apiversion"], "1");
        assert_eq!(metadata["author"], "luigicollesi");
    }
}
