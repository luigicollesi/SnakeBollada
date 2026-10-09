# Local Hovering Hobbs benchmark

The benchmark is isolated: two Docker containers run on the GitHub runner, and all Battlesnake move requests go to `127.0.0.1`. Vercel is not used as a game server.

The new `.github/workflows/hobbs-benchmark.yml` workflow is **manual only**. Inputs accept up to 30 numeric seeds; defaults are 20261002, 20261003, and 20261004, matching the previous comparison.

**GitHub limitation:** `workflow_dispatch` generally requires the workflow file on the **default branch** to appear in Actions. While the file is only in `dev`, it is prepared but might not be dispatchable from the UI. Merge the verified workflow to `main` or run the existing validation workflow on its pinned branch when ready. No PR, deployment or simulation is started by adding this file.

The test workflow writes game state JSONL, both Docker logs, winner, peak length, smaller/equal/larger turn counts, depth and P95 latency, CLI exit code and connection-error count. Artifacts are downloadable from Actions.

**Caution:** Git pushes can still cause an automatic Vercel preview through the repository integration. This is separate from game traffic. Disable preview builds for benchmark branches if avoiding *all* Vercel usage is required.

Baseline from 2026-10-09: seed 20261002 won in 356 turns, seed 20261003 lost in 386 turns, seed 20261004 lost in 288 turns (1/3 wins). Run the exact same seeds and compare outcomes, relative length, depth, latency and error rates before declaring improvement.
