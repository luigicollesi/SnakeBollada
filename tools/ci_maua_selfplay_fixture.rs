//! CI-only head-to-head simulation using Maua's production GameRuntime and
//! Battlesnake simultaneous-turn resolver. Injected by ci_maua_selfplay.py.
//! This is not an AWS Lambda sandbox: runtime/logic parity, not CPU equivalence.

use std::collections::HashMap;
use std::time::Instant;

use serde_json::json;
use crate::direction::Direction;
use crate::runtime::GameRuntime;
use crate::simulation::joint_action::JointAction;
use crate::simulation::resolver::resolve_turn;
use crate::simulation::state::{RulesContext, SimulatedGameState, SimulatedSnake};
use crate::{Battlesnake, Board, Coord, Game, GameState};

fn snake(id: &str, points: &[(i32, i32)]) -> SimulatedSnake {
    SimulatedSnake {
        id: id.into(), health: 100,
        body: points.iter().map(|&(x,y)| Coord { x, y }).collect(),
        alive: true,
    }
}

fn initial() -> SimulatedGameState {
    SimulatedGameState {
        turn: 0, width: 11, height: 11,
        food: vec![Coord { x: 5, y: 5 }],
        hazards: vec![],
        snakes: vec![
            snake("left-snake", &[(0,0), (0,1), (1,1)]),
            snake("right-snake", &[(10,10), (10,9), (9,9)]),
        ],
        our_snake_id: "left-snake".into(),
        rules: RulesContext {
            name: "standard".into(),
            max_health: 100,
            hazard_damage_per_turn: 0,
        },
    }
}

fn view(sim: &SimulatedGameState, id: &str) -> GameState {
    let roster: Vec<Battlesnake> = sim.snakes.iter()
        .filter(|s| s.alive)
        .map(|s| {
            let head = s.body[0];
            Battlesnake {
                id: s.id.clone(), name: s.id.clone(), health: s.health,
                body: s.body.clone(), head,
                length: s.body.len() as u32, latency: String::new(), shout: None,
            }
        })
        .collect();
    let you = roster.iter().find(|s| s.id == id).expect("alive actor").clone();
    GameState {
        game: Game {
            id: "ci-lambda-selfplay".into(),
            ruleset: HashMap::from([("name".to_string(), json!("standard"))]),
            timeout: 500,
        },
        turn: sim.turn,
        board: Board {
            height: sim.height, width: sim.width,
            food: sim.food.clone(), hazards: sim.hazards.clone(),
            snakes: roster,
        },
        you,
    }
}

#[tokio::test]
async fn ci_selfplay_sessions_are_isolated() {
    let runtime = GameRuntime::new();
    let position = initial();
    let a = view(&position, "left-snake");
    let b = view(&position, "right-snake");
    runtime.start(&a).await;
    runtime.start(&b).await;
    let a_move = runtime.decide(&a, Instant::now()).await;
    let b_move = runtime.decide(&b, Instant::now()).await;
    println!("SESSION_CHECK A={:?} B={:?} A_DEPTH={} B_DEPTH={} A_REASON={:?} B_REASON={:?}",
        a_move.direction, b_move.direction,
        a_move.search.completed_depth, b_move.search.completed_depth,
        a_move.reason, b_move.reason,
    );
    assert_eq!(a_move.direction, Direction::Right, "A's only legal move");
    assert_eq!(b_move.direction, Direction::Left, "B must not reuse A's decision");
}

#[tokio::test]
async fn ci_selfplay_limited_match() {
    let runtime = GameRuntime::new();
    let mut position = initial();
    let first_a = view(&position, "left-snake");
    let first_b = view(&position, "right-snake");
    runtime.start(&first_a).await;
    runtime.start(&first_b).await;
    let mut rounds = 0usize;
    let mut searched = 0usize;
    let mut fallback = 0usize;
    let mut min_depth = u8::MAX;
    let mut max_request_us = 0u128;
    for _ in 0..24 {
        if position.snakes.iter().filter(|s| s.alive).count() < 2 { break; }
        let a = view(&position, "left-snake");
        let b = view(&position, "right-snake");
        let started_a = Instant::now();
        let move_a = runtime.decide(&a, started_a).await;
        let elapsed_a = started_a.elapsed().as_micros();
        let started_b = Instant::now();
        let move_b = runtime.decide(&b, started_b).await;
        let elapsed_b = started_b.elapsed().as_micros();
        max_request_us = max_request_us.max(elapsed_a).max(elapsed_b);
        for decision in [&move_a, &move_b] {
            if matches!(decision.reason, crate::strategy::DecisionReason::HobbsSearch) {
                searched += 1;
                min_depth = min_depth.min(decision.search.completed_depth);
            } else { fallback += 1; }
        }
        println!("ROUND={:02} A={:?} B={:?} depthA={} depthB={} reasonA={:?} reasonB={:?} nodesA={} nodesB={} usA={} usB={}",
            position.turn, move_a.direction, move_b.direction,
            move_a.search.completed_depth, move_b.search.completed_depth,
            move_a.reason, move_b.reason,
            move_a.search.nodes, move_b.search.nodes, elapsed_a, elapsed_b);
        if rounds == 0 {
            assert_eq!(move_a.direction, Direction::Right);
            assert_eq!(move_b.direction, Direction::Left);
        }
        let a_actor = position.actor_index("left-snake").unwrap();
        let b_actor = position.actor_index("right-snake").unwrap();
        let joint = JointAction::new()
            .with_move(a_actor, move_a.direction)
            .with_move(b_actor, move_b.direction);
        let result = resolve_turn(&position, &joint).expect("real simulator");
        position = result.state;
        rounds += 1;
    }
    let alive: Vec<&str> = position.snakes.iter()
        .filter(|s| s.alive).map(|s| s.id.as_str()).collect();
    println!("MATCH_SUMMARY rounds={} survivors={:?} searched={} fallback={} min_depth={} max_request_us={} mem_kib={}",
        rounds, alive, searched, fallback,
        if min_depth == u8::MAX {0} else {min_depth},
        max_request_us,
        std::fs::read_to_string("/proc/self/status")
            .ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).map(str::to_string))
            .unwrap_or_default(),
    );
    assert!(rounds >= 1, "the simulation must execute at least one joint turn");
    runtime.end(&first_a).await;
    runtime.end(&first_b).await;
}
