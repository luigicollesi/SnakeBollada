#!/usr/bin/env python3
"""Benchmark deployed Maua AWS Lambda versus locally built original Hovering Hobbs.

Direct HTTP: official Battlesnake CLI enforces 500ms on both /move endpoints.
No secrets, no deploy, and never implies local CPU equals Hobbs production.
"""
import json
import os
import re
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

M = "https://7qmd5n7ia2.execute-api.us-east-1.amazonaws.com/dev"
H = "http://127.0.0.1:3080/hovering-hobbs"
OUT = Path("hobbs-local-results")
OUT.mkdir(exist_ok=True)


def info(url):
    with urllib.request.urlopen(url, timeout=12) as response:
        return json.load(response)


def main():
    try:
        mi, hi = info(M), info(H)
        if not all(x.get("apiversion") for x in (mi, hi)):
            raise ValueError("Bad Battlesnake metadata")
        print(f"METADATA Maua={mi} Hobbs={hi}", flush=True)
    except Exception as exc:
        print(f"PRE-FLIGHT ERROR: {type(exc).__name__} {exc}", flush=True)
        return 4

    results = []
    cli = str(Path.home() / "go/bin/battlesnake")
    # Distinct fixed seeds. A real benchmark needs many more games / balanced spawns.
    for seed in (601, 602, 603):
        target = OUT / f"game_{seed}.jsonl"
        args = [
            cli, "play", "-W", "11", "-H", "11",
            "-t", "500", "-r", str(seed),
            "-n", "SnakeLuigi", "-u", M,
            "-n", "HoveringHobbs", "-u", H,
            "--output", str(target),
        ]
        print(f"START 500ms actual endpoint duel seed={seed}", flush=True)
        start = time.monotonic()
        try:
            result = subprocess.run(args, text=True, capture_output=True, timeout=300)
            output = "\n".join((result.stdout or "", result.stderr or ""))
            exit_code = result.returncode
        except subprocess.TimeoutExpired as err:
            output, exit_code = str(err), 124
        (OUT / f"cli_{seed}.log").write_text(output)
        print(f"CLI seed={seed} exit={exit_code}\n{output[-3200:]}", flush=True)
        err_lines = [
            line.strip() for line in output.splitlines()
            if re.search(r"WARN|ERROR|failed|deadline exceeded|timeout|timed out", line, re.I)
        ]
        match = re.search(
            r"Game completed after\s+(\d+)\s+turns\.\s+(.+?)\s+was the winner", output, re.I
        )
        rec = {
            "seed": seed, "exit": exit_code,
            "elapsed_s": round(time.monotonic()-start, 2),
            "winner": match.group(2) if match else None,
            "turns": int(match.group(1)) if match else None,
            "move_or_network_errors": err_lines[:80],
            "valid_without_network_errors": exit_code == 0 and not err_lines,
        }
        results.append(rec)
        (OUT / "summary.json").write_text(json.dumps(results, indent=2))
        print("GAME_RESULT " + json.dumps(rec), flush=True)
        if exit_code != 0:
            break

    print("FINAL_LOCAL_HOBBS_RESULTS: " + json.dumps(results), flush=True)
    return 0 if all(item["exit"] == 0 for item in results) else 5


if __name__ == "__main__":
    sys.exit(main())
