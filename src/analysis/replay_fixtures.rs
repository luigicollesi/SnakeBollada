//! Captured states from the LOCAL GitHub Actions duel, seed 20261003.
//! Actual body order, health and food are retained; no invented geometry.
use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
use crate::Coord;

type RecordedCoordinates<'a> = (
    i32,
    i32,
    &'a [(i32, i32)],
    &'a [(i32, i32)],
    &'a [(i32, i32)],
);

pub(super) fn state(turn: i32) -> SimulatedGameState {
    let (our_health, enemy_health, ours, enemy, food): RecordedCoordinates<'_> = match turn {
        231 => (
            100,
            88,
            &[
                (10, 3),
                (9, 3),
                (9, 4),
                (8, 4),
                (7, 4),
                (6, 4),
                (6, 5),
                (7, 5),
                (8, 5),
                (8, 6),
                (7, 6),
                (7, 7),
                (6, 7),
                (6, 8),
                (5, 8),
                (4, 8),
                (3, 8),
                (2, 8),
                (2, 7),
                (3, 7),
                (3, 7),
            ],
            &[
                (0, 1),
                (0, 0),
                (1, 0),
                (2, 0),
                (2, 1),
                (3, 1),
                (3, 2),
                (4, 2),
                (4, 1),
                (4, 0),
                (5, 0),
                (5, 1),
                (5, 2),
                (6, 2),
                (6, 1),
                (7, 1),
                (7, 2),
                (7, 3),
                (8, 3),
                (8, 2),
                (8, 1),
            ],
            &[(1, 1), (6, 0), (7, 10), (0, 10), (4, 7)],
        ),
        232 => (
            99,
            100,
            &[
                (10, 2),
                (10, 3),
                (9, 3),
                (9, 4),
                (8, 4),
                (7, 4),
                (6, 4),
                (6, 5),
                (7, 5),
                (8, 5),
                (8, 6),
                (7, 6),
                (7, 7),
                (6, 7),
                (6, 8),
                (5, 8),
                (4, 8),
                (3, 8),
                (2, 8),
                (2, 7),
                (3, 7),
            ],
            &[
                (1, 1),
                (0, 1),
                (0, 0),
                (1, 0),
                (2, 0),
                (2, 1),
                (3, 1),
                (3, 2),
                (4, 2),
                (4, 1),
                (4, 0),
                (5, 0),
                (5, 1),
                (5, 2),
                (6, 2),
                (6, 1),
                (7, 1),
                (7, 2),
                (7, 3),
                (8, 3),
                (8, 2),
                (8, 2),
            ],
            &[(6, 0), (7, 10), (0, 10), (4, 7), (4, 10)],
        ),
        235 => (
            96,
            97,
            &[
                (9, 0),
                (10, 0),
                (10, 1),
                (10, 2),
                (10, 3),
                (9, 3),
                (9, 4),
                (8, 4),
                (7, 4),
                (6, 4),
                (6, 5),
                (7, 5),
                (8, 5),
                (8, 6),
                (7, 6),
                (7, 7),
                (6, 7),
                (6, 8),
                (5, 8),
                (4, 8),
                (3, 8),
            ],
            &[
                (1, 4),
                (1, 3),
                (1, 2),
                (1, 1),
                (0, 1),
                (0, 0),
                (1, 0),
                (2, 0),
                (2, 1),
                (3, 1),
                (3, 2),
                (4, 2),
                (4, 1),
                (4, 0),
                (5, 0),
                (5, 1),
                (5, 2),
                (6, 2),
                (6, 1),
                (7, 1),
                (7, 2),
                (7, 3),
            ],
            &[(6, 0), (7, 10), (0, 10), (4, 7), (4, 10)],
        ),
        241 => (
            97,
            91,
            &[
                (6, 3),
                (6, 2),
                (6, 1),
                (6, 0),
                (7, 0),
                (8, 0),
                (9, 0),
                (10, 0),
                (10, 1),
                (10, 2),
                (10, 3),
                (9, 3),
                (9, 4),
                (8, 4),
                (7, 4),
                (6, 4),
                (6, 5),
                (7, 5),
                (8, 5),
                (8, 6),
                (7, 6),
                (7, 7),
            ],
            &[
                (4, 5),
                (4, 4),
                (3, 4),
                (2, 4),
                (2, 5),
                (1, 5),
                (1, 4),
                (1, 3),
                (1, 2),
                (1, 1),
                (0, 1),
                (0, 0),
                (1, 0),
                (2, 0),
                (2, 1),
                (3, 1),
                (3, 2),
                (4, 2),
                (4, 1),
                (4, 0),
                (5, 0),
                (5, 1),
            ],
            &[(7, 10), (0, 10), (4, 7), (4, 10), (0, 8)],
        ),
        242 => (
            96,
            90,
            &[
                (7, 3),
                (6, 3),
                (6, 2),
                (6, 1),
                (6, 0),
                (7, 0),
                (8, 0),
                (9, 0),
                (10, 0),
                (10, 1),
                (10, 2),
                (10, 3),
                (9, 3),
                (9, 4),
                (8, 4),
                (7, 4),
                (6, 4),
                (6, 5),
                (7, 5),
                (8, 5),
                (8, 6),
                (7, 6),
            ],
            &[
                (4, 6),
                (4, 5),
                (4, 4),
                (3, 4),
                (2, 4),
                (2, 5),
                (1, 5),
                (1, 4),
                (1, 3),
                (1, 2),
                (1, 1),
                (0, 1),
                (0, 0),
                (1, 0),
                (2, 0),
                (2, 1),
                (3, 1),
                (3, 2),
                (4, 2),
                (4, 1),
                (4, 0),
                (5, 0),
            ],
            &[(7, 10), (0, 10), (4, 7), (4, 10), (0, 8), (5, 5)],
        ),
        _ => panic!("unrecorded turn"),
    };
    let convert = |points: &[(i32, i32)]| {
        points
            .iter()
            .map(|&(x, y)| Coord { x, y })
            .collect::<Vec<_>>()
    };
    SimulatedGameState {
        turn,
        width: 11,
        height: 11,
        food: convert(food),
        hazards: Vec::new(),
        snakes: vec![
            SimulatedSnake {
                id: "ours".into(),
                health: our_health,
                body: convert(ours),
                alive: true,
            },
            SimulatedSnake {
                id: "hobbs".into(),
                health: enemy_health,
                body: convert(enemy),
                alive: true,
            },
        ],
        our_snake_id: "ours".into(),
        rules: RulesContext {
            name: "standard".into(),
            max_health: 100,
            hazard_damage_per_turn: 0,
        },
    }
}

#[test]
fn recorded_lengths_and_heads_are_consistent() {
    for turn in [231, 232, 235, 241, 242] {
        let s = state(turn);
        assert_eq!(s.snakes.len(), 2);
        assert!(s.snakes.iter().all(|snake| snake.alive && snake.health > 0));
        assert_eq!(s.snakes[0].body.len(), if turn < 241 { 21 } else { 22 });
        assert_eq!(s.snakes[1].body.len(), if turn == 231 { 21 } else { 22 });
    }
    assert_eq!(state(241).snakes[0].head(), Some(Coord { x: 6, y: 3 }));
    assert_eq!(state(242).snakes[0].head(), Some(Coord { x: 7, y: 3 }));
}
/// Late-game checkpoints copied from the native GitHub Actions replay:
/// run 38008513012, shadow mode, seed 20261003. Each packed pair is (x,y)
/// in hexadecimal-like 0..9,A notation, preserving head-to-tail body order.
#[cfg(test)]
pub(crate) fn late_seed_20261003(turn: i32) -> SimulatedGameState {
    let (our_health, hobbs_health, ours, hobbs, food) = match turn {
        323 => (
            70,
            79,
            "92939495968685757473838272626364655545445453525141423222",
            "272625243435363747465657586867667677787969595A6A7A8A8988879798",
            "486011A114",
        ),
        325 => (
            100,
            77,
            "A1A2929394959686857574738382726263646555454454535251414242",
            "1617272625243435363747465657586867667677787969595A6A7A8A898887",
            "4860111438",
        ),
        326 => (
            99,
            76,
            "A0A1A29293949596868575747383827262636465554544545352514142",
            "151617272625243435363747465657586867667677787969595A6A7A8A8988",
            "4860111438",
        ),
        328 => (
            97,
            99,
            "9190A0A1A2929394959686857574738382726263646555454454535251",
            "1314151617272625243435363747465657586867667677787969595A6A7A8A89",
            "4860113800",
        ),
        332 => (
            100,
            95,
            "607071819190A0A1A2929394959686857574738382726263646555454444",
            "424333231314151617272625243435363747465657586867667677787969595A",
            "48113800",
        ),
        333 => (
            99,
            94,
            "61607071819190A0A1A29293949596868575747383827262636465554544",
            "5242433323131415161727262524343536374746565758686766767778796959",
            "48113800",
        ),
        _ => panic!("late-game checkpoint was not recorded"),
    };
    let decode = |encoded: &str| -> Vec<Coord> {
        let bytes = encoded.as_bytes();
        assert_eq!(bytes.len() % 2, 0, "one pair per board coordinate");
        let digit = |ch: u8| match ch {
            b'0'..=b'9' => i32::from(ch - b'0'),
            b'A' => 10,
            _ => panic!("invalid replay coordinate"),
        };
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| Coord {
                x: digit(pair[0]),
                y: digit(pair[1]),
            })
            .collect()
    };
    SimulatedGameState {
        turn,
        width: 11,
        height: 11,
        food: decode(food),
        hazards: Vec::new(),
        snakes: vec![
            SimulatedSnake {
                id: "ours".to_owned(),
                health: our_health,
                body: decode(ours),
                alive: true,
            },
            SimulatedSnake {
                id: "hobbs".to_owned(),
                health: hobbs_health,
                body: decode(hobbs),
                alive: true,
            },
        ],
        our_snake_id: "ours".to_owned(),
        rules: RulesContext {
            name: "standard".to_owned(),
            max_health: 100,
            hazard_damage_per_turn: 14,
        },
    }
}

#[test]
fn late_seed_20261003_checkpoints_preserve_head_body_and_health() {
    for (turn, ours, hobbs, our_health, hobbs_health) in [
        (323, (9, 2), (2, 7), 70, 79),
        (325, (10, 1), (1, 6), 100, 77),
        (326, (10, 0), (1, 5), 99, 76),
        (328, (9, 1), (1, 3), 97, 99),
        (332, (6, 0), (4, 2), 100, 95),
        (333, (6, 1), (5, 2), 99, 94),
    ] {
        let state = late_seed_20261003(turn);
        assert_eq!(state.snakes.len(), 2);
        assert_eq!(
            state.snakes[0].head(),
            Some(Coord {
                x: ours.0,
                y: ours.1
            })
        );
        assert_eq!(
            state.snakes[1].head(),
            Some(Coord {
                x: hobbs.0,
                y: hobbs.1
            })
        );
        assert_eq!(state.snakes[0].health, our_health);
        assert_eq!(state.snakes[1].health, hobbs_health);
        assert!(state.snakes.iter().all(|snake| snake.body.len() >= 28));
        assert!(state.food.iter().all(|cell| cell.x < 11 && cell.y < 11));
    }
}

#[test]
fn late_seed_20261003_final_contested_exit_is_a_losing_head_to_head() {
    use crate::direction::Direction;
    use crate::simulation::joint_action::JointAction;
    use crate::simulation::resolver::resolve_turn;

    let state = late_seed_20261003(333);
    let us = state.actor_index("ours").unwrap();
    let rival = state.actor_index("hobbs").unwrap();
    assert!(state.snake("ours").unwrap().length() < state.snake("hobbs").unwrap().length());
    let action = JointAction::new()
        .with_move(us, Direction::Left)
        .with_move(rival, Direction::Up);
    let next = resolve_turn(&state, &action).unwrap().state;
    assert!(!next.snake("ours").unwrap().alive);
    assert!(next.snake("hobbs").unwrap().alive);
}
