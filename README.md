<div align="center">

<img src="docs/images/logo.svg" width="88" alt="">

# Cyberbrain

**Cited, trust-tiered, local-first memory for AI coding agents.**

[![CI](https://github.com/bassprofressor-lab/cyberbrain/actions/workflows/ci.yml/badge.svg)](https://github.com/bassprofressor-lab/cyberbrain/actions/workflows/ci.yml)
[![Licence: FSL-1.1-ALv2](https://img.shields.io/badge/licence-FSL--1.1--ALv2-blue)](LICENSE.md)
[![Rust 1.98+](https://img.shields.io/badge/rust-1.98%2B-b7410e)](rust-toolchain.toml)
[![Linux and Windows](https://img.shields.io/badge/runs%20on-Linux%20%C2%B7%20Windows-333)](#install)
[![v0.1.0](https://img.shields.io/badge/version-0.1.0-lightgrey)](CHANGELOG.md)

[Install](#install) · [Start](#start) · [Compliance](#built-for-the-eu-switchable-off) ·
[Spec](docs/SPEC.md) · [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md)

</div>

One native binary. Your notes stay plain Markdown you can read, edit and grep. Nothing
leaves the machine unless you say so, and every path that could is listed in one place you
can print.

<img src="docs/images/search.png" alt="Recall over a project's notes: each hit carries its ring, its citation and its score">

<sub>A question in the words you would use months later. Each hit carries the ring it came
from, a citation that resolves back to the block, and its score. Screenshots are from a
demo store; the notes in them are invented.</sub>

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

```console
$ cargo install cyberbrain
$ cyberbrain init
```

The web page is compiled into the binary and ships inside the crate, so this needs no node
toolchain. Leave the page out if you would rather not carry it; the CLI, the hooks, the MCP
server and the HTTP API are unaffected, and `serve` answers the API while saying the page was
not built in:

```console
$ cargo install cyberbrain --no-default-features
```

From a checkout, build the page first — there it is not packaged, it is built:

```console
$ git clone https://github.com/bassprofressor-lab/cyberbrain && cd cyberbrain
$ (cd ui && npm ci && npm run build)
$ cargo install --path crates/cyberbrain
```

There is no release binary yet. When there is, it will be on the releases page.

Linux and Windows. The binary needs nothing at runtime: no system SQLite, no OpenSSL, no
model server, no node.

## Start

```console
$ cyberbrain init
$ cyberbrain write --ring 2 --kind bug --name pg18-moves-pgdata --body 'what you learned'
$ cyberbrain scan
$ cyberbrain recall 'what you learned'
```

Four ways in, all from the same binary:

| | |
|---|---|
| `cyberbrain <command>` | everything is reachable from the command line, `--json` on all of it |
| `cyberbrain hook <event>` | six agent lifecycle events; measured p99 of 4 ms including process start, against a 15 ms budget |
| `cyberbrain mcp` | Model Context Protocol over stdio, for any client that speaks it |
| `cyberbrain serve` | the web page and the HTTP API, on loopback, with no authentication because it never leaves the machine |

<details>
<summary>What the web UI shows (three screenshots)</summary>

<br>

**Status** — what the store holds, what the index knows, which model is loaded, and a panel
that lists what the page *cannot* measure.

<img src="docs/images/status.png" alt="Status screen">

**Graph** — notes and the links between them, by ring. A link to a note that does not exist
yet is drawn as intent rather than as an error.

<img src="docs/images/graph.png" alt="Graph screen">

The page is compiled into the binary, speaks English and German, and fetches nothing from
the network.

</details>

### Semantic search needs a model, and it will tell you if it has none

Out of the box search is lexical, and every result says so in a caveat. Lexical means
literal: without a model, `postgres` does not find `PostgreSQL`, and the words you search
for are the words that have to be in the paragraph. To turn on semantic search, place a
model2vec artefact — `model.safetensors`, `tokenizer.json` and a
`manifest.json` carrying the blake3 digest of each — under `<store>/models/model2vec`.
Nothing is downloaded on your behalf unless you set `embedding.model_source` and
`embedding.model_download_consent` in the config, and even then it happens once, through the
one registered outbound path.

## Built for the EU, switchable off

<img src="docs/images/compliance.png" alt="The compliance screen: nothing has left this machine, and the obligation catalogue below it">

<sub>The headline is computed from the egress register, not typed in. Below it, what the
active profile claims the law says — with the article each line rests on and how sure the
author is of it.</sub>

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

**v0.1.0, and young.** 447 tests, seven crates, clippy and rustfmt clean. It has been run
against one operator's real corpus — 1,086 notes across five projects — and not much else.
Expect rough edges, report them.

Cyberbrain is an original work. It shares no source code with any other memory tool; §0 of
[`docs/SPEC.md`](docs/SPEC.md) records the boundary it was built under, and the commit
history documents it decision by decision.

## Contributing and security

[`CONTRIBUTING.md`](CONTRIBUTING.md) before the first pull request: the clean-room rule in
SPEC §0 binds contributors, and a contribution that breaks it breaks the provenance claim
above retroactively. Vulnerabilities go through [`SECURITY.md`](SECURITY.md), not a public
issue.

## Licence

[FSL-1.1-ALv2](LICENSE.md). Use it for anything except building a competing product, and
each release becomes Apache-2.0 two years after it ships. Copyright 2026 Krynex Labs.
