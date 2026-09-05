/**
 * Hand-written seed notes for the mock. The content is about running Cyberbrain itself,
 * which is what the operator's first real store will look like. Nothing here is copied
 * from any other tool's notes.
 */
import type { NoteKind, Ring } from "../types";

export interface SeedNote {
  name: string;
  ring: Ring;
  kind: NoteKind;
  tags: string[];
  /** Days ago. */
  created: number;
  updated: number;
  retention?: string;
  pii?: "none" | "reviewed" | "flagged";
  body: string;
}

export const SEED_NOTES: SeedNote[] = [
  // ───────────────────────────────────────────── ring 0: operator invariants
  {
    name: "no-network-in-core",
    ring: 0,
    kind: "decision",
    tags: ["egress", "architecture"],
    created: 61,
    updated: 61,
    body: `# No network I/O in the core

The core crates perform **no network I/O**. Every byte that can leave the machine goes
through one wrapper in \`cyberbrain-policy\` that takes a registered \`EgressPurpose\`.

- A dependency that performs its own network I/O is disqualified, whatever it offers.
  This is why \`hf-hub\` was removed and static embedding inference lives in-tree, see
  [[model2vec-in-tree]].
- CI greps for direct HTTP client construction outside the egress module and fails the
  build on a hit. A test that only checks the register is not enough; the register has to
  be the only way.

Overrides anything in a higher ring that suggests "just fetch it".`,
  },
  {
    name: "never-copy-config-between-deployments",
    ring: 0,
    kind: "lesson",
    tags: ["deployment", "config"],
    created: 58,
    updated: 58,
    body: `# Never copy a config file between deployments

Two deployments that share source do **not** share configuration. Copying \`config\` from
one to the other has broken live operation before, because the receiving side had its own
values that were silently overwritten.

Rule: configuration moves by diff and by hand, never by \`cp\` or \`rsync\`. See
[[deployment-parity-checklist]] for the checklist that replaced the copy.`,
  },
  {
    name: "lower-ring-wins",
    ring: 0,
    kind: "decision",
    tags: ["rings", "retrieval"],
    created: 61,
    updated: 40,
    body: `# When blocks contradict, the lower ring wins

Ring 0 overrides everything. Ring 1 overrides 2 and up. A contradiction between rings is
**reported with both citations**, never resolved silently, and the higher-ring block is
still returned so the reader can see what lost.

Without a local model configured the contradiction check does not run, and the result
says so in its caveats. A skipped check that says nothing is indistinguishable from one
that passed.

Related: [[ring-cap-is-a-write-error]], [[contradiction-check-design]].`,
  },
  {
    name: "ring-cap-is-a-write-error",
    ring: 0,
    kind: "decision",
    tags: ["rings", "budget"],
    created: 60,
    updated: 60,
    body: `# The resident ring cap is enforced at write time

Rings 0 and 1 are injected into every session, so together they carry a hard budget
(default 8k tokens). Exceeding it is an **error at write time**, surfaced to whoever wrote
the note. It is never a silent truncation at read time: an agent that gets a truncated
protocol does not know which half it is missing.`,
  },

  // ───────────────────────────────────────────── ring 1: protocol / handoff
  {
    name: "handoff",
    ring: 1,
    kind: "session",
    tags: ["handoff"],
    created: 61,
    updated: 0,
    body: `# Handoff — current state

**Now:** frontend for \`cyberbrain serve\` is being built against a mock API. The Rust
side implements the contract in \`ui/src/api/types.ts\` next.

**Open:**
- \`cyberbrain serve\` route table (see [[api-contract-v1]])
- Windows hook job under \`cmd.exe\` still red on paths with spaces, see [[bug-012-cmd-exe-quoting]]
- Decide whether \`find\` ships with the regex indexer only ([[code-index-regex-first]])

**Do not:** touch \`config.py\` on the second server ([[never-copy-config-between-deployments]]).

Last session: [[session-2026-09-04]].`,
  },
  {
    name: "operating-protocol",
    ring: 1,
    kind: "reference",
    tags: ["protocol", "agents"],
    created: 61,
    updated: 12,
    body: `# Operating protocol for agents in this store

1. Before searching files, \`recall\` the question. Cite what you use.
2. Write findings back with a ring. Unsure → ring 3, never ring 0.
3. A bug goes to a \`bug\` note with a number. A repeated class of bug goes to
   [[do-not-repeat]].
4. When a session ends, update [[handoff]]. If nothing changed, say so there.
5. Standing down is announced. Silence is a broken hook, not a quiet one.`,
  },
  {
    name: "do-not-repeat",
    ring: 1,
    kind: "lesson",
    tags: ["lessons"],
    created: 55,
    updated: 3,
    body: `# Do not repeat

Recurring failure classes, each with the note that documents the first occurrence.

- **mtime is not a change detector.** Bind mounts and archive extraction preserve it.
  Compare content hash too. [[docker-bind-mount-inode-drift]]
- **A version number is not a security state.** Ask the advisory database, not the
  changelog. [[version-is-not-patch-level]]
- **A counter must count the exit.** [[counters-count-the-exit]]
- **Case-folding paths written for Windows deletes the wrong thing on Linux.**
  [[bug-009-path-case-folding]]
- **A silent early return is a bug.** Every declined branch says why.`,
  },

  // ───────────────────────────────────────────── ring 2: curated knowledge
  {
    name: "pg18-moves-pgdata",
    ring: 2,
    kind: "bug",
    tags: ["postgres", "deployment", "docker"],
    created: 2,
    updated: 2,
    retention: "P2Y",
    body: `# PostgreSQL 18 moves PGDATA

The official \`postgres:18\` image puts the data directory at
\`/var/lib/postgresql/18/docker\` instead of \`/var/lib/postgresql/data\`.

## Symptom

A bind mount that worked for 16 and 17 still mounts, the service reports healthy, and
every write lands **inside the container layer**. Nothing is lost until the container is
recreated, at which point everything is.

## Fix

Mount \`/var/lib/postgresql\` (the parent) or set \`PGDATA\` explicitly. Verify with
\`docker exec … psql -c 'show data_directory'\` and compare against the mount table.

Related failure class: [[docker-bind-mount-inode-drift]]. Checklist entry added to
[[deployment-parity-checklist]].`,
  },
  {
    name: "docker-bind-mount-inode-drift",
    ring: 2,
    kind: "lesson",
    tags: ["docker", "filesystem"],
    created: 44,
    updated: 30,
    body: `# Bind mounts and inode drift

A bind-mounted file that is replaced atomically (write temp, rename) gets a **new inode**.
The container keeps the old one open. From inside, the file never changes; from outside,
the edit is there. \`mtime\` is identical in both views.

This is why \`scan\` compares mtime **and** content hash: the identical-mtime case is not
theoretical, it is the default behaviour of every editor that saves safely.

Mount the directory, not the file.`,
  },
  {
    name: "model2vec-in-tree",
    ring: 2,
    kind: "decision",
    tags: ["embeddings", "dependencies"],
    created: 50,
    updated: 21,
    body: `# Static embeddings implemented in-tree

The default embedder is a static distilled token embedding (model2vec format): a token
lookup into a matrix, mean pooling, L2 normalisation. No transformer at query time.

## Why not the crate

The \`model2vec-rs\` crate depends unconditionally on \`hf-hub\`, which downloads models on
its own. An unregistered outbound path inside the binary makes the egress register false,
and a guarantee that is false is worse than none ([[no-network-in-core]]).

## What we read

- \`tokenizer.json\` via \`tokenizers\`
- weights via \`safetensors\`

Artefacts are content-addressed. Hash mismatch on load is a hard failure. Profile id is
\`model2vec/potion-base-8M@d256/mean-l2\` and is stored next to the vectors so a changed
model is detected ([[embedding-profile-mismatch]]).`,
  },
  {
    name: "embedding-profile-mismatch",
    ring: 2,
    kind: "knowledge",
    tags: ["embeddings", "index"],
    created: 48,
    updated: 48,
    body: `# Embedding profile mismatch disables semantic search

\`meta\` records model identity, dimension and pooling. If the configured model differs,
semantic search is **disabled with a loud message** until \`scan --full\` reindexes.

Comparing vectors from two models is the worst failure available: results get worse and
nothing errors. The lexical path keeps working, and \`recall\` reports \`mode: lexical\`
so the caller can tell.`,
  },
  {
    name: "hybrid-recall-rrf",
    ring: 2,
    kind: "knowledge",
    tags: ["retrieval"],
    created: 47,
    updated: 33,
    body: `# Hybrid recall with reciprocal rank fusion

1. Lexical candidates from FTS5 (BM25), top 50.
2. Semantic candidates by cosine over stored vectors, top 50, linear SIMD scan.
3. Fuse: \`score = Σ 1/(60 + rank_i)\`.
4. Multiply by ring weight \`[2.0, 1.6, 1.0, 0.8, 0.5]\`.
5. Return top n (default 8) with citation, ring, note name, block text.

There is no lexical-only default. A retrieval tool whose best mode is opt-in gets used in
its worst mode.

The flat vector scan is single-digit milliseconds at tens of thousands of blocks and
cannot go stale. An ANN index is a feature flag for a corpus we do not have.`,
  },
  {
    name: "contradiction-check-design",
    ring: 2,
    kind: "decision",
    tags: ["retrieval", "llm"],
    created: 40,
    updated: 40,
    body: `# Contradiction check

After fusion, pairs of returned blocks from **different rings** are handed to the local
model with a single yes/no prompt. On "yes" the lower ring wins and the result carries a
\`conflict\` naming both citations.

Costs one call per result set, only when a model is configured. Never blocks the result:
a timeout is a caveat, not an error.`,
  },
  {
    name: "sqlite-bundled-fts5",
    ring: 2,
    kind: "knowledge",
    tags: ["sqlite", "index", "build"],
    created: 52,
    updated: 52,
    body: `# SQLite: bundled, with FTS5

\`rusqlite\` with the \`bundled\` feature compiles SQLite into the binary, so no platform
needs a system SQLite and the FTS5 module is always present. It costs one \`cc\` build
dependency, which is enumerated in the build notes.

Deleting \`cyberbrain.db\` is non-destructive by design: \`scan\` rebuilds it from the
notes tree with no information loss. That property is tested, not assumed.`,
  },
  {
    name: "hook-budget-15ms",
    ring: 2,
    kind: "decision",
    tags: ["hooks", "performance"],
    created: 45,
    updated: 45,
    body: `# Hook budget: p99 under 15 ms

The hook path runs on every agent tool call. Anything slower is a tax on every action the
agent takes, so the budget is enforced by a benchmark in CI, not by hope.

Consequences:
- static embeddings, not a transformer ([[model2vec-in-tree]])
- SQLite opened read-only with a prepared statement cache
- no model call on the hook path, ever

\`session-start\` gets 150 ms because it runs once.`,
  },
  {
    name: "code-index-regex-first",
    ring: 2,
    kind: "decision",
    tags: ["code-index"],
    created: 38,
    updated: 38,
    body: `# Code index: regex first, tree-sitter optional

\`find <symbol>\` returns exact line ranges so the agent reads a slice, not a file. v0.1
ships a heuristic regex indexer over common languages. Tree-sitter is a feature.

The ignore file matters more than the parser: a vendored or archived copy of the project
outranks the live source otherwise, and the agent gets the old version with no indication
that it is old. See [[bug-004-archive-outranks-source]].`,
  },
  {
    name: "deployment-parity-checklist",
    ring: 2,
    kind: "reference",
    tags: ["deployment"],
    created: 57,
    updated: 2,
    body: `# Deployment parity checklist

Run before and after every deploy to a second machine.

| Check | How |
|---|---|
| Lockfile matches manifest | CI asserts the resolved lockfile |
| Config differs by intent | diff, never copy ([[never-copy-config-between-deployments]]) |
| Data directory is where the mount is | \`show data_directory\` ([[pg18-moves-pgdata]]) |
| Service answers on the user's route | hit the domain, not the container IP |
| Version pins are in force | check the resolved tree, not the manifest |`,
  },
  {
    name: "counters-count-the-exit",
    ring: 2,
    kind: "lesson",
    tags: ["metrics", "lessons"],
    created: 35,
    updated: 35,
    body: `# A counter counts the exit, not the entry

"52 orders processed" that were 52 rejections is worse than no counter. Every metric in
this project names the side of the boundary it counts: \`hits_returned\`, not \`hits\`;
\`bytes_out\`, not \`bytes\`.

The denominator comes first. A ratio over an unreachable population is noise.`,
  },
  {
    name: "version-is-not-patch-level",
    ring: 2,
    kind: "lesson",
    tags: ["security", "dependencies"],
    created: 33,
    updated: 33,
    body: `# A version number is not a security state

Distribution packages backport fixes without bumping the version; upstream branches fix
the same CVE in different point releases. The version string alone says nothing.

Ask the advisory database for the resolved version. \`cargo-deny\` does this for Rust;
for everything else, look it up.`,
  },
  {
    name: "api-contract-v1",
    ring: 2,
    kind: "reference",
    tags: ["api", "frontend"],
    created: 1,
    updated: 0,
    body: `# HTTP API v1 for \`cyberbrain serve\`

Defined by the frontend in \`ui/src/api/types.ts\`; the server is built to match.

- \`GET /api/v1/status\`, \`/recall\`, \`/recall/{citation}\`
- \`GET|PUT|DELETE /api/v1/notes/{name|id}\`, \`POST /api/v1/holds/{id}\`
- \`GET /api/v1/graph\`
- \`GET /api/v1/policy/{egress,audit,pii,retention,model-card,subject}\`
- \`GET /api/v1/doctor\`, \`POST /api/v1/scan\`

Errors carry \`{ error: { code, message, exit_code } }\` with the CLI exit code.
The UI is a client of the same operations the CLI exposes; no logic lives only in the
frontend. Owner: [[handoff]].`,
  },
  {
    name: "windows-hook-cmd-exe",
    ring: 2,
    kind: "knowledge",
    tags: ["windows", "hooks", "ci"],
    created: 20,
    updated: 6,
    body: `# Windows hooks run through cmd.exe

The harness invokes the installed hook command through \`cmd.exe\`, not a POSIX shell.
Paths with spaces need quoting that \`cmd.exe\` accepts, there is no \`$VAR\`, and no login
shell has run.

CI must **invoke** the installed hook on Windows, not merely build it. The build-only job
was green for three weeks while the hook failed on every machine with a space in the user
name. Tracked in [[bug-012-cmd-exe-quoting]].`,
  },

  // ───────────────────────────────────────────── ring 2 bugs
  {
    name: "bug-004-archive-outranks-source",
    ring: 2,
    kind: "bug",
    tags: ["code-index", "bug"],
    created: 37,
    updated: 36,
    body: `# bug-004: archived copy outranks the live source

**Seen:** \`find dispatcher\` returned a five-month-old copy under \`backup/\` first. The
agent edited the dead tree, reported success, nothing changed in production.

**Cause:** no ignore file; the archive had more matches because it was larger.

**Fix:** \`.cyberbrainignore\` with gitignore syntax, and the index reports when a match
lies under an ignored path that was indexed before the ignore existed.`,
  },
  {
    name: "bug-009-path-case-folding",
    ring: 2,
    kind: "bug",
    tags: ["paths", "bug", "windows"],
    created: 28,
    updated: 27,
    body: `# bug-009: case folding deletes the wrong directory on Linux

A path normaliser written for Windows lower-cased every path. On Linux \`/srv/App\` and
\`/srv/app\` are different directories and \`forget\` targeted the wrong one in dry-run.

**Fix:** normalisation is platform-specific and tested on both. Dry-run showed it before
it ran for real, which is the entire point of dry-run running the real path.`,
  },
  {
    name: "bug-012-cmd-exe-quoting",
    ring: 2,
    kind: "bug",
    tags: ["windows", "hooks", "bug"],
    created: 8,
    updated: 1,
    body: `# bug-012: hook command breaks under cmd.exe with spaces in the path

**Status:** open. The installed command is
\`"C:\\Users\\Jane Doe\\.cyberbrain\\bin\\cyberbrain.exe" hook pre-tool-use\` and \`cmd.exe\`
strips the outer quotes when the whole line is quoted again by the harness.

**Next:** wrap as \`cmd /d /s /c ""…""\` or ship a \`.cmd\` shim. The Windows CI job must
invoke the installed hook, see [[windows-hook-cmd-exe]].`,
  },

  // ───────────────────────────────────────────── ring 3: sessions
  {
    name: "session-2026-09-04",
    ring: 3,
    kind: "session",
    tags: ["session"],
    created: 1,
    updated: 1,
    retention: "P90D",
    body: `# Session 2026-09-04

- Scaffolded the workspace, six crates, \`deny.toml\` bans GPL/AGPL and \`openssl-sys\`.
- Wrote \`Ring\`, \`Frontmatter\`, \`Hit\`, \`EgressPurpose\` in core. Citations refuse to
  format without a ring.
- Decided the UI stack ([[api-contract-v1]]).
- Reproduced [[bug-012-cmd-exe-quoting]] on a VM with a space in the user name.

Handed off in [[handoff]].`,
  },
  {
    name: "session-2026-09-02",
    ring: 3,
    kind: "session",
    tags: ["session"],
    created: 3,
    updated: 3,
    retention: "P90D",
    body: `# Session 2026-09-02

Compared embedding backends on the hook path. Static embeddings at 0.4 ms per block
against 38 ms for a small transformer on this CPU. Decision recorded in
[[model2vec-in-tree]]. Observed that the transformer's recall gain on a Markdown corpus
of 24k blocks was within noise on the twelve test questions.`,
  },
  {
    name: "session-2026-08-27",
    ring: 3,
    kind: "session",
    tags: ["session"],
    created: 9,
    updated: 9,
    retention: "P90D",
    body: `# Session 2026-08-27

Retention design. Expiry never runs in the background: an agent's memory disappearing
without a record is indistinguishable from a bug. \`policy retention\` lists, \`--apply\`
erases through the forget path and writes an audit row per note.`,
  },
  {
    name: "session-2026-05-20",
    ring: 3,
    kind: "session",
    tags: ["session", "old"],
    created: 108,
    updated: 108,
    retention: "P90D",
    body: `# Session 2026-05-20

Early notes on what a memory tool must not do, collected from operating the predecessor.
Superseded by the requirements list in the specification. Kept for the record; due for
retention.`,
  },
  {
    name: "observation-audit-log-growth",
    ring: 3,
    kind: "knowledge",
    tags: ["audit", "observation"],
    created: 5,
    updated: 5,
    body: `# Observation: audit log growth

At one inference call per recall and ~300 recalls a day, the audit table grows by about
9k rows a month, roughly 2 MB. No rotation needed for years. Rows are append-only; the
table has no delete path by construction.`,
  },
  {
    name: "contact-notes-vendor-call",
    ring: 3,
    kind: "session",
    tags: ["vendor", "pii"],
    created: 14,
    updated: 14,
    retention: "P180D",
    pii: "reviewed",
    body: `# Vendor call notes

Call with the inference-appliance vendor about the pairing flow. Contact details were
found by the write-time scan and marked reviewed: they are business contacts and needed
for the follow-up. Follow-up in two weeks.`,
  },

  // ───────────────────────────────────────────── ring 4: imported / unverified
  {
    name: "imported-sqlite-fts5-notes",
    ring: 4,
    kind: "reference",
    tags: ["sqlite", "imported"],
    created: 51,
    updated: 51,
    body: `# Imported: FTS5 notes (unverified)

Imported from a colleague's scratch file. Claims, not yet checked:

- FTS5 \`bm25()\` returns **negative** numbers; smaller is better. (Confirmed later in
  [[hybrid-recall-rrf]] testing.)
- The \`trigram\` tokenizer makes substring search possible but triples index size.
- \`optimize\` after bulk insert halves query time on cold cache.`,
  },
  {
    name: "imported-postgres-data-dir-claim",
    ring: 4,
    kind: "reference",
    tags: ["postgres", "imported"],
    created: 30,
    updated: 30,
    body: `# Imported: "PGDATA stays at /var/lib/postgresql/data in every image version"

Copied from a forum answer dated 2023. States that the official image never changes the
data directory location and a bind mount to \`/var/lib/postgresql/data\` is safe forever.

This is contradicted by [[pg18-moves-pgdata]] for version 18. Kept as an example of why
imported material is ring 4.`,
  },
  {
    name: "imported-agent-harness-hook-payloads",
    ring: 4,
    kind: "reference",
    tags: ["hooks", "imported"],
    created: 25,
    updated: 25,
    body: `# Imported: harness hook payload examples

Sample JSON payloads for \`pre-tool-use\` and \`post-tool-use\` events as observed from
one harness version. Field names may have changed since; treat as a starting point and
verify against [[hook-payload-schema]] once written.`,
  },
];

/** Names that appear as `[[links]]` above but have no note: intent, shown as such. */
export const INTENDED_DANGLING = ["hook-payload-schema"];

/** Topic clusters used to generate the filler notes that make the graph a few hundred nodes. */
export const CLUSTERS: Array<{ prefix: string; tags: string[]; ring: Ring; kind: NoteKind; count: number; hub: string }> = [
  { prefix: "ops", tags: ["deployment", "ops"], ring: 2, kind: "knowledge", count: 38, hub: "deployment-parity-checklist" },
  { prefix: "index", tags: ["index", "sqlite"], ring: 2, kind: "knowledge", count: 30, hub: "sqlite-bundled-fts5" },
  { prefix: "embed", tags: ["embeddings"], ring: 2, kind: "knowledge", count: 24, hub: "model2vec-in-tree" },
  { prefix: "hook", tags: ["hooks"], ring: 2, kind: "knowledge", count: 22, hub: "hook-budget-15ms" },
  { prefix: "bug", tags: ["bug"], ring: 2, kind: "bug", count: 26, hub: "do-not-repeat" },
  { prefix: "session", tags: ["session"], ring: 3, kind: "session", count: 40, hub: "handoff" },
  { prefix: "obs", tags: ["observation"], ring: 3, kind: "knowledge", count: 18, hub: "observation-audit-log-growth" },
  { prefix: "imported", tags: ["imported"], ring: 4, kind: "reference", count: 16, hub: "imported-sqlite-fts5-notes" },
];

export const FILLER_SENTENCES = [
  "Measured on the development machine; numbers are indicative, not a benchmark.",
  "The change was verified by running the check against the broken state first.",
  "Recorded so the next session does not rediscover it.",
  "Follow-up needed once the Windows job is green.",
  "Rejected alternative: a second implementation for dry-run, which drifts.",
  "Counts name the side of the boundary they count.",
  "The file on disk is authoritative; the index is a cache and was rebuilt afterwards.",
  "Reported by doctor as a dangling link until the target note exists.",
  "Ring assignment provisional; promote after a second confirmation.",
  "No network call was involved; the egress register shows no change.",
];
