#!/usr/bin/env python3
"""Apply CPU-limited AWS Lambda V3 fixes, without touching Terraform or pushing.

From a checkout of SnakeBollada:
  python3 scripts/tune_maua_v3_lambda.py ../battlesnake_luigi

The destination MUST be on a non-deployment branch. This script edits local
Rust files only: the caller must run cargo fmt/test/build and commit manually.
"""
from pathlib import Path
import argparse
import subprocess

def git(repo, *args):
    return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()

def replace_once(value, old, new, path):
    count = value.count(old)
    if count != 1:
        raise RuntimeError(f"{path}: expected exactly one occurrence; found {count}. No files changed.")
    return value.replace(old, new, 1)

FALLBACK = r'''pub(crate) fn choose_move_baseline(state: &GameState) -> Decision {
    // The time-budget fallback must avoid a provable body collision BEFORE
    // ranking food. It is used whenever the Lambda cannot finish MAX/MIN.
    let simulated = crate::simulation::state::SimulatedGameState::from(state);
    let mobility = crate::simulation::mobility::MobilityAnalysis::from_state(&simulated);
    let deterministic = simulated
        .snake(&simulated.our_snake_id)
        .map(|ours| {
            Direction::ALL
                .into_iter()
                .filter(|direction| mobility.classify_move(&simulated, ours, *direction).is_none())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if deterministic.is_empty() {
        let direction = Direction::ALL
            .into_iter()
            .filter(|direction| direction_stays_in_bounds(state, *direction))
            .min_by_key(|direction| direction.rank())
            .unwrap_or(Direction::Up);
        return Decision {
            direction,
            reason: DecisionReason::NoSafeMove,
            reachable_cells: 0,
            search: SearchMetadata::default(),
        };
    }

    let map = NavigationMap::from_state(state);
    let best = deterministic
        .iter()
        .copied()
        .map(|direction| {
            let destination = direction.apply(state.you.head);
            let reachable = mobility.reachable_space(&simulated, &simulated.our_snake_id, direction);
            let contested_head = map.lethal_head_danger.contains(destination);
            let cramped = reachable < state.you.length;
            let hazard = map.is_hazard(destination);
            let food_distance = state
                .board
                .food
                .iter()
                .map(|food| destination.x.abs_diff(food.x) + destination.y.abs_diff(food.y))
                .min()
                .unwrap_or(u32::MAX);
            (direction, reachable, contested_head, cramped, hazard, food_distance)
        })
        .min_by_key(|(direction, reachable, contested_head, cramped, hazard, food_distance)| {
            (
                *contested_head,
                *cramped,
                *hazard,
                std::cmp::Reverse(*reachable),
                *food_distance,
                direction.rank(),
            )
        })
        .expect("at least one deterministic move");
    Decision {
        direction: best.0,
        reason: if deterministic.len() == 1 {
            DecisionReason::OnlyLegalMove
        } else {
            DecisionReason::BaselineFallback
        },
        reachable_cells: best.1,
        search: SearchMetadata::default(),
    }
}

'''

def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("destination", type=Path)
    args = ap.parse_args()
    root = args.destination.resolve()
    if git(root, "branch", "--show-current") in ("", "dev", "main", "homolog", "prod"):
        raise SystemExit("Refusing to edit a deployment branch. Create a fix/* branch first.")
    if "Maua-Dev/battlesnake_luigi" not in git(root, "remote", "get-url", "origin"):
        raise SystemExit("Wrong destination repository.")
    if not (root / "src/search/hobbs_flow.rs").is_file():
        raise SystemExit("Missing V3 engine. Migrate the V3 first.")

    files = {}
    def apply(path, old, new):
        current = files.get(path)
        if current is None:
            current = (root / path).read_text()
        files[path] = replace_once(current, old, new, path)

    strategy = "src/strategy.rs"
    raw = (root / strategy).read_text()
    start = raw.index("pub(crate) fn choose_move_baseline(state: &GameState) -> Decision {")
    end = raw.index("\n#[cfg(test)]\nmod tests {", start)
    files[strategy] = raw[:start] + FALLBACK + raw[end:]
    files[strategy] = files[strategy].replace(
        "use crate::navigation::{reachable_after_move, NavigationMap};",
        "use crate::navigation::NavigationMap;", 1)
    strategy_test = r'''
    #[test]
    fn lambda_fallback_rejects_food_in_one_cell_body_trap() {
        let ours = snake(
            "ours",
            vec![
                Coord { x: 3, y: 3 }, Coord { x: 3, y: 2 },
                Coord { x: 3, y: 1 }, Coord { x: 2, y: 1 },
                Coord { x: 1, y: 1 }, Coord { x: 0, y: 1 },
            ],
        );
        // (4,3) is a one-cell pocket; food must not outweigh a free region.
        let enemy = snake(
            "enemy",
            vec![
                Coord { x: 5, y: 3 }, Coord { x: 5, y: 4 },
                Coord { x: 4, y: 4 }, Coord { x: 4, y: 2 },
                Coord { x: 5, y: 2 },
            ],
        );
        let board = state(ours, vec![enemy], vec![Coord { x: 4, y: 3 }]);
        let decision = choose_move_baseline(&board);
        assert_ne!(decision.direction, Direction::Right);
        assert!(matches!(decision.direction, Direction::Up | Direction::Left));
    }
'''
    if not files[strategy].rstrip().endswith("}"):
        raise RuntimeError("Unexpected strategy test-module ending")
    last = files[strategy].rfind("\n}")
    files[strategy] = files[strategy][:last] + "\n" + strategy_test + files[strategy][last:]

    runtime = "src/runtime.rs"
    apply(runtime,
          '''        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&state.game.id).cloned()
        };
        if session.is_none() {
            warn!(
                "received /move without active session for {}; using stateless search",
                state.game.id
            );
        }
''',
          '''        // Lambda invocations can be routed to different warm environments.
        // A /move without /start must still establish a reusable per-game graph.
        let session = {
            let mut sessions = self.sessions.write().await;
            if !sessions.contains_key(&state.game.id) {
                warn!(
                    "MOVE {} game={} lazy_session=true (no /start on this instance)",
                    state.turn, state.game.id
                );
            }
            Some(
                sessions
                    .entry(state.game.id.clone())
                    .or_insert_with(|| Arc::new(GameSession::default()))
                    .clone(),
            )
        };
''')
    runtime_test = r'''
    #[tokio::test]
    async fn lambda_move_without_start_creates_reusable_warm_session() {
        let runtime = GameRuntime::new();
        let board = state("lazy-game");
        let _first = runtime.decide(&board, Instant::now()).await;
        assert_eq!(runtime.active_game_ids().await, vec!["lazy-game"]);
    }
'''
    if not files[runtime].rstrip().endswith("}"):
        raise RuntimeError("Unexpected runtime test-module ending")
    last = files[runtime].rfind("\n}")
    files[runtime] = files[runtime][:last] + "\n" + runtime_test + files[runtime][last:]

    logic = "src/logic.rs"
    apply(logic,
          '''pub async fn get_move(input: &RequestGameState) -> Value {
    // Include conversion work in the request-side search deadline.
    let started = Instant::now();
    let state = crate::GameState::from(input);
    let decision = runtime().decide(&state, started).await;
    json!({"move": decision.direction.as_str()})
}''',
          '''pub async fn get_move(input: &RequestGameState, started: Instant) -> Value {
    // Stopwatch began BEFORE API payload parsing in the Lambda handler.
    let state = crate::GameState::from(input);
    let mut decision = runtime().decide(&state, started).await;
    // Never return a provable immediate body/wall collision if another
    // deterministic move exists, including after a timeout fallback.
    let simulation = crate::simulation::state::SimulatedGameState::from(&state);
    if let Some(ours) = simulation.snake(&simulation.our_snake_id) {
        let mobility = crate::simulation::mobility::MobilityAnalysis::from_state(&simulation);
        if mobility.classify_move(&simulation, ours, decision.direction).is_some() {
            let fallback = crate::strategy::choose_move_baseline(&state);
            if mobility.classify_move(&simulation, ours, fallback.direction).is_none() {
                log::warn!(
                    "lambda_guard game={} turn={} invalid_direction={:?} replacement={:?}",
                    state.game.id, state.turn, decision.direction, fallback.direction
                );
                decision = fallback;
            }
        }
    }
    log::info!(
        "lambda_move game={} turn={} direction={:?} reason={:?} depth={} nodes={} search_us={} request_us={}",
        state.game.id,
        state.turn,
        decision.direction,
        decision.reason,
        decision.search.completed_depth,
        decision.search.nodes,
        decision.search.elapsed_us,
        started.elapsed().as_micros()
    );
    json!({"move": decision.direction.as_str()})
}''')
    main_file = "src/main.rs"
    apply(main_file,
          "use tracing::error;\n",
          "use tracing::error;\nuse std::time::Instant;\n")
    apply(main_file,
          '''        "/move" => match parse_state(&event) {
            Ok(state) => json_response(200, logic::get_move(&state).await),
            Err(reason) => bad_request(reason),
        },''',
          '''        "/move" => {
            let started = Instant::now();
            match parse_state(&event) {
                Ok(state) => json_response(200, logic::get_move(&state, started).await),
                Err(reason) => bad_request(reason),
            }
        },''')
    # Atomic with respect to in-memory transformations: do not edit if checks fail.
    for path, value in files.items():
        (root / path).write_text(value)
    print(f"Prepared {len(files)} code-only files on {git(root, 'branch', '--show-current')}.")
    print("Run cargo fmt, cargo test --locked --all-features, cargo build --locked --release.")
    print("No terraform or AWS config changed; no push was performed.")

if __name__ == "__main__":
    main()
