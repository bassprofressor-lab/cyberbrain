---
layout: ../layouts/Doc.astro
title: "Cyberbrain documentation"
description: "Cited, trust-tiered, local-first memory for AI coding agents. Install, rings and citations, commands, Claude Code hooks and MCP, semantic search, the team hub and the EU compliance layer."
slug: ""
---

# Cyberbrain

<p class="lead">Cited, trust-tiered, local-first memory for AI coding agents. One native binary, notes in plain Markdown, and every path by which data could leave the machine listed in one place you can print.</p>

An agent forgets everything between sessions. Cyberbrain gives it notes that stay: Markdown files in your repository, a hybrid search over them, and a citation on every hit that leads back to the exact block it came from. Rings 0 and 1 hold the rules of the project and go into every session; the agent may propose changes to them, but only a person accepts them.

<div class="btns">
  <a class="btn primary" href="install/">Install</a>
  <a class="btn ghost" href="quickstart/">Five minutes</a>
  <a class="btn ghost" href="https://github.com/bassprofressor-lab/cyberbrain">Source on GitHub</a>
</div>

```console
$ cyberbrain recall 'postgres data directory'
1. r2-6a203a50f6a4  r2  pg18-moves-pgdata  2026-10-31  (100% of top)
     # PostgreSQL 18 moves PGDATA
     The official `postgres:18` image puts the data directory at `/var/lib/postgresql/18/docker` ...
caveat: semantic search skipped: no embedder configured; hits are lexical only
caveat: contradiction check skipped: fewer than two hits, nothing to compare
```

The caveats matter as much as the hit. Cyberbrain says what it did **not** do: here, no model was installed, so the search was lexical, and with one hit there was nothing to compare. An unchecked result never looks like a checked one.

<div class="cards">
  <div class="card"><div class="k">version</div><div class="v">0.8.4</div><div class="s">released 2026-10-31</div></div>
  <div class="card"><div class="k">Linux binary</div><div class="v">20 MB</div><div class="s">web page included</div></div>
  <div class="card"><div class="k">platforms</div><div class="v">3</div><div class="s">Linux, Windows, macOS (Apple silicon)</div></div>
  <div class="card"><div class="k">telemetry</div><div class="v">none</div><div class="s">there is no code for it</div></div>
</div>

<sub>Size of the 0.8.4 release file `cyberbrain-linux-x86_64` (20,024,408 bytes).</sub>

## Four ways in

All four are the same binary and work on the same store.

| | |
|---|---|
| [`cyberbrain <command>`](commands/) | everything from the command line, `--json` on every command that prints |
| [`cyberbrain hook <event>`](claude-code/#the-hooks) | Claude Code lifecycle events: rings 0 and 1 at session start, the store guard before each tool call |
| [`cyberbrain mcp`](claude-code/#the-mcp-server) | Model Context Protocol over stdio (or HTTP with a token per client): `recall`, `recall_id`, `find`, `write`, `status` |
| [`cyberbrain serve`](serve/) | the web page and the HTTP API, on loopback, for your own account |

## Where to go next

- New here: [Install](install/), then [five minutes](quickstart/).
- What rings, citations and proposals are: [How it works](concepts/).
- Hooking it into an agent: [Claude Code](claude-code/).
- Better search than keywords: [Search and models](search/).
- More than one machine: [Team hub](hub/).
- Personal data, audits, erasure: [Running it in the EU](compliance/).
- Only want the small one: [Light or full](light-or-full/).

## Status

0.8.4 is young. The README for this release counts 1,097 tests and 18 browser tests, and says it has been run against one operator's real corpus (1,086 notes across five projects) and not much else. The Windows installer is the newest part. If something misbehaves, an [issue](https://github.com/bassprofressor-lab/cyberbrain/issues) helps.

Licence: [FSL-1.1-ALv2](https://github.com/bassprofressor-lab/cyberbrain/blob/main/LICENSE.md). Use it for anything except building a competing product; each release becomes Apache-2.0 two years after it ships.
