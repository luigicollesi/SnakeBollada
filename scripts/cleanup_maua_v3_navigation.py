#!/usr/bin/env python3
"""Clean only confirmed dead Rust symbols from Maua's V3 port.

Works with a dirty fix/ feature branch after the root-tactics patch. Does not
change the search, fallback rules, Terraform, lockfile, or deployment workflows.

Usage:
  python3 scripts/cleanup_maua_v3_navigation.py ../battlesnake_luigi
"""
from __future__ import annotations

import argparse
from pathlib import Path
import subprocess


def git(repo: Path, *args: str) -> str:
    return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()


def replace_exact(source: str, before: str, after: str, path: str) -> str:
    number = source.count(before)
    if number != 1:
        raise ValueError(f"{path}: expected one match for {before!r}, found {number}")
    return source.replace(before, after, 1)


def main() -> None:
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("target", type=Path)
    args = cli.parse_args()
    repo = args.target.resolve()
    branch = git(repo, "branch", "--show-current")
    if not branch.startswith("fix/"):
        raise SystemExit(f"Refusing to edit branch {branch!r}; select a fix/* branch")
    if "Maua-Dev/battlesnake_luigi" not in git(repo, "remote", "get-url", "origin"):
        raise SystemExit("Expected the Maua repository")
    navigation_file = repo / "src/navigation.rs"
    main_file = repo / "src/main.rs"
    nav = navigation_file.read_text()
    main_src = main_file.read_text()

    # All dead-code warnings were confirmed on the REAL Lambda release build.
    # Keep the navigation head-threat map and hazard check used by fallback.
    nav = replace_exact(nav, "use std::collections::VecDeque;\n\n", "", "navigation.rs")
    nav = replace_exact(nav, "    pub(crate) food: BoardMask,\n", "", "navigation.rs")
    nav = replace_exact(
        nav,
        "            food: BoardMask::from_coords(width, height, state.board.food.iter().copied()),\n",
        "",
        "navigation.rs",
    )
    nav = replace_exact(
        nav,
        """    pub(crate) fn is_blocked(&self, coord: Coord) -> bool {
        !self.in_bounds(coord)
            || self.occupied.contains(coord)
            || self.lethal_head_danger.contains(coord)
    }

    pub(crate) fn is_food(&self, coord: Coord) -> bool {
        self.food.contains(coord)
    }

""",
        "",
        "navigation.rs",
    )

    marker = "\npub(crate) fn reachable_after_move("
    if nav.count(marker) != 1:
        raise ValueError("navigation.rs: obsolete reachable_after_move function missing")
    # It is the final item of navigation.rs in the audited Maua revision.
    nav = nav[:nav.index(marker)].rstrip() + "\n"

    # Board/Game still belong to the crate's test fixtures, not production.
    main_src = replace_exact(
        main_src,
        "pub(crate) use v3_models::{Battlesnake, Board, Coord, Game, GameState};",
        """pub(crate) use v3_models::{Battlesnake, Coord, GameState};
#[cfg(test)]
pub(crate) use v3_models::{Board, Game};""",
        "main.rs",
    )

    # Preflight every edit in memory before touching either file.
    navigation_file.write_text(nav)
    main_file.write_text(main_src)
    print(f"Cleaned only navigation.rs and main.rs on {branch}.")
    print("No search, fallback, Lambda configuration or infrastructure was changed.")


if __name__ == "__main__":
    main()
