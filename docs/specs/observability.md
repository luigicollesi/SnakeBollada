# Runtime and observability

**Current implementation:** `src/runtime.rs` stores independent, ephemeral decision sessions in memory indexed by Battlesnake game ID.

- `POST /start` creates a new session; a duplicate start keeps the existing one.
- `POST /move` executes `DecisionState` for the requested session under its own mutex.
- `POST /move` without a session uses the stateless authoritative Hobbs flow.
- `POST /end` releases the session and its cached FutureGraph.
- Multiple game sessions are supported per process. There is no database persistence or game-history writer.

### Retained memory

Each `DecisionState` stores the FutureGraph, the last observed food set, and a bounded window of processing/latency times for jitter budgeting. The search graph contains physical states, transposition keys, joint actions, and forecast certainty; it does not persist opponent-intent profiles or Food/Hunting utility history.

### Search telemetry

Per move, standard process logs report selected action, analyzed depth, node/edge counts, transposition hits, processing time, reserve, Hobbs score, structural guard classification, provisional-spawn flag, root alternatives, and graph expansion cost. Logs are not a persisted match database.

All known opponents are considered using deterministic legal responses; predictions never remove a physically legal response for strategic plausibility. Run deterministic and multi-seed Hobbs duels separately to measure effective move quality.
