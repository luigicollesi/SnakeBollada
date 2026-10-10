# Port Ordering V3 to Mauá AWS Lambda (preparation)

## Pinned source / target

- Source: `luigicollesi/SnakeBollada`, V3 engine from commit `10f9c7035a5b10f6c8efea56d61ed295b541c430` (the 10-unseen-seed benchmark revision).
- Destination: `Maua-Dev/battlesnake_luigi`, starting at `dev` commit `991143f81a5f59325bc7036ed115605a523cfe11`.
- Integration branch: `feature/ordering-v3-lambda-port`.
- **Do not push directly to destination `dev`: its workflow deploys to AWS automatically.**

## Compatibility findings

| Component | Source | Destination | Integration |
| --- | --- | --- | --- |
| HTTP entrypoint | Rocket | `lambda_http` | Keep destination `src/main.rs` and handler |
| Gameplay entry | `GameRuntime` + `DecisionState` | `logic::get_move` | Async adapter calling the original runtime |
| Request structs | Internal types in source `main.rs` | `models.rs` | Separate `v3_models.rs` conversion |
| Forecast/search | `src/analysis`, `search`, `simulation`, etc. | Not present | Copy Rust engine modules unchanged |
| V3 activation | `SNAKE_TERRITORY_MODE=ordering_v3` | No such variable | Configure only in staged Terraform |
| Rust binary | `snake-bollada` native server | `battlesnake` musl bootstrap | Preserve destination Cargo package name and CD |
| Persistent state | In-process cache per game | Lambda execution environment | Reuse while warm; rebuild when cold |
| CPU/memory | Native benchmark environment | Lambda initially 256 MB | Benchmark Lambda before increasing memory or merging |

## Prepare, compile and stage

Run from a local machine with GitHub push access to the Mauá repository, Rust/Cargo, Python 3 and network access:

```bash
git clone https://github.com/luigicollesi/SnakeBollada.git
cd SnakeBollada
git switch dev
python3 scripts/prepare_maua_v3_port.py ../battlesnake_luigi --push
```

The script performs the following:

1. Verifies that the source contains the benchmarked engine revision.
2. Clones/fetches the destination and refuses dirty working trees.
3. Creates `feature/ordering-v3-lambda-port` from destination `origin/dev`.
4. Copies the original V3 engine modules, preserving FutureGraph, MAX/MIN, and the original runtime.
5. Adapts the JSON request structs and `logic.rs` to the destination's asynchronous Lambda handler.
6. Sets `SNAKE_TERRITORY_MODE=ordering_v3` in the **feature branch only**.
7. Regenerates `Cargo.lock`, formats, tests and builds the native release.
8. Commits and optionally pushes **only** the feature branch.

No Terraform apply or Lambda deploy is performed by the script.

## Merge gates

- [ ] Source 10 unseen-seed benchmark completed; record results and exact source SHA.
- [ ] `cargo test --locked --all-features` passes on staged branch.
- [ ] Destination CI builds `x86_64-unknown-linux-musl` and creates executable `bootstrap`.
- [ ] `GET /`, `POST /start`, `POST /move`, and `POST /end` pass Lambda handler tests.
- [ ] `/move` makes valid, legal responses on at least 12 pinned representative game states.
- [ ] Verify V3 is actually selected, not the default primary Ordering.
- [ ] Confirm response latency under the Battlesnake **500 ms deadline**, including cold starts, Lambda 256 MB CPU share and API Gateway overhead.
- [ ] Test session loss/restart: each Lambda instance may be frozen or destroyed; forecast cache must be optional.
- [ ] Verify Lambda logging and operational fallbacks under load.
- [ ] Review runtime/memory costs and quota prior to deployment.
- [ ] Only after validation, approve merge into Mauá `dev` (triggers AWS CD).

## Known limitations

- The staging preparation has not yet been compiled in AWS Lambda or run against Hobbs on AWS. Matching the native Rust engine **does not** establish the same runtime performance or competitive behavior after a platform migration.
- GitHub integration write access to `Maua-Dev/battlesnake_luigi` returned HTTP 403 `Resource not accessible by integration` when attempting to create the feature branch. The script must be executed by a collaborator with push permission, or after updating the connected GitHub App repository permissions.
- The current Lambda memory allocation is 256 MB. CPU-intensive minimax may require a different allocation, subject to measurement.
- This script deliberately avoids copying Rocket routes, Docker/Vercel config and source benchmark workflows into the destination. The target AWS infrastructure stays authoritative.
