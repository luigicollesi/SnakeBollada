# SnakeBollada

Battlesnake in Rust with a single state-based adversarial decision flow inspired by **Hovering Hobbs**, running on a persistent **FutureGraph**.

The active strategy does **not** include separate Food or Hunting modes, strategy weights, legacy Beam/Maximin scorers, or opponent-intent profiles. Food growth and enemy elimination are evaluated as consequences of simulated states.

## Runtime and stack

- Rust 1.98.1; Rocket 0.5.1; Battlesnake API v1
- Docker and Dockerfile.vercel
- In-memory independent sessions keyed by game ID, cleared on `/end`
- No database, file persistence, or outbound telemetry in the move path
- **Primary on `dev`: cached territorial root ordering** (`ordering`), selected automatically when no mode is specified

## Active move pipeline

```text
POST /move
  -> normalize board and ruleset
  -> reconcile previous FutureGraph / invalidate on unexpected food
  -> enumerate legal simultaneous actions for every living snake
  -> resolve official turn rules / cache transpositions
  -> iterative paranoid MAX(our move) / MIN(enemy responses)
       root: tactical fallback + cached territorial ordering (primary)
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

## Primary root ordering: architecture and development baseline

The **original `ordering` mode** is the primary decision path on the development
branch. It won all three games in the [2026-10-09 pinned-seed Hobbs benchmark](https://github.com/luigicollesi/SnakeBollada/actions/runs/38016396600)
(seeds `20261002`, `20261003`, `20261004`). In that same run,
`ordering_v2` won two and `shadow` won one. This is a **single three-seed run**,
not statistically conclusive: wall-clock iterative deepening can produce
different trajectories across repeated runs of the same seed.

The decision path for `ordering` is:

1. `DecisionEngine` normalizes the request and obtains the persistent `FutureGraph`.
2. `search_hobbs` runs iterative-deepening paranoid MAX/MIN over joint actions,
   using the **last fully completed search depth** when the deadline arrives.
3. At the root, the tactical baseline orders directions by known mobility
   continuations, enemy continuations, the preferred cached direction and
   a deterministic direction tie-break.
4. Beginning at depth two, `TerritoryDirectionSample::from_cached_replies`
   samples at most two cached opponent replies per direction, following up
   to four cached states; the search-scoped `TerritorySnapshot` cache caps
   new snapshots at 24. A 2 ms budget limits sampling **per eligible depth**.
5. Only if **every** root direction has a sample, order primarily by the
   *minimum sampled mean territorial margin* and *minimum sampled margin*,
   then tactical tie-breaks. Otherwise the original tactical order is kept.
6. MIN still considers replies required by the search, and `RouteRank`,
   `compare_routes`, terminal scoring and `Survival Guard` are unchanged.
   Sampled territory is an **ordering heuristic**, not a guaranteed MIN bound.
7. Optional escape/adversarial diagnostics run **after** move selection;
   they do not override the chosen move.

This is the **evolution anchor**: first improve the quality and coverage of
these root samples while preserving the same selection and scoring semantics,
then benchmark against the pinned primary revision before accepting changes.

### Search mode overrides

| `SNAKE_TERRITORY_MODE` | Behavior |
| --- | --- |
| unset or `ordering` | **Primary:** cached territorial ordering |
| `off` | Original baseline tactical ordering, without shadow diagnostics |
| `shadow` | Original baseline with post-search diagnostics |
| `ordering_v2` | Conservative experimental reordering, opt-in only |
| `ordering_v3` | Experimental territorial-drop tiebreak, using cached consecutive snapshots |

The default above is implemented on `dev`; this change does not merge to
`main` or deploy a new production build.

```bash
# Primary (default):
cargo run --release

# Explicitly compare against the historical baseline:
SNAKE_TERRITORY_MODE=off cargo run --release
```

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

### Experimental territorial-collapse hint (`ordering_v3`)

`ordering` remains the validated default. V3 compares raw Hobbs `claimed_cells`
between consecutive states of the **same cached reply chain**, recording absolute
and relative drops, opponent gain, first drop ply, and whether control recovers
on the next observed ply. This is a diagnostic of lost *predicted* control, not
a proof of a blocked gateway or an exhaustive MIN forecast. Only large drops
(at least 8 cells and 40%) without observed recovery, combined with at most
two cached continuations, affect ordering, and only inside a fixed
50-milli territorial-margin bucket. Missing evidence preserves the original
tactical ordering. This feature is opt-in; `HobbsScore`, `RouteRank`, opponent
replies, and iterative-deepening search are unchanged.

Production `ordering` no longer runs post-decision Shadow or V2 verification.
Hobbs leaf territory snapshots are reused by the root ordering sampler, avoiding
a second flood-fill when the state was already evaluated. The diagnostic
`shadow` and `ordering_v2` modes remain explicit comparison options.
