#!/usr/bin/env python3
"""Stage an isolated Maua Lambda self-play regression inside a disposable GitHub runner.

Never commits/pushes to Maua-Dev/battlesnake_luigi. The checkout is the
real Maua dev code; only the ephemeral workspace receives the test/fix.
"""
import argparse
from pathlib import Path


def replace_exact(body: str, before: str, after: str, *, expected: int = 1) -> str:
    count = body.count(before)
    if count != expected:
        raise RuntimeError(f"Expected {expected} occurrence(s), saw {count}: {before!r}")
    return body.replace(before, after)


def install(maua: Path, fixture: Path) -> None:
    entry = maua / "src/main.rs"
    body = entry.read_text(encoding="utf-8")
    flag = "#[cfg(test)]\nmod ci_maua_selfplay_fixture;\n"
    if flag in body:
        raise RuntimeError("CI fixture already installed")
    entry.write_text(body.rstrip() + "\n" + flag, encoding="utf-8")
    (maua / "src/ci_maua_selfplay_fixture.rs").write_text(
        fixture.read_text(encoding="utf-8"), encoding="utf-8"
    )
    print("TEST FIXTURE INSTALLED (temporary Maua checkout only)", flush=True)


def patch_session(maua: Path) -> None:
    path = maua / "src/runtime.rs"
    body = path.read_text(encoding="utf-8")
    # Preserve Lambda's lazy session initialization, CPU semaphore,
    # timeout/fallback behavior, and all existing logging.
    patches = [
        (
            "sessions: RwLock<HashMap<String, Arc<GameSession>>>",
            "sessions: RwLock<HashMap<(String, String), Arc<GameSession>>>",
            1,
        ),
        (
            "sessions.contains_key(&state.game.id)",
            "sessions.contains_key(&(state.game.id.clone(), state.you.id.clone()))",
            2,
        ),
        (
            "sessions.insert(state.game.id.clone(), Arc::new(GameSession::default()));",
            "sessions.insert((state.game.id.clone(), state.you.id.clone()), Arc::new(GameSession::default()));",
            1,
        ),
        (
            ".entry(state.game.id.clone())",
            ".entry((state.game.id.clone(), state.you.id.clone()))",
            1,
        ),
        (
            ".remove(&state.game.id)",
            ".remove(&(state.game.id.clone(), state.you.id.clone()))",
            1,
        ),
        (
            ".keys()\n            .cloned()",
            ".keys()\n            .map(|(game, _)| game.clone())",
            1,
        ),
    ]
    for before, after, n in patches:
        body = replace_exact(body, before, after, expected=n)
    path.write_text(body, encoding="utf-8")
    print("SESSION PATCH APPLIED to temporary checkout: (game.id, you.id)", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("stage", choices=["install", "patch"])
    parser.add_argument("--maua", type=Path, required=True)
    parser.add_argument("--fixture", type=Path)
    args = parser.parse_args()
    maua = args.maua.resolve()
    if args.stage == "install":
        if args.fixture is None:
            parser.error("--fixture required for install")
        install(maua, args.fixture)
    else:
        patch_session(maua)


if __name__ == "__main__":
    main()
