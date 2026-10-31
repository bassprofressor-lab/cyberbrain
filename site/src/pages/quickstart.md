---
layout: ../layouts/Doc.astro
title: "Five minutes"
description: "Create a store, write a note, find it again, expand a citation, propose a ring 0 rule and check the store's health. With the real output of 0.8.4."
slug: "quickstart/"
---

# Five minutes

Everything below was run against 0.8.4 in an empty directory. Paths are shortened to `.cyberbrain/…`.

## 1. A store

```console
$ cyberbrain init
Created a store at .cyberbrain
  config: .cyberbrain/cyberbrain.toml
  audit record: .cyberbrain/audit.db
  index cache: .cyberbrain/cyberbrain.db
```

That is the whole store: `notes/r0` to `notes/r4` for the Markdown, `cyberbrain.toml` with every setting commented, `audit.db` as the record and `cyberbrain.db` as a cache you may delete. Every later command finds the store by walking up from the working directory, so it works anywhere inside the project.

## 2. A note

```console
$ cyberbrain write --ring 2 --kind bug --name pg18-moves-pgdata --body '# PostgreSQL 18 moves PGDATA
The official `postgres:18` image puts the data directory at `/var/lib/postgresql/18/docker` instead of `/var/lib/postgresql/data`. A volume mounted at the old path stays empty.'
wrote pg18-moves-pgdata in ring r2 as .cyberbrain/notes/r2/pg18-moves-pgdata.md (377 bytes, 1 blocks, 0 vectors, 0 links, pii: None)
id: 01M68S19BPN9WZ1YV16R34E0C0
no vectors: no model artefact at .cyberbrain/models/model2vec (expected model.safetensors and tokenizer.json); search is lexical only
```

Without `--body` the text is read from stdin. The write checks the text for personal data first, appends an audit row and indexes the note. There is no separate step; `cyberbrain scan` is only needed after you edit files by hand.

The last line is the first honest caveat: no model has been placed yet, so there are no vectors. [Search and models](../search/) explains how to add one.

## 3. Find it

```console
$ cyberbrain recall 'postgres data directory'
1. r2-6a203a50f6a4  r2  pg18-moves-pgdata  2026-10-31  (100% of top)
     # PostgreSQL 18 moves PGDATA
     The official `postgres:18` image puts the data directory at `/var/lib/postgresql/18/docker` instead of `/var/lib/postgresql/data`. A volume mounted at the old path stays empty.
caveat: semantic search skipped: no embedder configured; hits are lexical only
caveat: contradiction check skipped: fewer than two hits, nothing to compare
```

`r2-6a203a50f6a4` is the citation: ring 2, and 12 hex digits that identify the block. Expand it:

```console
$ cyberbrain recall --id r2-6a203a50f6a4
r2-6a203a50f6a4 is block #0 of pg18-moves-pgdata (r2), ~72 tokens:

# PostgreSQL 18 moves PGDATA
The official `postgres:18` image puts the data directory at ...

--- full note ---
.cyberbrain/notes/r2/pg18-moves-pgdata.md
id: 01M68S19BPN9WZ1YV16R34E0C0  name: pg18-moves-pgdata  ring: r2  kind: bug  pii: None
blocks: r2-6a203a50f6a4
```

## 4. A rule for every session

Rings 0 and 1 are injected into every session. At your own terminal you write them like any other ring. An agent cannot: when Claude Code runs the same command, it is refused and recorded.

```console
$ cyberbrain write --ring 0 --kind decision --name never-push --body 'Never push without asking.'
cyberbrain: refused: refused by policy (ring-owner): ring 0 belongs to the operator; agent:claude-code:d7774d4e may not write it (note never-push). Propose the text instead — `cyberbrain propose --ring 0 --kind … --name never-push` — and the operator accepts it with `cyberbrain review never-push --accept`
```

So the agent proposes:

```console
$ cyberbrain propose --ring 0 --kind decision --name never-push --body 'Never push without asking.'
Proposed never-push for ring 0 as .cyberbrain/proposals/never-push.md
  by: agent:claude-code:d7774d4e
  196 bytes

It is not in the index, so recall cannot find it. Somebody else runs `cyberbrain review` to see it.
```

and you decide:

```console
$ cyberbrain review
1 proposal

  never-push  r0  decision
      by agent:claude-code:d7774d4e, 2026-10-31T10:45:45.141248942Z

`cyberbrain review <name> --accept`, or `--reject --reason "…"`.

$ cyberbrain review never-push --accept
```

An agent that tries the last command is refused as well. [How it works](../concepts/#rings-0-and-1-belong-to-the-operator) says how Cyberbrain tells the two apart, and how far that goes.

## 5. Is everything in order?

```console
$ cyberbrain status
store: .cyberbrain
notes on disk: 1 (r0 0, r1 0, r2 1, r3 0, r4 0); 0 files skipped
resident rings 0+1: ~0 of 8192 tokens
index: schema v5, 1 notes, 1 blocks, 0 vectors, 0 links (0 dangling)
audit: 4 rows in .cyberbrain/audit.db, chain verified over 4 rows
embedding: no embedding model: ... search is lexical only
inference: http://127.0.0.1:11434/v1 model (none): not in use: no inference model is configured (inference.model in cyberbrain.toml); the endpoint is never contacted without one
policy: profile eu (GDPR, Regulation (EU) 2016/679), PII scan active
  ...

$ cyberbrain doctor
clean: 13 checks, nothing to report
checks: waivers, notes tree, stale index, dangling links, unresolvable links, supersession, shared blocks, ring cap, session-start size, index integrity, embedding profile, audit chain, retention
```

## Next

- Connect Claude Code: `cyberbrain install --client claude-code`, see [Claude Code](../claude-code/).
- Every command and flag: [Commands](../commands/).
