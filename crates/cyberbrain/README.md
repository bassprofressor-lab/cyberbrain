# Cyberbrain

**Cited, trust-tiered, local-first memory for AI coding agents.**

One native binary. Your notes stay plain Markdown you can read, edit and grep. Nothing leaves
the machine unless you say so, and every path that could is listed in one place you can print.

```console
$ cyberbrain recall 'postgres data directory'
1. r2-867ef2a8cd01  r2  pg18-moves-pgdata  (100% of top)
     # PostgreSQL 18 moves PGDATA
     The official `postgres:18` image puts the data directory at
     `/var/lib/postgresql/18/docker` instead of `/var/lib/postgresql/data` ...
caveat: contradiction check skipped: no inference model is configured
```

That last line is the point as much as the hit is. The tool says what it did **not** check.

**Cited.** Every retrieved statement carries an identifier that resolves back to the exact
source block. An answer without a citation is a bug, not a degraded result.

**Trust-tiered.** Notes live in numbered rings. Ring 0 holds operator invariants and overrides
everything; ring 4 is unverified material. When two blocks contradict, the lower ring wins and
the conflict is reported rather than silently resolved.

**Local-first.** Hybrid lexical and semantic search, embeddings computed on your machine. The
core performs no network I/O at all. Optional local inference over any OpenAI-compatible
endpoint.

Compliance is a subsystem rather than a section in the docs: an egress register you can print,
erasure that erases, subject access, a PII check that asks before it writes, an append-only
audit log with a blake3 chain, and the obligation catalogue of the active profile with the
article each line rests on.

## Install

```console
$ cargo install cyberbrain                       # CLI, hooks, MCP, HTTP API, web page
$ cargo install cyberbrain --no-default-features # ...without the embedded page
$ cyberbrain init
```

## More

Screenshots, the full description and the specification are in the repository:
<https://github.com/bassprofressor-lab/cyberbrain>

Licence: [FSL-1.1-ALv2](https://github.com/bassprofressor-lab/cyberbrain/blob/main/LICENSE.md).
Source-available; each release becomes Apache-2.0 two years after it ships.
