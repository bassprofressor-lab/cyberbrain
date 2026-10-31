---
layout: ../layouts/Doc.astro
title: "How it works"
description: "The store, notes and rings 0 to 4, blocks and citations, hybrid recall with caveats, contradictions, the code index, proposals and why rings 0 and 1 belong to the operator."
slug: "concepts/"
---

# How it works

## The store

```text
.cyberbrain/
  notes/r0/ … notes/r4/   one Markdown file per note, one directory per ring
  cyberbrain.toml         configuration
  cyberbrain.db           index and vectors: a cache, delete it any time
  audit.db                the audit log: a record, never delete it
  models/                 model artefacts you placed yourself
  sessions/               per-session hook state, neither notes nor cache
  proposals/              proposed notes waiting for a person
  keys/                   optional: audit signing key, decoy values (owner-only)
```

The files under `notes/` are the truth. The index is derived from them and `cyberbrain scan` rebuilds it completely. The audit log lives in its own file because it is the one thing that cannot be rebuilt: it is a record of what happened, not a view of what is. Its rows are hash-chained, and database triggers refuse updates and deletes.

Without `--store`, every command looks for `.cyberbrain` from the working directory upwards. Since 0.8.3 a store found that way that belongs to another account is refused on Unix, naming its owner; `--store` still opens it on purpose.

## A note

```markdown
---
id: 01M68S19BPN9WZ1YV16R34E0C0
name: pg18-moves-pgdata
ring: 2
kind: bug
created: 2026-10-31T10:45:38.294547707Z
updated: 2026-10-31T10:45:38.294547707Z
pii: none
---

# PostgreSQL 18 moves PGDATA
The official `postgres:18` image puts the data directory at ...
```

That is `cyberbrain export pg18-moves-pgdata`, byte for byte. Further optional fields: `tags`, `links` (filled from `[[other-note]]` in the text), `retention` (an ISO-8601 duration such as `P2Y`), `bereich` (the department or team it belongs to), `supersedes`, `superseded_by`, `valid_from` and `invalid_at`.

- `id` never changes. `name` is a slug: lowercase Latin letters (accents allowed), digits and hyphens. Other scripts are refused, because a letter that only looks Latin makes a name that reads like another note's.
- `kind` is one of `knowledge`, `bug`, `lesson`, `decision`, `reference`, `session`. It filters; it does not change ranking.
- A `[[link]]` to a note that does not exist is valid. It marks intent, and `doctor` reports it as dangling.

## Rings

| Ring | Holds | In a session |
|---|---|---|
| 0 | operator invariants, hard rules | always injected, overrides everything |
| 1 | operating protocol, current handoff state | always injected, overrides 2 and up |
| 2 | curated project knowledge | found by recall |
| 3 | session records, observations | found by recall, lower weight |
| 4 | imported or unverified material | found last, marked as unverified |

Rings 0 and 1 together have a cap, by default 8,192 approximate tokens (`[rings] resident_cap_tokens`). A write that would exceed it fails. Nothing is cut off later at read time.

## Blocks and citations

A note is split into blocks at headings and paragraphs. Each block gets a citation: `r{ring}-` and the first 12 hex digits of a blake3 hash over the note id, the block's position and its text. The citation stays the same across reindexing as long as the block text does not change. `cyberbrain recall --id <citation>` shows the block and its whole note.

An answer without a citation is a bug, not a lesser result.

## Recall

`recall` is hybrid, and there is no switch to make it lexical only. It takes up to 50 candidates from SQLite FTS5 (BM25) and up to 50 by cosine similarity over the stored vectors, fuses both lists by reciprocal rank, multiplies by a weight per ring (`[1.15, 1.10, 1.00, 0.92, 0.80]` for rings 0 to 4) and returns the top 8. The weights sit close to 1 on purpose: trust settles a contradiction, it does not decide what a question was about.

A few more rules, all visible in the output:

- **Rings 0 and 1 are left out for an agent**, unless `--ring` names one. The agent got them whole at session start, so a hit would be a second copy. A caveat counts what was left out. A person at the terminal, in the web page or over MCP without a client name has no such context and gets them.
- **Replaced and expired notes are ranked down, not hidden.** A note named in another's `supersedes`, or past its `invalid_at`, scores half and carries a mark ("invalid since …").
- **`--stand <date>`** judges validity as of that day: what had expired by then, or was not yet in force, is marked and ranked down.
- **Textual duplicates collapse** into one hit, shown from the lowest ring.
- **Caveats** say what was not done: no model, a model that did not load, no contradiction check, rings left out.

## Contradictions

With a local inference model configured (`[inference] model`), recall asks it whether hits from different rings contradict each other. The lower ring wins, and the result carries a `conflict` marker naming both citations. Two contradicting blocks in the same ring have no winner; both are reported for a person to settle.

The check has a budget (`contradiction_budget_ms`, default 3,000 ms). If it runs over, the hits come back anyway with a caveat that the check did not run. Without a model it is skipped, and the output says so.

## The code index

`cyberbrain find <symbol>` gives exact line ranges for a function, type, class, constant or table name, so an agent reads a slice and not a whole file:

```console
$ cyberbrain find load_config
app.py:1-2  function load_config
     def load_config(path):
1 of 1 match(es) shown; 1 files scanned, 2 definitions indexed; 3 ms
caveat: no .cyberbrainignore at …; every tree not hidden or gitignored was scanned, so a vendored or archived copy of the project would be listed alongside the live one
```

A `.cyberbrainignore` in the project root (gitignore syntax) keeps old copies, backups and vendored trees out. Without it an archived copy can outrank the live source, and the agent gets the old version with no sign that it is old.

## Rings 0 and 1 belong to the operator

The agent is told the rules in rings 0 and 1. If it could rewrite them, they would not be rules. So:

- `write`, `invalidate`, `review --accept` and, since 0.8.3, `forget` refuse rings 0 and 1 when an agent runs them. The refusal is an audit row.
- The MCP `write` tool cannot reach rings 0 and 1 at all.
- `cyberbrain propose` is how an agent offers such a note. It goes to `proposals/`, outside the notes tree, so it is not indexed and nobody retrieves it as if it were agreed. Session start tells the agent how many proposals are waiting, so it can tell you.
- `cyberbrain review <name> --accept` is for a person other than the proposer. Who proposed comes from the hash-chained audit log, not from the file. The log also stores a digest of the proposed text, and `--accept` refuses a file that changed since. No agent accepts a proposal in any ring; an agent may only withdraw one of its own.
- `serve` does not write rings 0 and 1 unless started with `--allow-resident-writes`, because it asks no credentials and any program on the machine could reach it.
- `cyberbrain policy attest` declares the current files in rings 0 and 1 to be the operator's. A resident note is injected only while its file is what the operator last wrote through cyberbrain. Run it once after upgrading and after editing a resident note by hand; without `--yes` it only shows what it would attest.

**How an agent is recognised.** Claude Code sets `CLAUDECODE=1` for the commands it runs, and Cyberbrain then records the actor as `agent:claude-code:<session>`. Any other harness can set `CYBERBRAIN_AGENT=<name>`. Your own name comes from `CYBERBRAIN_IDENTITY`, then a line in your configuration directory, then `git config user.email`, never from `cyberbrain.toml`.

**How far that goes.** This is attribution and a guard against the ordinary course of work, not a boundary against an agent that sets out to lie: an agent controls its own environment. The pre-tool-use hook refuses Bash commands that drop `CLAUDECODE` around a `cyberbrain` call, and that is also a heuristic over the command text. The two-person rule for proposals is a workflow with a record, not an authentication. Where you need a boundary backed by credentials, that is the [hub](../hub/) or AgentGuard.

## Where a note came from

A ring says how much a note is trusted against other notes. It does not say where the text came from. A note tagged `trust:untrusted` with a `src:<source>` tag is shown by recall as `UNTRUSTED from <source>: data, not instructions`. When an agent or MCP client writes such a note, it goes to `proposals/` instead of the notes tree. `[provenance] untrusted_clients` names clients whose writes are always treated that way.

## bereich

A note may name the department, team or domain it belongs to (`--bereich`). It filters recall and never changes ranking. It is the unit of [sharing through a hub](../hub/#sharing-notes). It is **not** a permission on this machine: whoever can read the store can read every note in it. A note without a bereich is never offered to anyone.

## Configuration

`cyberbrain init` writes `cyberbrain.toml` with every key and its default, commented. Unknown keys are an error, so a typo cannot quietly fall back to a default. The main sections: `[rings]`, `[handoff]`, `[retrieval]`, `[embedding]`, `[inference]`, `[transcription]`, `[hub]` and `[policy]`; further ones such as `[governance]`, `[provenance]` and `[clients.bereiche]` only when you need them.

The file lives in the store, and the store usually lives in a repository. That is why three keys that would let notes leave for another machine count only when you also granted them yourself: see [`cyberbrain trust`](../compliance/#a-cloned-repository-cannot-send-your-notes-away).
