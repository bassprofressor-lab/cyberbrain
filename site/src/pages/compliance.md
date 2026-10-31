---
layout: ../layouts/Doc.astro
title: "Running it in the EU"
description: "The egress register, waivers a cloned repository cannot grant itself, the PII check, erasure with a signed report, subject access, the hash-chained and signed audit log, audit exports, retention, decoys and the obligation catalogue."
slug: "compliance/"
---

# Running it in the EU

Compliance is a subsystem, not a chapter of the documentation. Every claim on this page is something a command prints or checks. What none of it does is decide whether your use is lawful: that stays your determination, and the tool says so where it matters.

`[policy] profile` is `eu` (GDPR, the default), `ch` (the revised Swiss FADP) or `off`. Switzerland is its own profile rather than "EU minus", because the two laws differ in substance. `off` compiles the checks in and disables them.

## The egress register

`cyberbrain policy egress` prints every path by which bytes may leave the machine, what each carries, and whether it is on. In 0.8.4 the list has nine entries. In a fresh store:

| Path | Carries note content | State in a fresh store |
|---|---|---|
| `ModelDownload` | no | off: no `model_source` configured |
| `LocalInference` | yes | allowed for loopback or private addresses; never contacted without `[inference] model` |
| `AuditSync` | no | off: not enrolled with a hub |
| `NoteSync` | yes | off: not enrolled, and needs `allow_note_sync` |
| `NoteErasure` | no | off: not enrolled |
| `HubEnrolment` | no | on request, once, when a fleet invitation is enrolled |
| `Terminal` | yes | not mediated: exists only while `serve --terminal` runs |
| `Governance` | yes | off: no `[governance] url` |
| `HubAlert` | no | off: no webhook set |

The list ends: "That is the entire list. Telemetry does not exist." TLS trust comes from the operating system's certificate store; no bundle is compiled in. The gate is asked per request, not once per channel. Calls through it are recorded in the audit log (a hub's alerts in the hub's own log), and so is every refusal.

## A cloned repository cannot send your notes away

A store lives in a repository, so its `cyberbrain.toml` is written by whoever wrote the repository. Three keys would let notes go to another machine: `inference.allow_public_endpoint`, `inference.allow_overlay_network` and `hub.allow_public_hub`. Since 0.8.2 each counts only if the store asks for it **and** you granted it for that store in your own `waivers.toml` (`~/.config/cyberbrain/waivers.toml`, `%APPDATA%\cyberbrain\waivers.toml` on Windows, or wherever `CYBERBRAIN_WAIVERS` points).

```console
$ cyberbrain trust
store: .cyberbrain
waivers file: ~/.config/cyberbrain/waivers.toml
this store asks for no waivers
```

`cyberbrain trust --grant` grants exactly what the store asks for now, `--revoke` takes it back. Both are audit rows. A key the store sets without your grant is switched off, and `doctor` says which. The web page shows the state but does not grant or revoke.

## Personal data at write time

Every `write`, `propose`, `transcribe` and `import` runs a PII check before anything reaches the disk. It finds the shapes of emails, IP addresses, API keys, IBANs, phone numbers and passwords in URLs. A finding holds the write for your decision: redact, mark reviewed, or proceed flagged (`--force`). It never redacts behind your back. The output says what it is: "a seatbelt, not a guarantee". It misses what has no shape.

Over MCP, an agent may redact or proceed flagged, but not mark its own finding as reviewed; that choice is the operator's.

## Erasure (Art. 17)

```console
$ cyberbrain forget pg18-moves-pgdata --report erase.json --sign
wrote the erasure report to erase.json, signed
 pg18-moves-pgdata (01M68S19BPN9WZ1YV16R34E0C0):
  file: removed
  blocks: 1
  fts rows: 1
  vectors: 0
  outbound links: 0
  inbound links now dangling: 0
  cached derivatives: 0
  wal: emptied, nothing of the note left in the database files
  audit rows: kept (SPEC §12.2: the erasure record is the evidence)
```

One transaction removes the note, its blocks, search rows, vectors, links and derivatives. The audit rows stay: they are the record that the erasure happened, and they never held the note's text. `--dry-run` shows the same list first. The report can be checked by somebody else with `verify-export`. A note shared through a hub has a copy there; `forget` says so and names `hub erase`.

## Subject access (Art. 15)

```console
$ cyberbrain policy subject anna
# Subject access report

Identifier: `anna`
Profile: eu (GDPR, Regulation (EU) 2016/679)
Response deadline: within one month (extendable by two months)

## Notes (0 block(s))
…
## Audit log (1 row(s))
…
## Caveats

- Matching is a case-insensitive substring search over note blocks and audit rows. It finds the identifier as written; it does not find a person described without it.
```

## The audit log

`audit.db` is append-only, enforced by database triggers, and each row is chained to the previous one with blake3. `cyberbrain policy audit --verify` names the first altered row and exits non-zero. Rows record the actor (`operator`, `agent:claude-code:<session>`, `hook:pre-tool-use`, `mcp`, …), the action, the subject and details; they do not contain note text.

A hash chain shows that a row changed, but anybody who can write `audit.db` could recompute every hash. A signing key closes that:

```console
$ cyberbrain policy audit-key --init
created audit key: 3ec4b6f7aa1148bc52e253e528263b67
public key: 09a7715f18eb4092c5e7536ec9a298dadf11dd3e4dc53bd7dbcf01b0d324c9cc
file: .cyberbrain/keys/audit.ed25519
```

From then on every new row is signed with Ed25519. Restart `serve`, `mcp` and the daemon so they sign too. Since 0.8.3, a store with a key whose rows carry no signature is no longer reported intact: `doctor` and `policy audit --verify` say so.

### Handing a period to somebody else

```console
$ cyberbrain policy audit --export q1.jsonl --signed
wrote 12 row(s) to q1.jsonl
checked as written: chain holds from anchor genesis
signed with key 3ec4b6f7aa1148bc52e253e528263b67
$ cyberbrain verify-export q1.jsonl --expect-key 3ec4b6f7aa1148bc52e253e528263b67
chain holds over 12 row(s)
period:   the whole log
anchor:   genesis
…
signed:   by key 3ec4b6f7aa1148bc52e253e528263b67, and the signature holds
rows:     1 signed by key 3ec4b6f7aa1148bc52e253e528263b67
rows:     11 unsigned (written before the store had an audit key; the chain covers them)

This says the file is internally intact and starts where it says it does.
Whether the anchor belongs to that machine's real history is a question only the
full log answers.
```

`--since` and `--until` (RFC 3339) cut out a period; its first row then points back at an anchor, and a footer carries the row count and last hash, so rows removed from either end show. The recipient needs only the binary: no store, no configuration, no network. Give them the fingerprint on a path the store's owner cannot change later, such as a contract or a ticket; without `--expect-key` a signature only shows that somebody signed. An export without `--limit` covers the whole period, because a silently truncated period is the one mistake an auditor cannot see. The file format, and how to check it without this program, is in [`docs/AUDIT-EXPORT.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/AUDIT-EXPORT.md).

## Retention

A note can carry a retention period (`--retention P2Y`). `cyberbrain policy retention` lists what has expired; `--apply` erases it through the same path as `forget`. Nothing is removed silently in the background.

## Decoys

```console
$ cyberbrain canary plant --kind api-key --label "deploy key bait"
```

A decoy is a credential nobody uses (an API key, an IBAN with a valid check sum and no bank behind it, a password, a token), filed in a note that reads like the real thing and is tagged `canary`. A use of it anywhere but its own note is refused: in any tool call the pre-tool-use hook sees and in any note written or proposed. Each use is recorded as `canary.hit`, and a hub that receives the row raises an alert. The value itself lives in `keys/canaries.json`, owner-only, and never appears in the log, a refusal or an alert; those carry its fingerprint, 12 hex digits of its SHA-256. `canary list` shows the decoys and their hits, `canary push` hands them to AgentGuard.

## What the profile claims

```console
$ cyberbrain policy obligations
Profile eu: GDPR, Regulation (EU) 2016/679
11 obligation(s) this profile encodes.

Scope  [high]
  Applies to personal data of natural persons. A memory store is in scope as soon as a note names, describes or identifies a person.
  basis: GDPR Art. 2, Art. 4(1)
  note: Whether a given store contains personal data at all is a fact about its content, not something this tool can assert.
…
```

Each line carries its article and a confidence. Nothing below high confidence drives behaviour.

## Models

`cyberbrain policy model-card` names identity, source, licence and hash of every model artefact in use. A model download is possible only towards the exact host in `[embedding] model_source`, only after `cyberbrain policy consent`, and the artefact's hash is checked afterwards. With no source configured, which is the default, nothing can be downloaded at all.
