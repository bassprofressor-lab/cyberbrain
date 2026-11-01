#!/usr/bin/env python3
"""A/B measurement: the same tasks for Claude Code with Cyberbrain and without it.

Every claim that a memory tool "saves tokens" or "knows the project" needs a control group:
the same agent, the same model, the same files and the same questions, once with the tool and
once without. Without that, a ledger of "lines not read" only says what the tool returned, not
what the agent would have done otherwise. Idea from ProjectAtlas (styler-ai, MIT), whose token
report separates what was observed from what was modelled.

Two arms, each a directory with a `projekt/` checkout and an `mcp.json`:

    <arm>/projekt/   identical files in both arms; the "with" arm additionally has the store,
                     the hook entries (`cyberbrain install`) and the instructions file
    <arm>/mcp.json   the MCP servers of the arm; `{"mcpServers":{}}` for the control

Runs `claude -p` once per task, arm and repetition, the two arms of a task in random order,
with `--strict-mcp-config` so a user-wide MCP server cannot leak into the control. Records the
answer, whether it matches the task's regular expressions, turns, tool calls, tokens and time.
Uses the Claude Code login of the machine (subscription), no API key.

    scripts/ab-messung.py --tasks aufgaben.toml --with mit --without ohne --out lauf1 \\
        [--model sonnet] [--reps 2] [--only M1-trail,C1-fear-greed] [--seed 1]

Writes `<out>/runs.jsonl` (one line per run, appended, so an interrupted series resumes) and
`<out>/summary.md`. Re-running skips runs already in runs.jsonl.
"""

from __future__ import annotations

import argparse
import json
import random
import re
import statistics
import subprocess
import sys
import time
import tomllib
from pathlib import Path

PREFIX = (
    "Beantworte die Frage kurz auf Deutsch. Arbeite nur mit den Dateien in diesem Projekt und "
    "den Werkzeugen, die du hast. Wenn du es nicht herausfinden kannst, sag das ausdrücklich, "
    "statt zu raten.\n\nFrage: "
)
BASE_TOOLS = ["Read", "Grep", "Glob"]
WITH_TOOLS = ["Bash(cyberbrain recall:*)", "Bash(cyberbrain find:*)", "mcp__cyberbrain"]


def run_one(arm_dir: Path, arm: str, task: dict, model: str, timeout: int) -> dict:
    tools = BASE_TOOLS + (WITH_TOOLS if arm == "with" else [])
    cmd = [
        "claude", "-p", PREFIX + task["prompt"],
        "--output-format", "stream-json", "--verbose",
        "--model", model,
        "--strict-mcp-config", "--mcp-config", str(arm_dir / "mcp.json"),
        "--no-session-persistence",
        "--permission-mode", "default",
        "--allowedTools", *tools,
    ]
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, cwd=arm_dir / "projekt", capture_output=True, text=True,
                           timeout=timeout)
        out, err, code = p.stdout, p.stderr, p.returncode
    except subprocess.TimeoutExpired as e:
        out = e.stdout.decode() if isinstance(e.stdout, bytes) else (e.stdout or "")
        err, code = "timeout", -1
    wall = time.monotonic() - t0

    result, tools_used, denied = None, [], 0
    for line in out.splitlines():
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "assistant":
            for c in ev.get("message", {}).get("content", []):
                if c.get("type") == "tool_use":
                    tools_used.append(c.get("name"))
        elif ev.get("type") == "result":
            result = ev
            denied = len(ev.get("permission_denials") or [])
    answer = (result or {}).get("result") or ""
    usage = (result or {}).get("usage") or {}
    correct = all(re.search(rx, answer, re.I | re.S) for rx in task["muss"])
    return {
        "task": task["id"], "kind": task["kind"], "arm": arm, "model": model,
        "correct": correct, "answer": answer, "exit": code,
        "error": None if result else (err or "no result event")[-500:],
        "turns": (result or {}).get("num_turns"),
        "duration_s": round(wall, 1),
        "input_tokens": usage.get("input_tokens"),
        "cache_read": usage.get("cache_read_input_tokens"),
        "cache_write": usage.get("cache_creation_input_tokens"),
        "output_tokens": usage.get("output_tokens"),
        "cost_usd_equiv": (result or {}).get("total_cost_usd"),
        "tool_calls": len(tools_used), "tools": tools_used, "denied": denied,
    }


def total_tokens(r: dict) -> int:
    return sum(r.get(k) or 0 for k in ("input_tokens", "cache_read", "cache_write", "output_tokens"))


def summarise(rows: list[dict], path: Path) -> str:
    arms = ("with", "without")
    lines = ["# A/B-Messung Cyberbrain", ""]
    lines.append("| Gruppe | Arm | richtig | Läufe | Median Tokens | Median Runden | Median Werkzeuge | Median s |")
    lines.append("|---|---|---|---|---|---|---|---|")
    for kind in ("gedaechtnis", "code", "kontrolle", "alle"):
        for arm in arms:
            rs = [r for r in rows if r["arm"] == arm and (kind == "alle" or r["kind"] == kind)]
            if not rs:
                continue
            med = lambda f: statistics.median([f(r) for r in rs])  # noqa: E731
            lines.append(
                f"| {kind} | {arm} | {sum(r['correct'] for r in rs)} | {len(rs)} | "
                f"{med(total_tokens):,.0f} | {med(lambda r: r['turns'] or 0):.0f} | "
                f"{med(lambda r: r['tool_calls']):.0f} | {med(lambda r: r['duration_s']):.0f} |")
    lines += ["", "## Je Aufgabe (richtig mit / ohne, Median Tokens mit / ohne)", ""]
    for tid in sorted({r["task"] for r in rows}):
        w = [r for r in rows if r["task"] == tid and r["arm"] == "with"]
        o = [r for r in rows if r["task"] == tid and r["arm"] == "without"]
        mt = lambda rs: f"{statistics.median([total_tokens(r) for r in rs]):,.0f}" if rs else "–"  # noqa: E731
        lines.append(f"- {tid}: {sum(r['correct'] for r in w)}/{len(w)} gegen "
                     f"{sum(r['correct'] for r in o)}/{len(o)}; Tokens {mt(w)} gegen {mt(o)}")
    errs = [r for r in rows if r["error"]]
    if errs:
        lines += ["", f"**{len(errs)} Lauf/Läufe ohne Ergebnis** (in runs.jsonl: error)."]
    text = "\n".join(lines) + "\n"
    path.write_text(text)
    return text


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--tasks", type=Path, required=True)
    ap.add_argument("--with", dest="with_dir", type=Path, required=True)
    ap.add_argument("--without", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--model", default="sonnet")
    ap.add_argument("--reps", type=int, default=1)
    ap.add_argument("--only", default="")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--timeout", type=int, default=420)
    a = ap.parse_args()
    a.with_dir, a.without, a.out = a.with_dir.resolve(), a.without.resolve(), a.out.resolve()

    tasks = tomllib.loads(a.tasks.read_text())["aufgabe"]
    if a.only:
        keep = set(a.only.split(","))
        tasks = [t for t in tasks if t["id"] in keep]
    for d in (a.with_dir, a.without):
        if not (d / "projekt").is_dir() or not (d / "mcp.json").is_file():
            sys.exit(f"{d}: projekt/ oder mcp.json fehlt")
    a.out.mkdir(parents=True, exist_ok=True)
    log = a.out / "runs.jsonl"
    done = []
    if log.exists():
        done = [json.loads(line) for line in log.read_text().splitlines() if line.strip()]
    have = {}
    for r in done:
        have[(r["task"], r["arm"], r["model"])] = have.get((r["task"], r["arm"], r["model"]), 0) + 1

    rng = random.Random(a.seed)
    plan = []
    for rep in range(a.reps):
        order = tasks[:]
        rng.shuffle(order)
        for t in order:
            arms = ["with", "without"]
            rng.shuffle(arms)
            plan += [(rep, t, arm) for arm in arms]
    count = {}
    for rep, t, arm in plan:
        key = (t["id"], arm, a.model)
        count[key] = count.get(key, 0) + 1
        if count[key] <= have.get(key, 0):
            continue
        d = a.with_dir if arm == "with" else a.without
        r = run_one(d, arm, t, a.model, a.timeout)
        r["rep"] = rep
        with log.open("a") as f:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
        done.append(r)
        print(f"{t['id']:16} {arm:8} {'richtig' if r['correct'] else 'FALSCH ':8} "
              f"{total_tokens(r):>9,} Tokens {r['turns'] or 0:>3} Runden {r['duration_s']:>6} s"
              + (f"  FEHLER {r['error'][:80]}" if r["error"] else ""), flush=True)
    print(summarise(done, a.out / "summary.md"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
