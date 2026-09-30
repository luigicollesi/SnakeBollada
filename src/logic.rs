use log::info;
use serde_json::{json, Value};

use crate::{Battlesnake, Board, Game, GameState};
use crate::strategy;

pub(crate) fn info() -> Value {
    info!("INFO");

    json!({
        "apiversion": "1",
        "author": "luigicollesi",
        "color": "#888888",
        "head": "default",
        "tail": "default",
    })
}

pub(crate) fn start(_game: &Game, _turn: &i32, _board: &Board, _you: &Battlesnake) {
    info!("GAME START");
}

pub(crate) fn end(_game: &Game, _turn: &i32, _board: &Board, _you: &Battlesnake) {
    info!("GAME OVER");
}

pub(crate) fn get_move(state: &GameState) -> Value {
    let decision = strategy::choose_move(state);

    info!(
        "MOVE {}: {} ({:?})",
        state.turn,
        decision.direction.as_str(),
        decision.reason
    );

    json!({ "move": decision.direction.as_str() })
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
