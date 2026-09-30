# Hunting Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Hunting Mode as a stateless tactical-analysis layer that helps the in-progress Decision/Search Engine find lines that force, trap, or eliminate opponents without creating a second tree or duplicating simulation, pathfinding, Enemy Tracing, or final scoring.

**Architecture:** Hunting consumes `SimulatedGameState`, centralized `StateAnalysis`, and one-step `EnemyTracingOutput`. It produces target ordering and tactical hints for Decision search ordering, then derives edge-local `EnemyForced`/`EnemyTrapped` events by comparing parent and child tracing after the existing `resolve_turn` has resolved simultaneous movement. Confirmed kills/head-to-head outcomes remain owned by the existing TurnResolver; Survival priority, aggression weighting, route utility, cache, depth, and final move selection remain owned by Decision Making.

**Tech Stack:** Rust 1.98.1, existing `BoardMask`, `Direction`/`MoveMask`, `src/simulation/*`, centralized BFS analysis, standard library only.

**Spec:** `docs/specs/hunting-mode.md`, integrated with `docs/specs/decision-making.md` and `docs/specs/enemy-tracing.md`.

## Current Decision foundation already present on dev

Hunting must reuse, not replace:

- `src/simulation/state.rs::SimulatedGameState`
- `src/simulation/state.rs::SimulatedSnake`
- `src/simulation/state.rs::AggressionState`
- `src/simulation/resolver.rs::JointAction`
- `src/simulation/resolver.rs::resolve_turn`
- `src/simulation/resolver.rs::TurnResolution`
- `src/simulation/resolver.rs::InstantEvent`
- `src/simulation/resolver.rs::EliminationCause`
- `src/simulation/resolver.rs::ForecastDelta`

The simulation module is currently private internally; Hunting integration should use crate-level re-exports rather than importing deep private implementation paths everywhere.

## Strategy basis

- Battlesnake's official useful-algorithms guide explicitly identifies pathfinding toward another snake as an attack primitive and Flood Fill as a way to reason about available space.
- Competitive Tron agents commonly target contested points and reduce opponent mobility/territory rather than simply chase the head.
- Articulation points can detect choke cells in `O(V + E)`; this is useful as a search-order hint, not as a guaranteed reward.
- Because Battlesnake resolves moves simultaneously, Hunting must never infer a kill from geometric overlap alone; `resolve_turn` is authoritative.

## Global Constraints

- Hunting MUST NOT own a FutureGraph.
- Hunting MUST NOT recurse through future turns itself.
- Hunting MUST NOT run its own Snake→Food BFS.
- Hunting MUST NOT implement a second turn resolver.
- Hunting MUST NOT apply aggression weights.
- Hunting MUST NOT apply Survival weights.
- Hunting MUST NOT calculate final `benefit - harm` route utility.
- Hunting MUST NOT decide border/corner reserved-cell exceptions.
- Hunting MUST NOT choose final movement.
- Hunting MUST NOT decide depth, timeout, iterative deepening, or cache reuse.
- `EnemyKilled`, `HeadToHeadWon`, `HeadToHeadLost`, and `Died` remain authoritative TurnResolver events.
- A mobility reduction from 4→2 is useful information/hint, but is NOT an instant reward.
- `EnemyForced` is emitted only on a transition into exactly one plausible move.
- `EnemyTrapped` is emitted only when an enemy remains alive yet has zero plausible next moves.
- Events belong to the Decision edge/route that caused them; no sibling sharing.
- No new crate dependencies.

## Review Focus

- A dead enemy must not also be rewarded as trapped on the same transition.
- An enemy already at one plausible move must not repeatedly emit `EnemyForced`.
- Equal/shorter SnakeBollada must not receive an offensive head-to-head hint for a contested destination.
- General route queries must reuse the existing per-snake BFS, not introduce another search.
- Choke hints must only reorder search; Decision must still evaluate every plausible action required for correctness.

---

### Task 1: Make StateAnalysis usable by simulated search nodes

**Files:**
- Modify: `src/analysis/routes.rs`
- Modify: `src/analysis/mod.rs`
- Modify: `src/simulation/mod.rs`
- Test: inline tests in `src/analysis/routes.rs`

**Interfaces:**
- Consumes: `GameState` and `SimulatedGameState`.
- Produces:
  - `StateAnalysis::from_simulated(&SimulatedGameState, ForecastCertainty) -> StateAnalysis`
  - `StateAnalysis::distance_for(snake_id: &str, coord: Coord) -> Option<u16>`
  - `StateAnalysis::first_moves_for(snake_id: &str, coord: Coord) -> MoveMask`

- [ ] **Step 1: Add failing tests**

Add:

```rust
simulated_state_builds_same_route_analysis_as_api_state()
distance_query_works_for_non_food_cell()
equal_shortest_paths_to_non_food_cell_preserve_all_first_moves()
dead_simulated_snake_is_not_used_as_route_source()
```

- [ ] **Step 2: Run tests and confirm RED**

```bash
cargo test analysis::routes::tests -- --nocapture
```

Expected: FAIL because simulated-state/general-cell analysis is unavailable.

- [ ] **Step 3: Re-export simulation state types**

In `src/simulation/mod.rs`, re-export the crate-private types Hunting/Decision need rather than exposing deep module paths:

```rust
pub(crate) use state::{SimulatedGameState, SimulatedSnake};
pub(crate) use resolver::{
    EliminationCause, ForecastDelta, InstantEvent, JointAction, TurnResolution, resolve_turn,
};
```

- [ ] **Step 4: Refactor route-field construction around a shared board view**

Keep one BFS implementation. Add internal normalized snake/board input so both `GameState` and `SimulatedGameState` feed the same `build_route_field`.

Only alive simulated snakes participate as route sources/obstacles.

Keep current Food Mode APIs working unchanged.

- [ ] **Step 5: Retain per-snake route fields inside StateAnalysis**

Today route fields are discarded after food extraction. Store them so all-cell queries reuse the already-computed BFS.

Expose:

```rust
pub(crate) fn distance_for(
    &self,
    snake_id: &str,
    coord: Coord,
) -> Option<u16>;

pub(crate) fn first_moves_for(
    &self,
    snake_id: &str,
    coord: Coord,
) -> MoveMask;
```

- [ ] **Step 6: Verify GREEN**

```bash
cargo fmt --all -- --check
cargo test analysis::routes::tests -- --nocapture
cargo test modes::food::tests -- --nocapture
```

Expected: PASS, including existing Food Mode tests.

- [ ] **Step 7: Commit**

```bash
git add src/analysis src/simulation/mod.rs
git commit -m "feat: extend state analysis for simulated hunting"
```

---

### Task 2: Define Hunting target analysis without scoring

**Files:**
- Create: `src/modes/hunting.rs`
- Modify: `src/modes/mod.rs`
- Test: inline tests in `src/modes/hunting.rs`

**Depends on:** Enemy Tracing exposing next-turn `EnemyTracingOutput`/`EnemyMoveSet`. If Enemy Tracing lands later, use test fixtures against its agreed public contract and wire the real implementation in Task 5.

**Interfaces:**

```rust
pub(crate) struct HuntingTarget {
    pub(crate) snake_id: String,
    pub(crate) path_distance: Option<u16>,
    pub(crate) plausible_moves: u8,
    pub(crate) length_delta: i32,
    pub(crate) shared_food_pressure: u8,
}

pub(crate) struct HuntingModeOutput {
    pub(crate) targets: Vec<HuntingTarget>,
    pub(crate) hints: Vec<HuntingHint>,
}

pub(crate) fn analyze(
    state: &SimulatedGameState,
    analysis: &StateAnalysis,
    tracing: &EnemyTracingOutput,
    our_legal_moves: MoveMask,
) -> HuntingModeOutput;
```

- [ ] **Step 1: Add failing target tests**

```rust
closer_constrained_enemy_ranks_before_distant_open_enemy()
shorter_enemy_wins_length_tie_break()
target_distance_uses_real_bfs_distance_not_manhattan()
dead_enemy_is_not_a_target()
target_order_is_deterministic()
```

- [ ] **Step 2: Run RED**

```bash
cargo test modes::hunting::tests -- --nocapture
```

- [ ] **Step 3: Implement deterministic raw target ordering**

Use raw features only:

1. reachable target before unreachable;
2. shorter central-BFS path distance;
3. fewer plausible moves;
4. larger `length_delta = our_length - enemy_length`;
5. higher shared-food pressure;
6. stable snake id.

Do not turn this into aggression-weighted utility.

- [ ] **Step 4: Derive shared-food pressure from StateAnalysis**

`shared_food_pressure` = number of known foods where both us and target are reachable and absolute ETA difference <= 1.

Do not run new BFS.

- [ ] **Step 5: Verify GREEN**

```bash
cargo fmt --all -- --check
cargo test modes::hunting::tests -- --nocapture
```

- [ ] **Step 6: Commit**

```bash
git add src/modes/hunting.rs src/modes/mod.rs
git commit -m "feat: add hunting target analysis"
```

---

### Task 3: Add tactical contested-cell hints

**Files:**
- Modify: `src/modes/hunting.rs`
- Test: inline tests in `src/modes/hunting.rs`

**Interfaces:**

```rust
pub(crate) enum HuntingHintKind {
    FavorableHeadContest,
    ApproachTarget,
    MobilityPressure,
}

pub(crate) struct HuntingHint {
    pub(crate) target: String,
    pub(crate) first_move: Direction,
    pub(crate) kind: HuntingHintKind,
}
```

Hints exist only for Decision search ordering.

- [ ] **Step 1: Add failing tests**

```rust
longer_snake_gets_favorable_head_contest_hint()
equal_length_snake_does_not_get_favorable_contest_hint()
shorter_snake_does_not_get_favorable_contest_hint()
all_plausible_enemy_destinations_are_checked()
approach_hint_uses_central_distance()
```

- [ ] **Step 2: Run RED**

```bash
cargo test modes::hunting::tests -- --nocapture
```

- [ ] **Step 3: Implement favorable contest detection**

For each target plausible move and each Decision-supplied legal move of ours:

```text
enemy_destination = enemy_direction.apply(enemy_head)
our_destination   = our_direction.apply(our_head)
```

Only if destinations match AND `our_length > enemy_length`, emit `FavorableHeadContest`.

Never infer the kill here; `resolve_turn` remains authoritative.

- [ ] **Step 4: Add approach/mobility-pressure ordering hints**

Use `StateAnalysis::distance_for` to order moves that move toward a constrained target.

A hint does not remove other moves and does not create an event/reward.

- [ ] **Step 5: Verify GREEN**

```bash
cargo fmt --all -- --check
cargo test modes::hunting::tests -- --nocapture
```

- [ ] **Step 6: Commit**

```bash
git add src/modes/hunting.rs
git commit -m "feat: add hunting tactical hints"
```

---

### Task 4: Add hunting transition events after resolve_turn

**Files:**
- Modify: `src/simulation/resolver.rs` only to extend the shared event enum
- Modify: `src/modes/hunting.rs`
- Modify: the Decision edge-expansion file once it lands
- Test: Hunting tests + Decision transition tests

**Interfaces:**

Extend existing `InstantEvent` with:

```rust
EnemyForced {
    enemy: String,
    remaining_moves: u8,
},
EnemyTrapped {
    enemy: String,
},
```

Add:

```rust
pub(crate) fn derive_transition_events(
    parent: &EnemyTracingOutput,
    child: &EnemyTracingOutput,
    turn_resolution: &TurnResolution,
) -> Vec<InstantEvent>;
```

- [ ] **Step 1: Add failing event tests**

```rust
four_to_two_move_reduction_is_not_an_instant_event()
transition_into_one_move_emits_enemy_forced()
already_forced_enemy_does_not_repeat_enemy_forced()
alive_enemy_with_zero_moves_emits_enemy_trapped()
killed_enemy_is_not_also_marked_trapped()
hunting_does_not_recreate_enemy_killed_or_head_to_head_won()
```

- [ ] **Step 2: Run RED**

```bash
cargo test modes::hunting::tests -- --nocapture
```

- [ ] **Step 3: Extend shared InstantEvent enum**

Add `EnemyForced` and `EnemyTrapped` to the existing enum in `src/simulation/resolver.rs`.

Do NOT make `resolve_turn` emit these; it does not own future-mobility analysis.

- [ ] **Step 4: Implement edge-local event derivation**

For enemies alive in the child state:

```text
before = parent plausible count
after  = child plausible count
```

Rules:

- `before > 1 && after == 1` → `EnemyForced`.
- `after == 0` → `EnemyTrapped`.
- `after >= 2` → no instant Hunting event.
- If `TurnResolution.events` already contains `EnemyKilled { enemy, .. }`, emit neither Forced nor Trapped for that enemy.

- [ ] **Step 5: Integrate into Decision edge construction**

Decision sequence must be:

```text
parent tracing
   ↓
joint action
   ↓
resolve_turn
   ↓
child StateAnalysis
   ↓
child Enemy Tracing
   ↓
derive_transition_events
   ↓
SearchEdge.events =
    TurnResolution.events
    + Hunting transition events
```

Store events on the edge, never the child node.

- [ ] **Step 6: Verify GREEN**

```bash
cargo fmt --all -- --check
cargo test modes::hunting::tests -- --nocapture
cargo test simulation::resolver::tests -- --nocapture
cargo test decision -- --nocapture
```

Use the actual Decision test namespace if it differs.

- [ ] **Step 7: Commit**

```bash
git add src/simulation/resolver.rs src/modes/hunting.rs src/decision
git commit -m "feat: derive hunting events from simulated transitions"
```

---

### Task 5: Integrate Hunting with Decision search ordering

**Files:**
- Modify: the in-progress Decision search expansion module
- Test: Decision search tests

**Consumes:** `HuntingModeOutput.hints`.

**Produces:** deterministic ordering only. No pruning.

- [ ] **Step 1: Add failing Decision tests**

```rust
favorable_hunting_hint_orders_move_before_neutral_move()
hunting_ordering_does_not_remove_unhinted_move()
fatal_hunting_line_still_loses_to_survival_gate()
food_and_hunting_can_both_annotate_same_first_move()
```

- [ ] **Step 2: Run RED**

Run the Decision search test target.

- [ ] **Step 3: Add stable hint ordering**

Recommended hint priority:

```text
FavorableHeadContest
> MobilityPressure
> ApproachTarget
> none
```

This is only an expansion-order key.

Decision remains responsible for:

- Survival gating;
- Food candidates;
- aggression;
- route utility;
- final move selection.

- [ ] **Step 4: Assert set preservation**

Before/after Hunting ordering, the set of Decision-expanded legal first moves must be identical.

- [ ] **Step 5: Verify GREEN and commit**

```bash
cargo test decision -- --nocapture
git add src/decision
git commit -m "feat: order decision search with hunting hints"
```

---

### Task 6: Add optional choke-point hints

**Files:**
- Create: `src/analysis/choke.rs`
- Modify: `src/analysis/mod.rs`
- Modify: `src/modes/hunting.rs`
- Test: inline tests

**Purpose:** improve search ordering after core Hunting works. This task can be postponed if depth/latency measurements show no need yet.

- [ ] **Step 1: Add failing articulation tests**

```rust
corridor_between_rooms_has_articulation_point()
open_area_has_no_false_choke()
occupied_cell_is_not_a_choke()
```

- [ ] **Step 2: Implement Tarjan articulation detection**

Build the current free-cell graph and detect articulation points in `O(V + E)`.

- [ ] **Step 3: Add Hunting choke tests**

```rust
choke_we_reach_before_target_produces_hint()
choke_target_reaches_first_produces_no_priority()
choke_hint_never_emits_immediate_trap_reward()
```

Use central route queries for arrival time.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all -- --check
cargo test analysis::choke::tests -- --nocapture
cargo test modes::hunting::tests -- --nocapture
git add src/analysis/choke.rs src/analysis/mod.rs src/modes/hunting.rs
git commit -m "feat: add choke-point hunting hints"
```

---

### Task 7: Integrate hunting events into Decision E2E route evaluation

**Files:**
- Modify: the Decision route evaluator
- Test: Decision route-evaluation tests
- Do NOT add scoring logic to Hunting

- [ ] **Step 1: Add failing route-isolation tests**

```rust
forced_reward_exists_only_on_route_that_caused_it()
trapped_reward_exists_only_on_route_that_caused_it()
kill_supersedes_forced_and_trapped_for_same_enemy()
sibling_kill_does_not_change_other_route()
hunting_raw_value_is_weighted_by_decision_aggression()
survival_failure_still_dominates_hunting_reward()
```

- [ ] **Step 2: Run RED**

Run Decision evaluator tests.

- [ ] **Step 3: Extend Decision's existing event reducer**

Within ONE E2E route:

```text
EnemyForced  -> low raw Hunting benefit
EnemyTrapped -> higher raw Hunting benefit
EnemyKilled  -> strongest raw Hunting benefit
HeadToHeadWon -> confirmed tactical/kill benefit
```

For the same enemy, `EnemyKilled` supersedes earlier Forced/Trapped benefit to avoid double counting.

Do not define aggression constants in Hunting. Decision applies its route-local `AggressionState` after Survival classification.

- [ ] **Step 4: Verify GREEN and commit**

```bash
cargo test decision -- --nocapture
git add src/decision
git commit -m "feat: evaluate hunting events in decision routes"
```

---

### Task 8: Telemetry and acceptance

**Files:**
- Modify: `src/telemetry.rs`
- Modify: `docs/specs/hunting-mode.md`
- Test: telemetry tests + full quality gate

- [ ] **Step 1: Record Hunting observability**

Summarize, rather than persist every search node:

- primary/relevant target;
- target plausible moves at root;
- favorable contest hints considered;
- forced events on chosen route;
- trapped events on chosen route;
- TurnResolver-confirmed kills;
- raw Hunting benefit of chosen route;
- route-local aggression applied by Decision;
- completed search depth.

- [ ] **Step 2: Add telemetry tests**

Verify optional target, forced/trapped counts, and no kill/trap double counting.

- [ ] **Step 3: Update Hunting spec status**

Set branch target to `dev` and mark only implemented criteria complete.

- [ ] **Step 4: Run full quality gate**

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

Expected: all exit 0.

- [ ] **Step 5: Architectural duplication check**

```bash
rg "VecDeque|resolve_turn|food_candidates|build_route_field" src/modes/hunting.rs src/analysis src/decision src/simulation
```

Expected:

- no BFS queue inside Hunting;
- no duplicate turn resolver;
- no food pathfinder inside Hunting;
- shared central route analysis only;
- Decision owns tree/scoring/depth.

- [ ] **Step 6: Commit**

```bash
git add src/telemetry.rs docs/specs/hunting-mode.md
git commit -m "docs: mark hunting mode implementation status"
```

## Parallel implementation order with Decision Making

Because Decision is already being implemented:

1. **Task 1 can start immediately** and benefits both Decision and Hunting.
2. **Tasks 2–3 can proceed once Enemy Tracing's public structs are stable**, even before the full graph is complete.
3. **Task 4 waits for Decision edge construction**, but the current `resolve_turn` foundation already exists.
4. **Task 5 should be merged after Decision has a stable action-expansion ordering hook.**
5. **Task 6 is optional for Hunting V1** and should not delay core integration.
6. **Task 7 waits for Decision's E2E event reducer/aggression logic.**
7. **Task 8 closes the feature after Decision integration is stable.**

This minimizes conflicts: Hunting owns `src/modes/hunting.rs` and optional shared analysis; Decision owns graph expansion, route scoring, aggression, Survival priority, reserved cells, cache, depth, and final movement.

## Deferred beyond Hunting V1

- probabilistic opponent models;
- Minimax/MCTS inside Hunting;
- a separate Hunting tree;
- learned opponent profiles;
- probabilistic kill likelihood;
- full Voronoi/chamber-of-trees scoring;
- Hunting-specific depth control.
