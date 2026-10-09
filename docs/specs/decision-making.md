# Active decision architecture — Hobbs + FutureGraph

**Status:** Single production decision flow on `dev`. Historical Food/Hunting experiments are archived under `docs/superpowers/plans/` and are not active implementations.

## Request lifecycle

1. Convert Battlesnake API state to `SimulatedGameState`.
2. Reuse an in-memory per-game `FutureGraph` when the observed state matches a cached node, or rebuild when food changes unexpectedly or the graph cannot reconcile.
3. Generate all deterministically safe moves for us and each living adversary; if no safe moves exist, use an in-bounds fallback so the simulator can represent unavoidable death.
4. Resolve each joint move via `simulation/resolver.rs`.
5. Cache equivalent future states by `StateKey`, keeping only the branch associated with our chosen root move between requests.
6. Use `search/hobbs_flow.rs` to evaluate each root action against the **minimum** over enumerated opponent joint responses.
7. Score leaves with `evaluation/hobbs_score.rs` (terminal result; critical-health path to food; weighted temporal territory + capped relative length).
8. Use `evaluation/survival_guard.rs` to classify trapped, constrained, or otherwise dangerous exits. This is a structural ordering constraint, not a weighted category score.
9. Accept only depths whose expansions and evaluations finished within the budget; fall back to the last completed depth.
10. Return a move and lightweight search diagnostics.

## Exact authority

`decision/engine.rs` calls `search_hobbs` directly. There is no runtime environment switch for an alternative Food/Hunting engine.

`search/graph.rs` stores physical states and joint actions. Edge transition rewards and legacy actor metrics have been deleted.

`decision/joint_actions.rs` produces a Cartesian product of known legal responses; the decision engine does **not** exclude a dangerous response based on an estimated opponent intention.

The state-value function is not a trajectory reward sum. Unknown future food spawns are provisional, never invented.

## Limits and research tasks

- Iterative paranoid search can be expensive in multi-snake games. It is budgeted and reuses state nodes but **does not yet implement full Alpha-Beta pruning**.
- Temporal flood fill is an approximation of tail release under unknown future growth.
- The structural guard includes contest heuristics, not a proof of inevitability for every edge/corner scenario.
- The fallback strategy exists only for unsupported rulesets and search failure, not as another selectable strategic mode.
- The resulting move quality must still be validated through deterministic and multi-seed games against Hovering Hobbs.

## Validation

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
```

Do not delete regression fixtures merely because they were derived from old episodes. Structural protection, food growth, and state-level decision tests remain relevant even though the former modes have been removed.
