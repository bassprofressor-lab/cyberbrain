#!/usr/bin/env python3
"""Measure recall quality against a labelled query set.

    scripts/recall-eval.py queries.toml --store ~/project/.cyberbrain

A query set is TOML: each `[[query]]` carries `q`, the `expect`ed note names (any one of
them counts as correct) and a `kind` used only to group the report. The runner shells out
to the binary with `--json`, so the same set can be run against two builds and compared —
which is the point: a retrieval change is an opinion until it has a number on both sides.

Reported: recall@1/@3/@10 and MRR, overall and per kind. A miss is printed with what came
back instead, because the interesting question is never "how many" but "which ones".

The expected names are checked against the notes tree first. A typo in the set would
otherwise read as a retrieval failure, and a measurement instrument that fails silently is
worse than none.
"""
import argparse, json, subprocess, sys, tomllib
from pathlib import Path


def load_set(path):
    with open(path, "rb") as f:
        return tomllib.load(f)["query"]


def known_notes(store):
    return {p.stem for p in (Path(store) / "notes").rglob("*.md")}


def recall(binary, store, q, n):
    r = subprocess.run(
        [binary, "--store", str(store), "--json", "recall", q, "-n", str(n)],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        sys.exit(f"recall failed for {q!r}: {r.stderr.strip()}")
    return [h["note_name"] for h in json.loads(r.stdout)["hits"]]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("queries")
    ap.add_argument("--store", required=True)
    ap.add_argument("--bin", default="cyberbrain")
    ap.add_argument("-n", type=int, default=10)
    ap.add_argument("--json", action="store_true", help="machine-readable summary on stdout")
    a = ap.parse_args()

    qs = load_set(a.queries)
    known = known_notes(a.store)
    unknown = {e for q in qs for e in q["expect"] if e not in known}
    if unknown:
        sys.exit("query set names notes that are not in the store: " + ", ".join(sorted(unknown)))

    rows = []
    for q in qs:
        hits = recall(a.bin, a.store, q["q"], a.n)
        rank = next((i + 1 for i, h in enumerate(hits) if h in q["expect"]), None)
        rows.append({"q": q["q"], "kind": q["kind"], "expect": q["expect"], "rank": rank, "hits": hits})

    def summarise(rs):
        n = len(rs)
        if not n:
            return {}
        at = lambda k: sum(1 for r in rs if r["rank"] and r["rank"] <= k) / n
        mrr = sum(1 / r["rank"] for r in rs if r["rank"]) / n
        return {"n": n, "r@1": at(1), "r@3": at(3), "r@10": at(10), "mrr": mrr}

    overall = summarise(rows)
    per_kind = {k: summarise([r for r in rows if r["kind"] == k]) for k in sorted({r["kind"] for r in rows})}

    if a.json:
        print(json.dumps({"overall": overall, "per_kind": per_kind, "rows": rows}, indent=2))
        return

    print(f"{len(rows)} queries, top-{a.n}, store {a.store}\n")
    print(f"{'':<14} {'n':>3}  {'r@1':>6} {'r@3':>6} {'r@10':>6} {'MRR':>6}")
    for k, s in per_kind.items():
        print(f"{k:<14} {s['n']:>3}  {s['r@1']:>6.2f} {s['r@3']:>6.2f} {s['r@10']:>6.2f} {s['mrr']:>6.3f}")
    print(f"{'ALL':<14} {overall['n']:>3}  {overall['r@1']:>6.2f} {overall['r@3']:>6.2f} {overall['r@10']:>6.2f} {overall['mrr']:>6.3f}")

    misses = [r for r in rows if not r["rank"]]
    if misses:
        print(f"\n{len(misses)} not found in top {a.n}:")
        for r in misses:
            print(f"  [{r['kind']}] {r['q']}")
            print(f"      wanted {' | '.join(r['expect'])}")
            print(f"      got    {', '.join(r['hits'][:5]) or '(nothing)'}")
    weak = [r for r in rows if r["rank"] and r["rank"] > 3]
    if weak:
        print(f"\n{len(weak)} found but below rank 3:")
        for r in weak:
            print(f"  [{r['kind']}] rank {r['rank']}: {r['q']}")


if __name__ == "__main__":
    main()
