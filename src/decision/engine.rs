use crate::decision::state_key::StateKey;
use crate::simulation::state::SimulatedGameState;
use crate::strategy::{choose_move_baseline, Decision};
use crate::GameState;

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DecisionEngine;

impl DecisionEngine {
    pub(crate) fn stateless() -> Self {
        Self
    }

    pub(crate) fn decide(&self, state: &GameState) -> Decision {
        let normalized = SimulatedGameState::from(state);
        let _root_key = StateKey::from_state(&normalized);

        choose_move_baseline(state)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::*;
    use crate::{Battlesnake, Board, Coord, Game};

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

    #[test]
    fn shell_preserves_baseline_decision() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 2, y: 2 },
                Coord { x: 2, y: 1 },
                Coord { x: 2, y: 0 },
            ],
        );
        let state = GameState {
            game: Game {
                id: "decision-shell".to_string(),
                ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
                timeout: 500,
            },
            turn: 2,
            board: Board {
                width: 7,
                height: 7,
                food: vec![Coord { x: 4, y: 2 }],
                snakes: vec![ours.clone()],
                hazards: vec![],
            },
            you: ours,
        };

        assert_eq!(
            DecisionEngine::stateless().decide(&state),
            choose_move_baseline(&state)
        );
    }
}
