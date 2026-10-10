#!/usr/bin/env python3
"""Real deployed Maua Lambda vs remote Hovering Hobbs via Battlesnake CLI.

- No AWS credentials read or copied: the Maua API Gateway is public.
- Locates deployment URL from publicly-readable GitHub Actions deploy logs.
- Verifies BOTH snakes are reachable before starting a duel.
- Saves CLI turn-by-turn JSON logs and diagnostics as artifacts.
- Never modifies Maua, AWS, or either snake's source.
"""
import json
import os
import re
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlsplit
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

M_REPO = "Maua-Dev/battlesnake_luigi"
BASE = "https://api.github.com"
OUTPUT = Path("results-live")
OUTPUT.mkdir(exist_ok=True)
LAMBDA_RE = re.compile(r"https://[a-z0-9]+\.execute-api\.us-east-1\.amazonaws\.com/dev/?")
HOBBS_CANDIDATES = (
    "https://battlesnake-rs.fly.dev/hovering-hobbs",
    "https://battlesnake.coreyja.com/hovering-hobbs",
)


def get(url: str, *, token: bool = False, timeout: int = 20) -> bytes:
    headers = {"Accept": "application/vnd.github+json", "User-Agent": "SnakeBollada-public-test"}
    if token and os.getenv("GITHUB_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GITHUB_TOKEN"]
    request = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return response.read()


def github_json(route: str):
    url = BASE + route
    try:
        return json.loads(get(url, token=True))
    except urllib.error.HTTPError as error:
        # A token scoped to SnakeBollada might be unable to read Maua actions;
        # public unauthenticated access is a separate fallback.
        if error.code in (403, 404):
            return json.loads(get(url, token=False))
        raise


def find_deployed_lambda() -> tuple[str, dict]:
    override = os.getenv("MAUA_SNAKE_URL", "").strip()
    if override:
        print("Using explicitly configured public MAUA_SNAKE_URL", flush=True)
        return override.rstrip("/"), {"source": "MAUA_SNAKE_URL override"}
    runs = github_json(
        f"/repos/{M_REPO}/actions/runs?branch=dev&status=success&per_page=25"
    ).get("workflow_runs", [])
    checked = []
    for run in runs:
        jobs = github_json(f"/repos/{M_REPO}/actions/runs/{run['id']}/jobs?per_page=100")
        for job in jobs.get("jobs", []):
            if "Deploy" not in job.get("name", "") or job.get("conclusion") != "success":
                continue
            checked.append({"run": run["id"], "job": job["id"], "name": job["name"]})
            endpoint = f"{BASE}/repos/{M_REPO}/actions/jobs/{job['id']}/logs"
            try:
                raw = get(endpoint, token=True, timeout=30)
            except (urllib.error.HTTPError, urllib.error.URLError) as error:
                try:
                    raw = get(endpoint, token=False, timeout=30)
                    print(f"Read public deployment log anonymously, job={job['id']}", flush=True)
                except (urllib.error.HTTPError, urllib.error.URLError) as anonymous:
                    code = getattr(error, "code", "network")
                    anon_code = getattr(anonymous, "code", "network")
                    print(f"Deploy log {job['id']} unavailable (token={code}, public={anon_code})", flush=True)
                    continue
            if raw[:2] == b"PK":
                import io
                import zipfile
                with zipfile.ZipFile(io.BytesIO(raw)) as archive:
                    raw = b"\n".join(archive.read(n) for n in archive.namelist() if n.endswith(".txt"))
            hits = LAMBDA_RE.findall(raw.decode("utf-8", errors="replace"))
            if hits:
                url = hits[-1].rstrip("/")
                return url, {"source": "public deploy log", "run": run["id"], "job": job["id"]}
    (OUTPUT / "discovery.json").write_text(json.dumps({"checked": checked}, indent=2))
    raise RuntimeError(
        "Could not find the public Lambda API URL in accessible Maua deploy logs. "
        "No actual duel was started. Provide it through MAUA_SNAKE_URL (a URL, not AWS keys)."
    )


def ping_snake(url: str) -> dict:
    raw = get(url, timeout=15)
    payload = json.loads(raw)
    if not isinstance(payload, dict) or "apiversion" not in payload:
        raise ValueError(f"GET {url}: not a Battlesnake info JSON")
    return {key: payload.get(key) for key in ("apiversion", "author", "color", "head", "tail")}


def find_hobbs() -> tuple[str, dict]:
    provided = os.getenv("HOBBS_SNAKE_URL", "").strip()
    candidates = [provided] if provided else HOBBS_CANDIDATES
    failures = []
    for url in candidates:
        try:
            return url.rstrip("/"), ping_snake(url)
        except Exception as e:
            failures.append(f"{url}: {type(e).__name__}: {e}")
    (OUTPUT / "hobbs_discovery_failures.txt").write_text("\n".join(failures) + "\n")
    raise RuntimeError("Hovering Hobbs remote endpoint unavailable; no duel started. " + "; ".join(failures))


# A local relay gives us independent control of network allowance and
# Battlesnake's advertised compute budget. The remote AI is always told 500ms.
REMOTE_COMPUTE_BUDGET_MS = 500
TRANSPORT_ALLOWANCE_MS = 2000
relay_log_lock = threading.Lock()
relay_records = []


def make_relay(base_url: str, snake_name: str):
    class Relay(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            self.forward(None)

        def do_POST(self):
            content = self.rfile.read(int(self.headers.get("Content-Length", 0)))
            self.forward(content)

        def forward(self, payload):
            route = self.path
            if payload is not None:
                try:
                    decoded = json.loads(payload)
                    if isinstance(decoded, dict) and isinstance(decoded.get("game"), dict):
                        decoded["game"]["timeout"] = REMOTE_COMPUTE_BUDGET_MS
                    payload = json.dumps(decoded).encode("utf-8")
                except (ValueError, UnicodeDecodeError):
                    pass
            req = urllib.request.Request(
                base_url if route == "/" else base_url + route,
                data=payload, method="POST" if payload is not None else "GET",
                headers={"Content-Type": "application/json", "User-Agent": "SnakeBollada-live-benchmark"}
            )
            started = time.monotonic()
            status, returned, error = 502, b"{}", None
            try:
                with urllib.request.urlopen(req, timeout=10) as response:
                    status, returned = response.status, response.read()
            except urllib.error.HTTPError as exc:
                status = exc.code
                returned = exc.read()
                error = str(exc)
            except Exception as exc:
                error = f"{type(exc).__name__}: {exc}"
            elapsed_ms = (time.monotonic() - started) * 1000
            event = {
                "snake": snake_name, "route": route, "elapsed_ms": round(elapsed_ms, 2),
                "within_real_500ms": elapsed_ms <= REMOTE_COMPUTE_BUDGET_MS,
                "status": status, "error": error,
            }
            with relay_log_lock:
                relay_records.append(event)
                with (OUTPUT / "http_latency.jsonl").open("a") as stream:
                    stream.write(json.dumps(event) + "\n")
            print(f"PROXY snake={snake_name} route={route} ms={elapsed_ms:.1f} status={status} "
                  f"within_500ms={elapsed_ms<=500} error={error}", flush=True)
            try:
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(returned)))
                self.end_headers()
                self.wfile.write(returned)
            except (BrokenPipeError, ConnectionResetError):
                pass

    return Relay


def start_relays(mau_url, hob_url):
    servers = []
    for port, name, url in (
        (18771, "SnakeLuigi", mau_url),
        (18772, "HoveringHobbs", hob_url),
    ):
        server = ThreadingHTTPServer(("127.0.0.1", port), make_relay(url, name))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        servers.append(server)
    return servers


def main():
    print("REAL ENDPOINT TEST: Ubuntu GitHub runner -> Maua deployed AWS API Gateway vs remote Hovering Hobbs", flush=True)
    try:
        mau_url, mau_source = find_deployed_lambda()
        mau_info = ping_snake(mau_url)
        hob_url, hob_info = find_hobbs()
        print(f"MAUA LIVE ENDPOINT FOUND: {mau_url} ({mau_source})", flush=True)
        print(f"HOBBS LIVE ENDPOINT FOUND: {hob_url}", flush=True)
        print(f"GET METADATA: Maua={mau_info} Hobbs={hob_info}", flush=True)
        (OUTPUT / "endpoints.json").write_text(
            json.dumps(
                {"maua": {"url": mau_url, "info": mau_info, "source": mau_source},
                 "hobbs": {"url": hob_url, "info": hob_info}}, indent=2
            )
        )
    except Exception as e:
        print(f"PRE-FLIGHT BLOCKER: {e}", flush=True)
        (OUTPUT / "preflight_failure.txt").write_text(str(e) + "\n")
        return 3

    cli = os.getenv("HOME", "/home/runner") + "/go/bin/battlesnake"
    if not Path(cli).exists():
        print("Battlesnake official CLI not installed", flush=True)
        return 4

    relay_servers = start_relays(mau_url, hob_url)
    print("Local relay started: remote engine sees game.timeout=500; CLI transport budget=2000ms", flush=True)
    summaries = []
    for seed in (401, 402, 403):
        run_path = OUTPUT / f"battle_{seed}.jsonl"
        command = [
            cli, "play", "-W", "11", "-H", "11",
            "-t", str(TRANSPORT_ALLOWANCE_MS), "-r", str(seed),
            "-n", "SnakeLuigi", "-u", "http://127.0.0.1:18771",
            "-n", "HoveringHobbs", "-u", "http://127.0.0.1:18772",
            "--output", str(run_path),
        ]
        print(f"STARTING REAL REMOTE DUEL seed={seed} (500ms internal budget, 2000ms transport allowance)", flush=True)
        started = time.monotonic()
        try:
            run = subprocess.run(command, capture_output=True, text=True, timeout=360)
            output = (run.stdout or "") + "\n" + (run.stderr or "")
            code = run.returncode
        except subprocess.TimeoutExpired as error:
            output = "DUEL TIMEOUT AFTER 360 SECONDS\n" + str(error)
            code = 124
        (OUTPUT / f"cli_{seed}.log").write_text(output)
        excerpt = output[-2000:]
        print(f"DUEL RESULT seed={seed} exit={code} elapsed={time.monotonic()-started:.2f}s: {excerpt}", flush=True)
        win = re.search(r"after\s+(\d+)\s+turns.\s+(.+?)\s+is\s+the\s+winner", output, re.I)
        matches = [json.loads(line) for line in run_path.read_text().splitlines()
                   if line.strip().startswith("{")] if run_path.exists() else []
        final_snakes = [s.get("name") for s in matches[-1].get("board", {}).get("snakes", [])] if matches else []
        slow = [x for x in relay_records if x["route"] == "/move" and not x["within_real_500ms"]]
        fail = [x for x in relay_records if x["route"] == "/move" and x["status"] != 200]
        summaries.append({
            "seed": seed, "returncode": code,
            "total_moves_so_far": sum(x["route"] == "/move" for x in relay_records),
            "slow_moves_so_far": len(slow), "failed_moves_so_far": len(fail),
            "ranking_valid_at_500ms": len(slow) == 0 and len(fail) == 0,
            "turns_output": len(matches), "final_alive": final_snakes,
            "winner_from_cli": win.group(2) if win else None,
            "winner_turn": int(win.group(1)) if win else None,
            "latency_wall_s": round(time.monotonic()-started, 2),
        })
        (OUTPUT / "results.json").write_text(json.dumps(summaries, indent=2))
        if code != 0:
            print("Stopping after CLI failure to avoid spamming deployed services", flush=True)
            return 5

    print("FINAL REAL REMOTE DUELS: " + json.dumps(summaries), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
