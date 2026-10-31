---
layout: ../layouts/Doc.astro
title: "Commands"
description: "Every cyberbrain command and its options in 0.8.4: init, write, recall, find, propose, review, forget, scan, status, doctor, trust, install, serve, mcp, hook, policy, canary, hub and more."
slug: "commands/"
---

# Commands

```text
cyberbrain [OPTIONS] <COMMAND>
```

This page follows `cyberbrain --help` and `cyberbrain <command> --help` of 0.8.4. When in doubt, the help of your binary is right and this page is wrong.

## Options for every command

| Option | |
|---|---|
| `--store <STORE>` | The store to work on. Without it, `.cyberbrain` is searched from the working directory upwards. Also `CYBERBRAIN_STORE`. |
| `--json` | Machine-readable output. Every command that prints anything supports it. |
| `-q`, `--quiet` | Print less. Errors still go to stderr. |
| `-h`, `--help` | Help. `--help` is longer than `-h` for some commands. |
| `-V`, `--version` | Version. |

## Notes

### init

Create a store and a commented `cyberbrain.toml`.

| Option | |
|---|---|
| `--path <PATH>` | where to create it; default `.cyberbrain` in the working directory |

### write

Write a note. The body comes from stdin unless `--body` is given.

```text
cyberbrain write --ring <RING> --kind <KIND> --name <NAME> [--body <BODY>] [options]
```

| Option | |
|---|---|
| `--ring <RING>` | 0 to 4; lower is more trusted and wins a contradiction. Rings 0 and 1 are refused for agents. |
| `--kind <KIND>` | `knowledge`, `bug`, `lesson`, `decision`, `reference`, `session` |
| `--name <NAME>` | slug, unique in the store |
| `--body <BODY>` | Markdown; omit to read stdin |
| `--tags <TAGS>` | one tag; repeat the flag for several |
| `--bereich <BEREICH>` | department, team or domain; filters recall, never ranking |
| `--retention <RETENTION>` | ISO-8601 duration such as `P2Y`; absent means keep indefinitely |
| `--supersedes <NAME>` | a note this one replaces; repeatable. The old note is ranked lower and marked, not changed. |
| `--valid-from <DATE>` | from when the content holds: `YYYY-MM-DD` (00:00 UTC) or RFC 3339 |
| `--invalid-at <DATE>` | from when it no longer holds; recall still finds it, ranked down and marked |
| `--force` | write despite PII findings, recorded as flagged |
| `--dry-run` | run the real path with no-op writers and report what would happen |

When the PII check finds something, nothing is written and the exit code is 3:

```console
$ cyberbrain write --ring 2 --kind knowledge --name contact-anna --body 'Ask anna.schmidt@example.com about the VPN.'
Write held: 1 possible personal data item(s) found.
  line 1, bytes 4..28: email [low] email shape, but a documentation domain (RFC 2606)
    anna.schmidt@example.com
Options: redact, mark reviewed, proceed flagged.
PII detection is heuristic: it finds the shapes of emails, IP addresses, API keys, IBANs and phone numbers. It misses what has no shape and flags some things that are harmless. It is a seatbelt, not a guarantee.
Nothing was written. Re-run with --force to write it flagged, or edit the body.
```

### propose

The same arguments as `write` (without the validity and supersession flags) and the same PII check, but the note goes to `proposals/` for somebody else to accept. It is not indexed. `--force` proposes despite PII findings.

### review

```text
cyberbrain review [TARGET] [--accept | --reject --reason <REASON>] [--force] [--from-agentguard] [--dry-run]
```

Without a target: the list of waiting proposals. `--accept` moves the note into its ring and indexes it. `--reject` needs `--reason`, and the proposal file goes. `--force` accepts even though the note changed after it was proposed. `--from-agentguard` accepts because an administrator approved it in AgentGuard; the approval recorded at propose time is read back and must say `approved`. The proposer cannot review their own proposal.

### invalidate

```text
cyberbrain invalidate <NAME> [--at <DATE>] [--by <NAME>] [--clear] [--dry-run]
```

Declare that a note stopped holding from a day on, without rewriting its text. Sets `invalid_at` (and `superseded_by` with `--by`), writes one audit row and reindexes. `--at` defaults to now; `--clear` takes the declaration back.

### forget

```text
cyberbrain forget <TARGET> [--dry-run] [--report <PATH>] [--sign]
```

Erase a note and everything derived from it: file, blocks, search rows, vectors, links and cached derivatives, in one transaction. The audit rows stay; they are the evidence of the erasure. `--report` writes an erasure report that `verify-export` can check, `--sign` signs it with the store's audit key. Since 0.8.3 an agent is refused rings 0 and 1 here too. If the note had a bereich and the store is enrolled with a hub, `forget` says that the hub's copy is untouched and names `hub erase`.

### export

`cyberbrain export <TARGET> [--format md|json]` prints a note, frontmatter included.

### import

Bring an existing tree of Markdown notes into the store, driven by a TOML mapping file.

| Option | |
|---|---|
| `--suggest <FOLDER>` | look at a folder and print a plan instead of importing; nothing is written |
| `--plan <PLAN>` | the mapping file: what to take, what to skip, how to split, which ring |
| `--accept-pii` | accept every PII finding in bulk |
| `--dry-run` | |

### transcribe

```text
cyberbrain transcribe <FILE> [--ring <RING>] [--kind <KIND>] [--name <NAME>] [--tags <TAG>] [--bereich <B>] [--segments] [--print] [--force] [--dry-run]
```

Send a recording (WAV, MP3, M4A, OGG, WebM, FLAC or a video with sound) to the local whisper.cpp server in `[transcription]` and write the text like `write` does. Default ring 3, kind `knowledge`, name `transcript-<file name>-<date>`, and the tag `transcript` is always added. `--segments` writes one paragraph per stretch of speech with its time; `--print` shows the text and writes nothing. Anything but 16 kHz WAV needs the server started with `--convert`.

### propose-session

Summarise a Claude Code session with the local model and file it as a proposal (ring 3, kind `session`). Needs `[inference]`. `--transcript <PATH>` names the `.jsonl`, `--latest` takes the newest transcript of this store's project.

## Finding

### recall

```text
cyberbrain recall [QUERY] [--id <CITATION>] [-n <N>] [--ring <RING>] [--bereich <B>] [--stand <DATE>]
```

| Option | |
|---|---|
| `--id <CITATION>` | expand a citation to its full note instead of searching |
| `-n`, `--n <N>` | how many hits, default 8 |
| `--ring <RING>` | exactly this ring |
| `--bereich <BEREICH>` | only notes of this department, team or domain |
| `--stand <DATE>` | judge validity as of this day (`YYYY-MM-DD`) or moment (RFC 3339) |

### find

`cyberbrain find <SYMBOL> [-l, --limit <N>]` gives exact line ranges for a function, type, class, constant or table name. Default limit 20; past it the result says it was truncated.

### scan

Rebuild the index from the notes tree. `--full` discards the index and rebuilds everything; the audit log is untouched. `--dry-run`.

## Health

### status

Store, index, audit chain, embedding profile, the recall daemon, the inference endpoint and the compliance profile with each egress path.

### doctor

Thirteen checks in 0.8.4: waivers, notes tree, stale index, dangling links, unresolvable links, supersession, shared blocks, ring cap, session-start size, index integrity, embedding profile, audit chain, retention.

### trust

`cyberbrain trust [--grant | --revoke]` shows which waivers this store's `cyberbrain.toml` asks for and whether you granted them. See [Running it in the EU](../compliance/#a-cloned-repository-cannot-send-your-notes-away).

## Models

### manifest

`cyberbrain manifest [--path <PATH>]` writes the `manifest.json` that a model artefact needs, with the blake3 digest of `model.safetensors` and `tokenizer.json`. Default path `<store>/models/model2vec`.

### bench-embedding

`cyberbrain bench-embedding [--sample <N>]` measures the embedding models installed in the local Ollama on a sample of this store's blocks (default 128) and prints the `[embedding]` section for the one that fits. It downloads nothing.

## Agents

### install

```text
cyberbrain install [--client claude-code|claude-desktop|codex] [--project <PATH>] [--name <NAME>] [--undo] [--dry-run]
```

Write this binary into an agent's own configuration: the hooks for Claude Code, the MCP server for a desktop client. `--client` can be repeated; without it, every client found on this machine. `--project` is the project whose `.claude/settings.json` gets the hooks (default: working directory). `--name` names the MCP entry (default `cyberbrain`). Details on the [Claude Code page](../claude-code/#cyberbrain-install).

### hook

`cyberbrain hook <EVENT>` reads the harness payload on stdin and answers on stdout. Events: `session-start`, `session-rings`, `session-context`, `user-prompt-submit`, `pre-tool-use`, `post-tool-use`, `stop`, `pre-compact`. A hook never fails the harness: an internal error is recorded and the process exits 0.

### mcp

`cyberbrain mcp [--client <NAME>] [--http <ADDR:PORT>]`. Over stdio by default. `--client` records the client as `agent:mcp:<name>`. `--http` serves Streamable HTTP at `POST /mcp` on a loopback or private address, with a token per client.

### mcp-client

The clients that may call `mcp --http`: `add <NAME> [--expires <WHEN>]` prints a token once and keeps only its hash, `list`, `remove <NAME>`.

### guard

Ask AgentGuard about one tool call without a memory store (pre-tool-use payload on stdin). For the AgentGuard plugin; address, key and mode come from the environment. Always exits 0.

## The web page

### serve

```text
cyberbrain serve [--port <PORT>] [--no-open] [--terminal] [--allow-resident-writes]
```

Default port 7777. See [Web page and HTTP API](../serve/).

## Compliance

### policy

| Subcommand | |
|---|---|
| `policy egress` | every path by which bytes may leave this machine, and whether it is on |
| `policy obligations` | what the active profile claims about the law, with basis and confidence |
| `policy audit` | the audit log: `--limit`, `--action`, `--subject`, `--since`, `--until`, `--verify`, `--export <PATH>`, `--signed` |
| `policy audit-key [--init]` | the store's Ed25519 audit key and its fingerprint |
| `policy subject <IDENTIFIER>` | everything stored about an identifier (GDPR Art. 15) |
| `policy retention [--apply] [--dry-run]` | notes whose retention has expired; `--apply` erases them through `forget` |
| `policy model-card` | identity, source, licence and hash of every model artefact in use |
| `policy attest [--yes]` | declare the current files in rings 0 and 1 to be the operator's |
| `policy consent [--withdraw]` | record or withdraw consent for the one model download |

### verify-export

`cyberbrain verify-export <PATH> [--expect-key <FINGERPRINT>]` checks an audit bundle or an erasure report. Needs no store, no configuration and no network. Exits non-zero when the chain does not hold.

### canary

Decoys: `plant [--kind api-key|iban|password|token] [--label <TEXT>] [--ring <RING>]`, `list`, `remove <WHICH>`, `push`. See [Running it in the EU](../compliance/#decoys).

## Team

### hub

Run and use a hub: `serve`, `add`, `admin reset`, `service`, `enrol`, `invite`, `push`, `pull`, `licence`, `cert`, `fleet`, `principal`, `erase`, `conflicts`, `grant`, `request`, `approve`, `requests`, `disclose`, `access-log`, `verify`, `backup`, `retention`, `alerts`, `report`, `revoke`. The [Team hub](../hub/) page walks through them.

## Exit codes

`0` when the command did what it was asked. A refusal by policy (a ring that belongs to the operator, a PII hold, a proposal an agent may not accept) exits `3` with the reason on stderr. Checks such as `verify-export`, `hub verify`, `hub backup` and `hub service status` exit non-zero when what they check does not hold, so a scheduled job turns a failure into a failed job.

## Environment

| Variable | |
|---|---|
| `CYBERBRAIN_STORE` | the store, like `--store` |
| `CYBERBRAIN_IDENTITY` | your name in the audit log and for reviews |
| `CYBERBRAIN_AGENT` | marks the caller as an agent with this name (Claude Code sets `CLAUDECODE` instead) |
| `CYBERBRAIN_DISABLED` | the hooks stand down for this session, and session start says so |
| `CYBERBRAIN_WAIVERS` | another location for your `waivers.toml` |
| `CYBERBRAIN_AGENTGUARD_KEY` | the AgentGuard key for `[governance]` |
| `CYBERBRAIN_HUB_TOKEN` | a device token for a service account, instead of the token file |
| `CYBERBRAIN_HUB_PRINCIPAL_TOKEN` | the credential for hub commands that take `--as` |
