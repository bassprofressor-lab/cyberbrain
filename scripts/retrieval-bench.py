#!/usr/bin/env python3
"""Reproducible retrieval benchmark: lexical against hybrid, plus latency.

    # public: LongMemEval-S (MIT), session-level retrieval, no LLM calls
    scripts/retrieval-bench.py longmemeval --data ~/.cache/cyberbrain-eval/longmemeval/longmemeval_s_cleaned.json \\
        --model-dir /path/to/models/model2vec --work /tmp/cb-bench

    # private: a copy of a real store and a labelled query set (see recall-eval.py)
    scripts/retrieval-bench.py store --source ~/project/.cyberbrain --queries eval-local/queries.toml \\
        --model-dir /path/to/models/model2vec --work /tmp/cb-bench

Everything runs against throwaway stores under --work, built with `cyberbrain init` and
filled with note files, then indexed with `scan --full`. The source store is only read (its
`notes/` tree is copied); its configuration is not taken over, so no inference endpoint and
no governance hook is involved, and a store the benchmark writes to can never be a live one.

The two modes differ in one thing only: `embedding.model_path`. Hybrid points it at the
model artefact, lexical at a directory that does not exist, which is the documented
fallback. Every answer is checked for the "lexical only" caveat, and a hybrid answer that
carries it — or a lexical one that lacks it — stops the run. A benchmark that silently
measures the wrong mode is worse than none.

Recall returns blocks. The metrics are computed per note (first occurrence of each note
counts, later blocks of the same note are dropped), because the unit that is labelled is a
note — a LongMemEval session, a store note. `--unit block` keeps the raw block list, which
is what `recall-eval.py` measured before 26.09.2026.

Latency is measured through the real binary, process start included:
  - cli cold      CYBERBRAIN_NO_DAEMON=1, every call loads what it needs itself
  - daemon first  no daemon running: the call answers locally and starts one
  - daemon ready  seconds from that first call until the socket accepts
  - daemon warm   later calls answered by the resident process

Only numbers are written (`--out`, JSON). No note text leaves --work.
"""
import argparse, concurrent.futures as cf, datetime as dt, json, math, os, platform, random, shutil
import statistics, subprocess, sys, time, tomllib
from pathlib import Path

LEXICAL_MARK = "hits are lexical only"
CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
KS = (1, 3, 5, 10)


# ---------------------------------------------------------------- store handling

def ulid(rng):
    n = rng.getrandbits(128) >> 2  # 126 bits: first character stays <= 7
    return "".join(CROCKFORD[(n >> (5 * i)) & 31] for i in reversed(range(26)))


def run(cmd, env=None, check=True):
    r = subprocess.run(cmd, capture_output=True, text=True, env=env)
    if check and r.returncode != 0:
        sys.exit(f"failed: {' '.join(map(str, cmd))}\n{r.stderr.strip()}")
    return r


def env(daemon=False):
    e = dict(os.environ)
    if daemon:
        e.pop("CYBERBRAIN_NO_DAEMON", None)
    else:
        e["CYBERBRAIN_NO_DAEMON"] = "1"
    return e


def set_model(store, model_dir):
    cfg = Path(store) / "cyberbrain.toml"
    lines = cfg.read_text().splitlines()
    out = [f'model_path = "{model_dir}"' if l.startswith("model_path =") else l for l in lines]
    if not any(l.startswith("model_path =") for l in lines):
        sys.exit(f"{cfg}: no model_path line to set")
    cfg.write_text("\n".join(out) + "\n")


def fresh_store(binary, path, model_dir):
    if Path(path).exists():
        shutil.rmtree(path)
    run([binary, "init", "--path", str(path)], env=env())
    set_model(path, model_dir)


def scan(binary, store):
    run([binary, "--store", str(store), "scan", "--full"], env=env())


def recall(binary, store, q, n, mode, daemon=False):
    r = run([binary, "--store", str(store), "--json", "recall", q, "-n", str(n)], env=env(daemon))
    out = json.loads(r.stdout)
    lexical = any(LEXICAL_MARK in c for c in out.get("caveats", []))
    if lexical != (mode == "lexical"):
        sys.exit(f"mode check failed ({mode}) for {q!r}: caveats {out.get('caveats')}")
    return [h["note_name"] for h in out["hits"]]


def ranked(hits, unit):
    if unit == "block":
        return hits
    seen, out = set(), []
    for h in hits:
        if h not in seen:
            seen.add(h)
            out.append(h)
    return out


# ---------------------------------------------------------------- metrics

def score(ranking, relevant):
    ranks = [i + 1 for i, h in enumerate(ranking) if h in relevant]
    first = ranks[0] if ranks else None
    row = {"rank": first, "mrr": 1 / first if first else 0.0}
    for k in KS:
        row[f"r@{k}"] = 1.0 if first and first <= k else 0.0
    row["rall@10"] = 1.0 if relevant and all(r in ranking[:10] for r in relevant) else 0.0
    dcg = sum(1 / math.log2(r + 1) for r in ranks if r <= 10)
    idcg = sum(1 / math.log2(i + 2) for i in range(min(len(relevant), 10)))
    row["ndcg@10"] = dcg / idcg if idcg else 0.0
    return row


METRICS = [f"r@{k}" for k in KS] + ["rall@10", "mrr", "ndcg@10"]


def summarise(rows):
    if not rows:
        return {"n": 0}
    s = {m: sum(r[m] for r in rows) / len(rows) for m in METRICS}
    s["n"] = len(rows)
    return s


def report(rows, group):
    out = {"all": {m: summarise([r[m] for r in rows]) for m in ("hybrid", "lexical")}}
    for g in sorted({r[group] for r in rows}):
        sub = [r for r in rows if r[group] == g]
        out[g] = {m: summarise([r[m] for r in sub]) for m in ("hybrid", "lexical")}
    return out


def print_table(res):
    print(f"\n{'':<26} {'n':>4}  mode     " + " ".join(f"{m:>7}" for m in METRICS))
    for g, modes in res.items():
        for m, s in modes.items():
            if s["n"]:
                print(f"{g:<26} {s['n']:>4}  {m:<8} " + " ".join(f"{s[x]:>7.3f}" for x in METRICS))


# ---------------------------------------------------------------- latency

def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, max(0, math.ceil(p / 100 * len(xs)) - 1))]


def timed(fn):
    t = time.perf_counter()
    fn()
    return (time.perf_counter() - t) * 1000


def stop_daemon(store):
    subprocess.run(["pkill", "-f", "--", f"--store {store} daemon"], capture_output=True)
    sock = Path(store) / "daemon.sock"
    for _ in range(100):
        if not sock.exists():
            break
        time.sleep(0.05)
    sock.unlink(missing_ok=True)


def latency(binary, store, queries, model_dir, n):
    """Wall-clock per recall, process start included. Leaves the store in hybrid mode."""
    out = {}
    stop_daemon(store)
    for mode, md in (("lexical", Path(store) / "no-model"), ("hybrid", model_dir)):
        set_model(store, md)
        xs = [timed(lambda q=q: recall(binary, store, q, n, mode)) for q in queries]
        out[f"cli_cold_{mode}"] = {"p50_ms": statistics.median(xs), "p95_ms": pct(xs, 95), "n": len(xs)}

    sock = Path(store) / "daemon.sock"
    t0 = time.perf_counter()
    first = timed(lambda: recall(binary, store, queries[0], n, "hybrid", daemon=True))
    ready = None
    while time.perf_counter() - t0 < 60:
        if sock.exists():
            probe = timed(lambda: recall(binary, store, queries[0], n, "hybrid", daemon=True))
            if probe < first / 4:  # answered by the resident process, not locally again
                ready = time.perf_counter() - t0
                break
        time.sleep(0.05)
    if ready is None:
        stop_daemon(store)
        sys.exit("daemon did not come up within 60 s")
    warm = [timed(lambda q=q: recall(binary, store, q, n, "hybrid", daemon=True)) for q in queries]
    stop_daemon(store)
    out["daemon_first_ms"] = first
    out["daemon_ready_s"] = ready
    out["daemon_warm_hybrid"] = {"p50_ms": statistics.median(warm), "p95_ms": pct(warm, 95), "n": len(warm)}
    return out


def print_latency(lat):
    print("\nlatency (wall clock, process start included)")
    for k, v in lat.items():
        if isinstance(v, dict):
            print(f"  {k:<22} p50 {v['p50_ms']:8.1f} ms   p95 {v['p95_ms']:8.1f} ms   (n={v['n']})")
        else:
            print(f"  {k:<22} {v:8.2f}")


# ---------------------------------------------------------------- environment

def git_commit():
    here = Path(__file__).resolve().parent
    r = subprocess.run(["git", "-C", str(here), "rev-parse", "--short", "HEAD"], capture_output=True, text=True)
    d = subprocess.run(["git", "-C", str(here), "status", "--porcelain", "--untracked-files=no"],
                       capture_output=True, text=True)
    return r.stdout.strip() + ("-dirty" if d.stdout.strip() else "")


def hardware():
    cpu = "?"
    try:
        for l in Path("/proc/cpuinfo").read_text().splitlines():
            if l.startswith("model name"):
                cpu = l.split(":", 1)[1].strip()
                break
    except OSError:
        cpu = platform.processor() or "?"
    mem = "?"
    try:
        kb = int(Path("/proc/meminfo").read_text().split()[1])
        mem = f"{kb / 1024 / 1024:.0f} GiB"
    except (OSError, ValueError):
        pass
    return {"cpu": cpu, "cores": os.cpu_count(), "ram": mem, "os": platform.platform()}


def model_profile(binary, store):
    r = run([binary, "--store", str(store), "--json", "status"], env=env(), check=False)
    try:
        s = json.loads(r.stdout)
    except json.JSONDecodeError:
        return "?"

    return ((s.get("index") or {}).get("embedding") or {}).get("id", "?")


def meta(a, binary, store):
    ver = run([binary, "--version"], env=env()).stdout.strip()
    return {"date": dt.datetime.now().astimezone().isoformat(timespec="seconds"), "commit": git_commit(),
            "binary": ver, "profile": model_profile(binary, store), "hardware": hardware(),
            "unit": a.unit, "n": a.n}


# ---------------------------------------------------------------- LongMemEval

def lme_date(s):
    # "2023/05/20 (Sat) 02:21"
    d = dt.datetime.strptime(s.split(" (")[0] + " " + s.split(") ")[1], "%Y/%m/%d %H:%M")
    return d.strftime("%Y-%m-%dT%H:%M:00Z")


def lme_note(name, nid, date, turns):
    body = "\n\n".join(f"{t['role']}: {t['content'].strip()}" for t in turns if t["content"].strip())
    return (f"---\nid: {nid}\nname: {name}\nring: 2\nkind: knowledge\ncreated: {date}\n"
            f"updated: {date}\n---\n\n{body or '(empty session)'}\n")


def lme_question(binary, work, model_dir, q, n, unit, keep):
    store = Path(work) / "lme" / q["question_id"]
    fresh_store(binary, store, model_dir)
    rng = random.Random(q["question_id"])
    ring2 = store / "notes" / "r2"
    names = {}
    for i, (sid, date, turns) in enumerate(zip(q["haystack_session_ids"], q["haystack_dates"], q["haystack_sessions"])):
        name = f"s-{i:03d}"
        names[name] = sid
        (ring2 / f"{name}.md").write_text(lme_note(name, ulid(rng), lme_date(date), turns))
    scan(binary, store)
    relevant = {nm for nm, sid in names.items() if sid in set(q["answer_session_ids"])}
    if not relevant:
        sys.exit(f"{q['question_id']}: no answer session in the haystack")
    row = {"id": q["question_id"], "type": q["question_type"]}
    row["hybrid"] = score(ranked(recall(binary, store, q["question"], n, "hybrid"), unit), relevant)
    set_model(store, store / "no-model")
    row["lexical"] = score(ranked(recall(binary, store, q["question"], n, "lexical"), unit), relevant)
    set_model(store, model_dir)
    if not keep:
        shutil.rmtree(store)
    return row


def cmd_longmemeval(a):
    data = json.loads(Path(a.data).read_text())
    qs = [q for q in data if not q["question_id"].endswith("_abs")]  # abstention: nothing to retrieve
    if a.limit:
        qs = random.Random(0).sample(qs, a.limit) if a.limit < len(qs) else qs
    print(f"LongMemEval: {len(qs)} questions (abstention excluded), jobs {a.jobs}", file=sys.stderr)
    rows, done = [], 0
    with cf.ThreadPoolExecutor(a.jobs) as ex:
        futs = [ex.submit(lme_question, a.bin, a.work, a.model_dir, q, a.n, a.unit, False) for q in qs]
        for f in cf.as_completed(futs):
            rows.append(f.result())
            done += 1
            if done % 25 == 0:
                print(f"  {done}/{len(qs)}", file=sys.stderr)
    res = report(rows, "type")
    print_table(res)

    # latency on one haystack (the first question), kept for that purpose
    q0 = qs[0]
    lme_question(a.bin, a.work, a.model_dir, q0, a.n, a.unit, keep=True)
    store = Path(a.work) / "lme" / q0["question_id"]
    lat_qs = [q["question"] for q in qs[: a.latency_queries]]
    lat = latency(a.bin, store, lat_qs, a.model_dir, a.n)
    print_latency(lat)
    out = {"meta": meta(a, a.bin, store), "dataset": {"name": "LongMemEval-S (cleaned)", "file": Path(a.data).name,
           "questions": len(qs), "latency_store_notes": len(q0["haystack_session_ids"])},
           "results": res, "latency": lat}
    shutil.rmtree(store)
    write_out(a, out)


# ---------------------------------------------------------------- a real store

def cmd_store(a):
    qs = tomllib.loads(Path(a.queries).read_text())["query"]
    store = Path(a.work) / "store"
    fresh_store(a.bin, store, a.model_dir)
    shutil.rmtree(store / "notes")
    shutil.copytree(Path(a.source) / "notes", store / "notes")
    known = {p.stem for p in (store / "notes").rglob("*.md")}
    unknown = {e for q in qs for e in q["expect"] if e not in known}
    if unknown:
        sys.exit(f"query set names {len(unknown)} notes that are not in the store")
    t = time.perf_counter()
    scan(a.bin, store)
    scan_s = time.perf_counter() - t

    rows = [{"kind": q["kind"]} for q in qs]
    for mode, md in (("hybrid", a.model_dir), ("lexical", store / "no-model")):
        set_model(store, md)
        for r, q in zip(rows, qs):
            r[mode] = score(ranked(recall(a.bin, store, q["q"], a.n, mode), a.unit), set(q["expect"]))
    set_model(store, a.model_dir)
    res = report(rows, "kind")
    print_table(res)
    lat = latency(a.bin, store, [q["q"] for q in qs[: a.latency_queries]], a.model_dir, a.n)
    print_latency(lat)
    out = {"meta": meta(a, a.bin, store), "dataset": {"name": "private store copy", "notes": len(known),
           "questions": len(qs), "scan_full_s": scan_s}, "results": res, "latency": lat}
    if not a.keep:
        shutil.rmtree(store)
    write_out(a, out)


def write_out(a, out):
    if a.out:
        Path(a.out).write_text(json.dumps(out, indent=2, ensure_ascii=False) + "\n")
        print(f"\nwritten: {a.out}", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("longmemeval", "store"):
        p = sub.add_parser(name)
        p.add_argument("--bin", default="target/release/cyberbrain")
        p.add_argument("--model-dir", required=True, type=lambda s: Path(s).resolve())
        p.add_argument("--work", required=True, type=lambda s: Path(s).resolve())
        p.add_argument("-n", type=int, default=100, help="blocks asked for per recall (default 100)")
        p.add_argument("--unit", choices=("note", "block"), default="note")
        p.add_argument("--latency-queries", type=int, default=20)
        p.add_argument("--out", help="write the numbers as JSON here")
        if name == "longmemeval":
            p.add_argument("--data", required=True)
            p.add_argument("--limit", type=int, help="random sample of this many questions (seed 0)")
            p.add_argument("--jobs", type=int, default=4)
        else:
            p.add_argument("--source", required=True, help="store whose notes/ tree is copied (read only)")
            p.add_argument("--queries", required=True)
            p.add_argument("--keep", action="store_true")
    a = ap.parse_args()
    a.bin = str(Path(a.bin).resolve())
    if not (a.model_dir / "manifest.json").exists():
        sys.exit(f"{a.model_dir}: no manifest.json — hybrid would silently run lexical")
    # <work>/lme/<question id>/daemon.sock must fit sockaddr_un (the daemon refuses > 100)
    if len(str(a.work)) > 60:
        sys.exit(f"--work {a.work} is too long for the daemon socket; use a short path like /tmp/cbb")
    a.work.mkdir(parents=True, exist_ok=True)
    {"longmemeval": cmd_longmemeval, "store": cmd_store}[a.cmd](a)


if __name__ == "__main__":
    main()
