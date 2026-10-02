# Actor-relative metric contract

Status: active architecture contract

## Goal

Every signal used by decision making must have one explicit role. Expensive data that is neither a final decision metric nor an input to one should not be persisted in the search graph.

## Information classes

### InstantMetrics

Derived only from one `SimulatedGameState`. Recomputed independently for every searchable node.

Allowed examples:

- deterministic mobility;
- territory control;
- enclosure pressure;
- border exposure and structural risk;
- food opportunity;
- current health pressure;
- current snake lengths.

Instant metrics must never read previous real turns, opponent history, runtime latency, or search depth.

### TransitionFacts

Derived only from:

- parent node instant metrics;
- child node instant metrics;
- events produced by the resolved joint action.

Examples:

- mobility delta;
- territory delta;
- enclosure improvement;
- food consumption;
- ownership capture/denial;
- hazard damage;
- kill/death/win.

Transition facts are local causal information, not persistent strategy state.

### OrderingHints

May use the current node plus learned opponent observations. They can change expansion order but must not directly change actor utility or remove a legal opponent move.

Examples:

- opponent move hypotheses;
- food/hunting/head-threat support;
- learned opponent profile biases.

### RuntimeContext

May use runtime history such as latency and compute jitter. Runtime context controls search budget only and must not change game-state utility.

## Decision pipeline

```text
SimulatedGameState
        |
        v
instant analyses
        |
        v
ActorUtilityMetrics + StrategicWeights
        |
resolve joint action
        |
        v
child instant analyses
        |
        v
TransitionFacts(parent, child, events)
        |
        v
ActorTransitionScore
        |
        v
route actor totals
        |
        v
U_ours - sum(U_opponents)
```

## Metric ownership

A metric must be classified as exactly one of:

1. final metric: crosses the analysis/evaluation boundary and has an explicit utility consumer;
2. internal component: exists only inside an analysis module to derive a final metric;
3. ordering hint: affects expansion priority only;
4. runtime metric: affects budget/telemetry only.

If none applies, the metric should be removed.

## Current final actor metrics

| Metric | Category | Consumer |
| --- | --- | --- |
| safe non-reverse moves | Survival | mobility delta |
| enclosure risk | Survival | enclosure delta |
| border structural risk | Survival | structural-risk delta |
| border exposure | Survival | repeated per-turn harm |
| border pin risk | Survival | repeated per-turn harm |
| space capacity | Survival | dynamic weight + spatial-capacity delta |
| territory control | Survival | dynamic weight + territory-control delta |
| food potential | Food | food-potential delta |
| food survival pressure | Survival | repeated per-turn starvation harm |

Strategic weights are recomputed from the node state for every living actor. They are not persisted across turns.

## Double-counting rule

Correlated geometry is allowed to contribute through one final metric only unless two signals have intentionally different semantics.

Examples:

- body-on-edge and leading-edge-chain are internal components of border structural risk;
- inward control is an internal component of border pin risk;
- spatial capacity and territory control are distinct Survival signals;
- food opportunity for growth belongs to Food, while food runway required to stay alive belongs to Survival;
- own territory-control gain belongs to Survival;
- causal capture/denial of enemy ownership belongs to Hunting.

## Traceability requirement

Every final metric must have:

- a documented numeric range;
- one producer;
- one utility consumer;
- a monotonic unit test;
- at least one transition-level test proving that changing the metric changes the intended utility category.

## Performance rule

Search-node analyses should retain only data required for:

- future expansion;
- transition scoring;
- graph reconciliation;
- telemetry explicitly used in production.

Temporary intermediate vectors, maps, choke details, and aggregates should be collapsed into compact final signals and discarded when no later consumer exists.


## Opponent tracing invariant

Enemy tracing is an ordering layer, not a pruning layer. Every deterministic legal opponent move remains in the joint-action generator. Historical opponent profiles may change move order only; they never alter actor utility and never remove a legal move.
