# Cyberbrain — Specification v0.1

Cited, trust-tiered, local-first memory for AI coding agents.

Copyright 2026 Krynex Labs. Licensed FSL-1.1-ALv2.

---

## 0. Provenance and clean-room statement

Cyberbrain is an original work. It is **not** derived from, and shares no source code with:

- `openwolf` / `openwolf-enhanced` (AGPL-3.0-only, © Cytostack Pvt Ltd)
- `cfetch` (FSL-1.1-ALv2, © Corbet Labs)

Both were operated in production by the author, and the *operational lessons* from doing so
(section 14) inform this specification. Lessons about a problem domain are not protected
expression; source code is. **No contributor to Cyberbrain may consult the source of either
project.** Behaviour described here is specified from first principles and must be implemented
from this document alone.

Any implementer who has read the source of the above projects must declare it and step back
from the affected module.

---

## 1. What it is

A single native binary that gives an AI coding agent a durable, searchable, auditable memory
of a project, stored as plain Markdown the human can read and edit.

Three properties define it and none may be traded away:

**Cited.** Every retrieved statement carries an identifier that resolves back to the exact
source block. An answer without a citation is a bug, not a degraded result.

**Trust-tiered.** Knowledge lives in numbered rings. Ring 0 is operator invariants that
override everything. Higher rings are progressively weaker. When two blocks contradict, the
lower ring wins and the conflict is reported, never silently resolved.

**Local-first.** The core performs no network I/O. Every byte that can leave the machine
leaves through a declared, enumerable path (section 12).

### Non-goals for v0.1

- No cloud sync, no hosted service, no account system.
- No migration bridges from other memory tools. Separate trees are intentional.
- No agent orchestration. Cyberbrain remembers; it does not act.

---

## 2. Licensing and ownership

- Code: **FSL-1.1-ALv2** (`LICENSE.md`). Converts to Apache-2.0 two years after each release.
- Copyright is held wholly by Krynex Labs so that dual-licensing stays available.
- Every dependency must be permissively licensed (MIT / Apache-2.0 / BSD / ISC / MPL-2.0).
  **No GPL or AGPL dependency, direct or transitive.** CI enforces this with `cargo-deny`;
  a violation fails the build.
- Third-party notices are generated into `THIRD-PARTY-LICENSES.txt` at release time.

---

## 3. Data model

### 3.1 Note

The unit of storage is a Markdown file with YAML frontmatter.

```markdown
---
id: 01JQZ8...            # ULID, immutable, assigned on creation
name: pg18-moves-pgdata  # kebab-case slug, unique within the store
ring: 2                  # 0..4
kind: knowledge | bug | lesson | decision | reference | session
created: 2026-09-05T09:12:03Z
updated: 2026-09-05T09:12:03Z
tags: [postgres, deployment]
links: [docker-bind-mount-inode-drift]   # derived from [[...]], written back on scan
retention: P2Y           # ISO-8601 duration, optional; absent means indefinite
pii: none | reviewed | flagged
---

Body in Markdown. Links to other notes are written [[like-this]].
```

Rules:

- `id` never changes. `name` may change; renames are tracked so citations survive.
- A `[[link]]` to a non-existent note is **valid** and denotes intent. It is reported by
  `cyberbrain doctor` as a dangling link, never auto-created, never an error.
- The file on disk is authoritative. The index is a cache and must be rebuildable from
  the files alone, with no information loss.

### 3.2 Ring semantics

| Ring | Meaning | Override behaviour |
|---|---|---|
| 0 | Operator invariants. Hard rules. | Overrides all higher rings. Always injected. |
| 1 | Operating protocol, active handoff state. | Overrides 2+. Always injected. |
| 2 | Curated project knowledge. | Retrieved on demand. |
| 3 | Session records, observations. | Retrieved on demand, lower weight. |
| 4 | Imported or unverified external material. | Retrieved last, marked as unverified. |

Rings 0 and 1 are size-capped (default 8k tokens combined). Exceeding the cap is an error
surfaced at write time, not a silent truncation at read time.

### 3.3 Block and citation

A note is split into **blocks** at heading and paragraph boundaries, each block ≤ 512 tokens.
A citation is `r{ring}-{first 12 hex of blake3(note_id || block_index || block_text)}`,
e.g. `r2-a91f2c33e1bd`. It is stable across reindexing as long as the block text is
unchanged, and it resolves via `cyberbrain recall --id`.

48 bits, not 40. At 100k blocks the birthday collision probability drops from about 5e-3 to
2e-5 — the difference between a thing that happens to somebody and one that does not. A
collision is caught loudly as a uniqueness violation at index time rather than producing
wrong data, but two extra characters are a cheap way to avoid a baffling scan failure.

---

## 4. Storage layout

```
<store>/                     # default: .cyberbrain/ at project root
  notes/
    r0/ r1/ r2/ r3/ r4/      # one directory per ring, flat inside
  cyberbrain.db              # SQLite: index and vectors. A CACHE. Disposable.
  audit.db                   # SQLite: the audit log. A RECORD. Not disposable.
  cyberbrain.toml            # configuration
  models/                    # model artefacts, content-addressed
```

The `notes/` tree contains only Markdown. Deleting `cyberbrain.db` must be non-destructive:
`cyberbrain scan` rebuilds it completely.

**The audit log lives in its own file, and that is why.** In the first draft it shared
`cyberbrain.db`, which made the sentence above false: everything in that file is rebuildable
from the Markdown except the audit log, which is the one thing that is a record of events
rather than a derived view of state. One file cannot be both disposable and evidence. The
split makes the rule true again and makes each file's nature obvious from its name.

`audit.db` carries `BEFORE UPDATE` and `BEFORE DELETE` triggers that abort, so append-only is
enforced by the database and not by convention.

---

## 5. Index

SQLite via `rusqlite` with the **bundled** feature, so no system SQLite is required on any
platform. FTS5 for lexical search.

Tables (indicative, implementer may refine):

- `notes(id, name, ring, kind, path, created, updated, mtime, size, hash)` — `hash` and the
  stamp are computed by **one function in `cyberbrain-core`**, called by both the scanner and
  the index. Two implementations of the same hash silently drift apart and the incremental
  compare then reports changes that are not there, or worse, misses ones that are.
- `blocks(citation, note_id, idx, text, token_count)`
- `blocks_fts` — FTS5 virtual table over `blocks.text`
- `vectors(citation, dim, data BLOB)` — f32 little-endian
- `links(from_note, to_name, resolved_note_id NULLABLE)`
- `audit(ts, actor, action, subject, detail)` — append-only, see §12
- `meta(key, value)` — schema version, embedding profile id, model hash

**Incremental scan.** `scan` compares file mtime *and* content hash. mtime alone is not
sufficient: bind mounts and archive extraction produce identical mtimes for changed content.
Changed notes have their blocks, vectors and links replaced transactionally.

**Embedding profile.** `meta` records the single opaque `profile_id` (§6.4) that produced the
stored vectors, and nothing derived from it. If the configured model no longer matches, semantic search is
**disabled with a loud message** until reindexed. Silently comparing vectors from two
different models is the worst possible failure: it degrades quality without any error.

---

## 6. Embeddings

Default backend: **static distilled token embeddings (model2vec format), implemented
in-tree.** No transformer inference at query time; the whole operation is a token lookup into
an embedding matrix, mean pooling and L2 normalisation. Model artefact ~30 MB.

Implemented in-tree rather than taken from the `model2vec-rs` crate because that crate depends
unconditionally on `hf-hub`, which downloads models on its own. An unregistered outbound path
inside the binary would make the guarantee in §12.1 false, and a guarantee that is false is
worse than no guarantee. We read `tokenizer.json` via `tokenizers` and the weights via
`safetensors`, and every fetch goes through our own registered egress path.

Rationale: the hook path runs on every agent tool call, so cold start dominates, not
throughput. Static embeddings are two to three orders of magnitude faster to produce than a
transformer forward pass, at a retrieval-quality cost that is small over a Markdown corpus.

Optional backend behind the `candle` feature: **`candle`** running a real sentence embedding
model (BGE / E5 class). Still pure Rust, still cross-compiles, optional CUDA/Metal.

**ONNX Runtime is deliberately excluded from the core.** It is the fastest transformer path on
CPU but introduces a C++ library that breaks the single-static-binary property and complicates
the Windows build. If a user needs it, they use the `candle` feature or an external service.

### 6.1 Artefact identity

A model is two files: `model.safetensors` holding a 2-D `[vocab, dim]` tensor named
`embeddings` (F32, F16, BF16 or I8), and `tokenizer.json`. An **artefact manifest** carries a
blake3 digest of each. Both are hashed before parsing and the bytes that were hashed are the
bytes that get parsed. A mismatch is a hard failure, never a warning.

The tokenizer is hashed alongside the weights because a different vocabulary maps the same
text onto different rows of the same matrix: it changes what a vector means exactly as much
as changed weights do.

### 6.2 Fixed inference settings

These four settings change the resulting vector, so they are part of the format contract and
not configuration: **no special tokens**, **unknown tokens dropped rather than pooled**,
**mean pooling**, **L2 normalisation**. Any deviation is a different embedding profile.

### 6.3 Degenerate input

Empty input, or input whose every token is unknown, yields the **all-zero vector**, never
NaN and never an error. An error would abort a whole batch over one empty block, and a NaN
reaching cosine similarity silently poisons every ranking it touches. The vector carries a
"no semantic signal" marker and the index skips semantic scoring for it.

### 6.4 Profile identity

`profile_id` is a single opaque string folding together weights hash, tokenizer hash,
dimension and pooling. It is the **only** identity the index stores (§5); storing model name
and dimension separately alongside it would create two sources of truth that can disagree.

### 6.5 Load cost on the hot path

Hashing and parsing a real artefact costs tens of milliseconds, which does not fit the hook
budget in §9.1. Two rules follow, and they are requirements, not optimisations:

- **No hook on the hot path loads the embedder.** `pre-tool-use`, `post-tool-use`,
  `user-prompt-submit` and `stop` perform no semantic work at all. Only explicit `recall`
  and `scan` do.
- Verification is **staleness-checked, not repeated blindly**: size and mtime are recorded
  next to the digest, a load re-hashes only when either changed, and the weights are mapped
  rather than copied. The guarantee is unchanged — any actual change to the file triggers a
  full re-hash — while the steady-state cost falls to near zero.

---

## 7. Retrieval

`recall` runs **hybrid search by default**. There is no lexical-only default mode: a retrieval
tool whose best mode is opt-in will be used in its worst mode.

1. Lexical candidates from FTS5 (BM25), top `k_lex` (default 50).
2. Semantic candidates by cosine over stored vectors, top `k_sem` (default 50).
3. Fuse with Reciprocal Rank Fusion, `score = Σ 1/(60 + rank_i)`.
4. Apply ring weighting: multiply by `w[ring]`, default `[1.15, 1.10, 1.00, 0.92, 0.80]`.
   Ranks are **1-based**. The weights sit close to 1 on purpose: fused RRF scores occupy a
   narrow band around `1/61`, so a factor of 2 would not nudge the order, it would sort by
   ring and let relevance break ties. Trust settles a contradiction (§3.2); it does not
   decide what the query was about. The resident rings get the smallest boost of all,
   because they are injected into every session anyway and weighting them here counts them
   twice.
5. Return top `n` (default 8) with citation, ring, note name, and the block text.

**Vector search is a linear SIMD scan.** At the expected corpus size (tens of thousands of
blocks, a few tens of MB of f32) a flat scan is single-digit milliseconds and cannot go stale.
An ANN index (`hnsw_rs`) is introduced only when a real corpus exceeds ~500k blocks, and then
behind a feature flag with the flat scan retained as the correctness reference.

**Contradiction reporting.** If two returned blocks from different rings are detected as
contradictory by the configured local model (§11), the lower ring wins and the result set
carries a `conflict` marker naming both citations. Without a local model configured, this
check is skipped and its absence is stated in the output.

---

## 8. CLI surface

```
cyberbrain init [--store PATH]        create a store, write config, print next steps
cyberbrain scan [--full]              (re)build the index from the notes tree
cyberbrain recall <query> [-n N] [--ring R] [--json]
cyberbrain recall --id <citation>     expand a citation to its full note
cyberbrain find <symbol>              exact line ranges from the code index
cyberbrain write --ring R --kind K --name N [--stdin]
cyberbrain forget <name|id> [--dry-run]   erase note, blocks, vectors, links, and say what went
cyberbrain doctor                     dangling links, ring cap, stale index, orphan vectors
cyberbrain status [--json]            store health, model, backend, compliance profile
cyberbrain export <name|id> [--format md|json]
cyberbrain serve [--port 7777]        local web UI (§13)
cyberbrain hook <event>               agent harness integration (§9)
cyberbrain mcp                        MCP server over stdio (§9)
cyberbrain policy <subcommand>        compliance operations (§12)
```

Exit codes: `0` success, `1` user error, `2` internal error, `3` policy refusal.

Output is human-readable by default and machine-readable with `--json`. Every command that
can change state supports `--dry-run`. **`--dry-run` must run the real code path with a
no-op writer, never a parallel simulation** — a separate dry-run implementation is a second
implementation, and it drifts.

---

## 9. Agent integration

### 9.1 Hooks

`cyberbrain hook <event>` reads the harness payload on stdin and writes its response on
stdout. Events: `session-start`, `user-prompt-submit`, `pre-tool-use`, `post-tool-use`,
`stop`, `pre-compact`.

Hard requirements:

- **Budget: p99 under 15 ms** for every event except `session-start` (budget 150 ms).
  The hook path runs on every tool call; anything slower is a tax on every action the agent
  takes. Enforced by a benchmark in CI, not by hope.
- **A hook must never fail the harness.** Any internal error is caught, logged to the store,
  and the hook exits 0 with empty output.
- **Standing down must be announced.** If Cyberbrain is installed but disabled for a session,
  `session-start` says so. Silent inaction is indistinguishable from a broken hook, and the
  agent has no way to know which protocol applies.
- **Windows: the installed hook command must work when invoked through `cmd.exe`.** Paths with
  spaces, no POSIX shell syntax, no assumption of a login shell. Verified by a Windows CI job
  that actually invokes the installed hook, not one that only builds.

### 9.2 MCP

`cyberbrain mcp` serves the same operations over stdio MCP: `recall`, `recall_id`, `find`,
`write`, `status`. Tool descriptions are generated from one source shared with the CLI help,
so the two cannot drift.

---

## 10. Code index

`cyberbrain find <symbol>` returns exact file and line ranges so the agent reads a slice
instead of a whole file.

- Default: a fast regex/heuristic indexer over common languages, pure Rust.
- Optional `treesitter` feature for precise symbol extraction.
- An ignore file (`.cyberbrainignore`, gitignore syntax) excludes trees from the index.
  This is not cosmetic: a vendored or archived copy of a project will otherwise outrank the
  live source and the agent gets the old version with no indication that it is old.

---

## 11. Local LLM layer

Cyberbrain talks to local inference over an **OpenAI-compatible HTTP API** at a configurable
base URL, default `http://127.0.0.1:11434/v1`.

This single choice covers, with no per-vendor code:

- **Ollama** (native default port 11434)
- **LM Studio**
- **NVIDIA PAIR** (Personal AI Router, Apache-2.0, released 2026-09-03), which presents
  Ollama- and OpenAI-compatible proxy endpoints on the engine's default port and distributes
  requests across RTX 20-series and newer, RTX PRO (Turing+), DGX Spark, and Apple M4+ nodes
  discovered over mDNS with user-approved mTLS pairing.
- Any other OpenAI-compatible local server (vLLM, llama.cpp server, llamafile).

The layer is **optional**. Every core function works without it. What it enables:

- summarising a session into a note,
- detecting contradictions between retrieved blocks (§7),
- proposing a ring and tags for a new note,
- rewriting a note when new evidence supersedes part of it.

Requirements:

- The base URL must resolve to a loopback or private-range address unless the operator
  explicitly sets `allow_public_endpoint = true`. A memory tool that quietly posts project
  notes to a public API is the single worst bug this project can ship, and it has precedent.
- Model, endpoint and token counts of every call are recorded in the audit log. Many local
  servers omit `usage`; the log then records that it was absent and **never estimates one**.
  An invented number in an audit log is worse than a gap, because a gap is visibly a gap.
- **No hook calls this layer synchronously.** Completion timeouts are measured in tens of
  seconds and the hot-path hook budget in §9.1 is 15 ms; the two cannot meet. This follows
  the same rule as §6.5 and for the same reason.
- The model may never propose ring 0 or ring 1 for a note. Those rings are operator
  invariants and protocol; a suggestion engine that can write into them defeats the purpose
  of having a trust hierarchy at all. A suggestion of 0 or 1 is demoted to 2 and marked.
- Contradiction detection is defined across rings (§3.2). Two contradictory blocks **in the
  same ring** have no precedence rule, so no winner is invented: both are reported as an
  unresolved disagreement for a human to settle.
- A refused or unreachable endpoint degrades the feature and says so. It never blocks a core
  operation and never falls back to a remote provider.

---

## 12. Compliance layer

This is a first-class subsystem, not documentation. Configured by profile:

```toml
[policy]
profile = "eu"    # eu | ch | off
```

`off` compiles the checks in but disables them at zero runtime cost on the hot path.
`ch` exists separately rather than as "EU minus" because the revised Swiss FADP differs in
substance — erasure, breach notification timing and the register of processing are not the
same obligations, and folding them together produces claims that are wrong in one country.

### 12.1 Egress register

Every code path capable of sending bytes off the machine is registered at compile time in one
module. `cyberbrain policy egress` prints the complete list: purpose, destination, what data,
which profile permits it, and whether it is currently enabled.

A network call from a path not in the register must be structurally impossible. The wrapper
cannot live in this crate, because the crates that actually make requests (`llm`, and model
download) must not depend on the policy crate — so the **seam is the `EgressGate` trait in
`cyberbrain-core`**, the policy crate implements it, and the binary wires it in. Any code
path wanting a request holds a gate and calls `permit(purpose, destination)` first.

`permit` decides **and** records in one call. Two separate calls would permit a request that
was never audited, which is precisely the failure the register exists to prevent.
`destination` is what will really be contacted, after name resolution where that applies, not
what the configuration string claimed.

`DenyAllEgress` is the default for any path not yet wired, so a forgotten wiring surfaces as
a refusal in the log rather than as a silent unaudited request.

CI whitelists HTTP-client construction only at call sites that take an `EgressGate`.

### 12.1.1 What the transport itself may do behind your back

Naming the destination is not enough if the HTTP stack rewrites it. Both of these are
disabled and covered by tests, and both would defeat the register while the config string
still innocently read `127.0.0.1`:

- **Proxy environment variables.** `HTTP_PROXY` / `HTTPS_PROXY` are read by default by most
  clients, and a proxy sends every byte to a host nobody validated.
- **Redirects.** A local endpoint answering `307` to a public URL exfiltrates on the second
  hop.

Additionally, the validated addresses are **pinned** for the connection, so a name that
resolves differently between validation and connection cannot be used to slip past the check.

Registered purposes for v0.1: `ModelDownload` (once, on consent), `LocalInference` (loopback
or private range only). That is the entire list. Telemetry does not exist.

**"Private range" means** loopback, RFC1918, RFC4193 unique-local, and link-local. It
deliberately does **not** include `100.64.0.0/10`: that range is carrier-grade NAT, where an
address is usually somebody else's machine, but it is also what overlay networks such as
Tailscale hand out — which is a completely reasonable way to reach your own GPU box. Those
two cases are indistinguishable from the address alone, so they get their own switch,
`allow_overlay_network`, rather than being folded into `allow_public_endpoint`. A setting
whose name misdescribes what the operator is agreeing to is a consent failure, not a
convenience.

**HTTPS to a local endpoint is off by default.** No root certificate store is compiled in, so
`https://` fails closed unless the operator supplies a CA. Local inference is plain HTTP over
loopback or a private network, and shipping a root store to serve an unusual case would put
a trust anchor in the binary for no benefit.

### 12.2 Erasure (GDPR Art. 17)

`cyberbrain forget` removes the note file, its blocks, its vectors, its FTS entries, its
outbound link rows, and any cached derivative, in one transaction, and prints what it removed
per store.

**Inbound links are unresolved, not deleted.** Other notes still contain `[[name]]` on disk,
so deleting those rows would put the index at odds with the files and the next scan would
recreate them. They become dangling links, which `doctor` reports and the UI renders as
intent (§13).

**Audit rows are exempt from erasure.** An erasure record naming what was erased is itself a
trace, and it is kept deliberately: evidence that the erasure happened is what makes the
erasure demonstrable. This is stated here so that nobody later "fixes" it. Deleting a Markdown file while its vector stays in the index is the
obvious silent failure of this design and must be covered by a test that asserts the vector
is gone, not merely that the file is.

### 12.3 Access (GDPR Art. 15)

`cyberbrain policy subject <identifier>` searches every note, block and audit row for an
identifier (name, email, handle) and outputs everything found with citations, in a form that
can be handed to a data subject.

### 12.4 PII at write time

Before a note is written, its body is scanned for email addresses, IP addresses, API keys,
IBANs and phone numbers. Under `eu` and `ch` the write is held and the operator is shown what
was found, with the options to redact, to mark reviewed, or to proceed. Under `off` the scan
does not run. Detection is heuristic and is documented as such; it is a seatbelt, not a
guarantee.

### 12.5 Retention

A note may carry a `retention` duration. `cyberbrain policy retention` lists what is due and
`--apply` erases it through the same path as `forget`. Expiry never happens silently in the
background: an agent's memory disappearing without a record is indistinguishable from a bug.

### 12.6 Audit log

Append-only table recording writes, erasures, model downloads, inference calls, policy
refusals and egress attempts, with timestamp, actor and subject. `cyberbrain policy audit`
exports it. This is the evidence an AI Act conformity discussion asks for, and it is the
integration seam for AgentGuard.

### 12.7 Model transparency (EU AI Act)

`cyberbrain policy model-card` prints the identity, source, licence, hash, dimension and
intended use of every model artefact in use. Cyberbrain itself is a minimal-risk system, but
it ships model weights, and a deployer inside a regulated workflow needs this on paper.

---

## 13. Frontend

`cyberbrain serve` starts a local HTTP server (default `127.0.0.1:7777`) and serves a single
page application **embedded in the binary** via `rust-embed`. No separate install, no node
runtime on the user's machine, no external asset fetch at runtime — which is also what keeps
the egress register short.

Stack: **React + Vite + Tailwind v4**, TypeScript. Chosen for skill-match with the operator's
existing Next.js work rather than novelty; the bundle is static and framework-agnostic at the
serving layer.

Screens for v0.1:

1. **Search** — hybrid recall with the ring, citation and score visible per hit. The citation
   is copyable in one click, because that is what gets pasted back to an agent.
2. **Note** — rendered Markdown, frontmatter as structured fields, inbound and outbound links,
   inline editing that writes the file on disk.
3. **Graph** — notes and links, coloured by ring, dangling links shown as such rather than
   hidden.
4. **Compliance** — the egress register with live enable state, the audit log, PII findings,
   retention queue, and the model card. This screen is the product's differentiator and gets
   design attention accordingly.
5. **Status** — store size, index freshness, embedding profile, configured inference endpoint
   and which backend answered last.

Dark by default, light available, honouring `prefers-color-scheme`. Keyboard-first: search
focus, result navigation and citation copy must all work without a mouse.

The UI is a client of the same API the CLI uses. No logic lives only in the frontend.

---

## 14. Requirements carried over from operating a predecessor

These are lessons from running a prior memory tool in production. They are stated as
requirements on Cyberbrain, derived from observed failures, and contain no third-party design.

1. **A dependency pin is not in force until the lockfile shows it.** CI asserts the resolved
   lockfile, not the manifest.
2. **A check has not passed until it has been run against the broken state.** Every regression
   test is first demonstrated to fail on the defect it covers.
3. **A counter must count the exit, not the entry.** "52 orders processed" that were 52
   rejections is worse than no counter. Every metric names the side of the boundary it counts.
4. **The denominator comes before the numerator.** A ratio over an unreachable population is
   noise. Ratios state their population.
5. **A silent early return is a bug.** Every branch that declines to act says why.
6. **`--dry-run` runs the real path.** No second implementation.
7. **A lock that keeps going under contention is not a lock.** Fail closed.
8. **Platform-dependent normalisation is a hazard.** Case folding written for Windows deletes
   the wrong directory on Linux. Path handling is tested on both.
9. **A test must take the user's route.** Checking a container IP proves nothing about a
   request that reaches the proxy first.
10. **One outbound path is one too many if it is not declared.** §12.1 exists because a
    predecessor shipped a path that sent private notes off the machine.
11. **Renaming does not relicense.** §0 exists because the obvious shortcut here was illegal.

---

## 15. Platform and build

- Rust stable, edition 2024, MSRV pinned to the release toolchain (currently 1.98.1).
- Targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
  `x86_64-pc-windows-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`.
- **The shipped binary has no runtime dependencies.** No system SQLite, no OpenSSL, no model
  server, no node runtime. This is the property that matters to a user and it is verified in
  CI by running the release binary on a bare image.
- Build-time C is permitted but stays enumerated and justified. Current footprint: **`cc`,
  for bundled SQLite, and nothing else.** `tokenizers` declares `esaxx-rs` with
  `default-features = false`, so its `cpp` feature is off and that dependency is pure Rust;
  a C++ archive appearing in a target directory means a stale artefact from an earlier
  feature resolution, not a live dependency. `cmake`, `aws-lc-sys`, `openssl-sys` and
  `native-tls` are banned outright in `deny.toml`. Any addition to this list is a decision,
  not an accident: CI fails on an unlisted `cc`/`cmake` build script.
- Every dependency that performs network I/O of its own is disqualified, because it defeats
  the egress register (§12.1). `hf-hub` was removed for exactly this reason, which is why
  static embedding inference is implemented in-tree (§6) rather than taken from a crate.
- Release artefacts: one static binary per target, plus an npm wrapper package that downloads
  the matching binary, because that is how agent-harness users install things.
- CI: build and test on Linux, Windows and macOS. The Windows job must exercise the installed
  hook through `cmd.exe`, not merely compile.

### Performance budgets (enforced by benchmarks in CI)

| Operation | Budget |
|---|---|
| `hook pre-tool-use` (p99) | 15 ms |
| `hook session-start` (p99) | 150 ms |
| `recall` over 25k blocks | 50 ms |
| `scan` incremental, no changes | 100 ms |
| cold binary start to first output | 20 ms |

---

## 16. Scope of v0.1

**In:** store, scan, index, hybrid recall with citations, `find`, `write`, `forget`, `doctor`,
`status`, the six hooks, MCP server, compliance layer per §12, local LLM layer per §11,
web UI per §13, Linux and Windows builds.

**Out (deferred, in this order):** ANN index, tree-sitter code index, macOS release job,
contradiction detection beyond a single model call, graph importance ranking, npm wrapper.

Done means: the operator runs Cyberbrain as the only memory system in a real project for a
week, and `cyberbrain doctor` is clean at the end of it.
