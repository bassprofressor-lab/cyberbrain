# Cyberbrain

Cited, trust-tiered, local-first memory for AI coding agents.

One native binary. Your notes stay plain Markdown you can read, edit and grep. Nothing
leaves the machine unless you say so, and every path that could is listed in one place you
can print.

```console
$ cyberbrain recall 'postgres data directory'
1. r2-867ef2a8cd01  r2  pg18-moves-pgdata  (100% of top)
     # PostgreSQL 18 moves PGDATA
     The official `postgres:18` image puts the data directory at
     `/var/lib/postgresql/18/docker` instead of `/var/lib/postgresql/data` ...
caveat: contradiction check skipped: no inference model is configured
```

That last line is the point as much as the hit is. The tool says what it did **not** check.

## What it is for

An agent that forgets everything between sessions relearns the same things and repeats the
same mistakes. Cyberbrain gives it a memory that survives, and three properties that keep
that memory worth trusting:

**Cited.** Every retrieved statement carries an identifier that resolves back to the exact
source block. `cyberbrain recall --id r2-867ef2a8cd01` expands it. An answer without a
citation is a bug, not a degraded result.

**Trust-tiered.** Notes live in numbered rings. Ring 0 holds operator invariants and
overrides everything; ring 4 is unverified material. Rings 0 and 1 are injected into every
session and are size-capped so that stays affordable. When two blocks contradict, the lower
ring wins and the conflict is reported rather than silently resolved.

**Local-first.** Hybrid lexical and semantic search, embeddings computed on your machine.
The core performs no network I/O at all. Optional local inference over any OpenAI-compatible
endpoint — Ollama, LM Studio, llama.cpp, or NVIDIA PAIR spreading the work across an RTX
box, a DGX Spark and an Apple M4 Mac.

## Install

Prebuilt binaries are on the releases page. From source:

```console
$ cargo install cyberbrain                      # CLI, hooks, MCP, HTTP API
$ cargo install cyberbrain --features ui        # ...and the embedded web page
```

The `ui` feature compiles the web page into the binary, which means building it needs a
node toolchain. That is deliberately not the price of installing: without it the CLI, the
hooks, the MCP server and the HTTP API are all unaffected, and `serve` answers the API
while saying the page was not built in. From a source checkout, build the page first:

```console
$ cd ui && npm ci && npm run build
```

Linux and Windows. The binary needs nothing at runtime: no system SQLite, no OpenSSL, no
model server, no node.

## Start

```console
$ cyberbrain init
$ cyberbrain write --ring 2 --kind bug --name pg18-moves-pgdata --body 'what you learned'
$ cyberbrain scan
$ cyberbrain recall 'what you learned'
```

Wire it into an agent harness with `cyberbrain hook <event>`, serve the web UI with
`cyberbrain serve`, or speak MCP over stdio with `cyberbrain mcp`.

### Semantic search needs a model, and it will tell you if it has none

Out of the box search is lexical, and every result says so in a caveat. To turn on semantic
search, place a model2vec artefact — `model.safetensors`, `tokenizer.json` and a
`manifest.json` carrying the blake3 digest of each — under `<store>/models/model2vec`.
Nothing is downloaded on your behalf unless you set `embedding.model_source` and
`embedding.model_download_consent` in the config, and even then it happens once, through the
one registered outbound path.

## Built for the EU, switchable off

Compliance is a subsystem, not a section in the docs. `cyberbrain policy egress` prints
every path by which bytes can leave the machine, what each carries, and whether it is on.
Today that list has two entries and ends with "Telemetry does not exist."

- **Erasure that erases** (GDPR Art. 17): `cyberbrain forget` removes the note, its blocks,
  its vectors, its index rows and its derivatives in one transaction, and prints what went.
- **Subject access** (Art. 15): `cyberbrain policy subject <identifier>` returns everything
  stored about it, with citations.
- **A PII check before a note is written**, holding the write for your decision rather than
  redacting behind your back.
- **An append-only audit log** with a blake3 hash chain, and a `verify` that names the first
  altered row.
- **Retention** per note, applied on request and never silently in the background.

`policy.profile` is `eu`, `ch` or `off`. Switzerland is a separate profile rather than "EU
minus", because the revised FADP differs in substance and folding them together produces
claims that are wrong in one of the two countries. Every obligation carries a confidence,
and nothing below high confidence drives behaviour.

## Status

**v0.1.0, and young.** 423 tests, seven crates, clippy and rustfmt clean. It has been run
against one operator's real corpus — nearly 900 notes across five projects — and not much
else. Expect rough edges, report them.

Cyberbrain is an original work. It shares no source code with any other memory tool; §0 of
[`docs/SPEC.md`](docs/SPEC.md) records the boundary it was built under, and the commit
history documents it decision by decision.

## Licence

[FSL-1.1-ALv2](LICENSE.md). Use it for anything except building a competing product, and
each release becomes Apache-2.0 two years after it ships. Copyright 2026 Krynex Labs.
