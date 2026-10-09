# SnakeBollada

Battlesnake in Rust with a single state-based adversarial decision flow inspired by **Hovering Hobbs**, running on a persistent **FutureGraph**.

The active strategy does **not** include separate Food or Hunting modes, strategy weights, legacy Beam/Maximin scorers, or opponent-intent profiles. Food growth and enemy elimination are evaluated as consequences of simulated states.

## Runtime and stack

- Rust 1.98.1; Rocket 0.5.1; Battlesnake API v1
- Docker and Dockerfile.vercel
- In-memory independent sessions keyed by game ID, cleared on `/end`
- No database, file persistence, or outbound telemetry in the move path

## Active move pipeline

```text
POST /move
  -> normalize board and ruleset
  -> reconcile previous FutureGraph / invalidate on unexpected food
  -> enumerate legal simultaneous actions for every living snake
  -> resolve official turn rules / cache transpositions
  -> iterative paranoid MAX(our move) / MIN(enemy responses)
       leaf: terminal result > Hobbs territory/length/health evaluation
       guard: structurally blocked or threatened escape paths
  -> use last fully completed search depth before the deadline
  -> keep chosen root direction and relevant future nodes
  -> return a Battlesnake direction
```

### Evaluation

- Tail-aware competing territory flood fill (up to 12 cycles).
- Empty/food/hazard cells weighted 5/20/1.
- Territory ratio plus relative length bonus `160 milli × clamp(our length − longest enemy, −3, 3)`.
- Critical-health policy prioritizes access to known food (health below 60 in duels, 85 with 3+ living snakes).
- Terminal wins/losses take priority over ordinary territorial score.
- The **Survival Guard** is a categorical check for trapped/contested escape routes, **not** a Food/Hunting/Survival point budget.
- Future unknown food spawns are marked provisional rather than fabricated.

The search is an iterative, budgeted paranoid minimax on the FutureGraph, **not an exact copy of Hobbs' complete minimax implementation**. The standard ruleset uses this flow exclusively. Unsupported rulesets or exhausted search budgets use the simple safety-oriented fallback.

## Run locally

```bash
cargo run
```

Server: `http://localhost:8000`.

## API

- `GET /`
- `POST /start`
- `POST /move`
- `POST /end`

See [Battlesnake API](https://docs.battlesnake.com/api).

## Docker

```bash
docker build -t snake-bollada .
docker run --rm -p 8000:8000 snake-bollada
```

## Local test game

```bash
battlesnake play -W 11 -H 11 --name "SnakeBollada" --url http://localhost:8000 -g solo --browser
```

## CI

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

Regression tests include recorded Hovering Hobbs positions, territory release/ties, growth, collision resolution, and corridor safety. Winning a duel is **not** guaranteed by a passing CI; tournament benchmarks must be run separately.

## Attribution

The project began from the MIT-licensed [Battlesnake Rust Starter Project](https://github.com/BattlesnakeOfficial/starter-snake-rust). The decision evaluator is independently adapted from ideas used in Hovering Hobbs.
