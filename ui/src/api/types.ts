/**
 * Cyberbrain HTTP API contract as the UI sees it.
 *
 * Precedence (SPEC §8.1): where a shape is fixed by a `serde` derive in a Rust crate, the
 * Rust type wins and this file follows it. This file is authoritative only for what the
 * crates do not define — request shapes, view models, anything assembled for the screen.
 * Every interface below names the Rust type it mirrors; the ones marked "UI-owned" are
 * ours. This file was corrected against the crates on 2026-09-05; the wire structs live in
 * `crates/cyberbrain/src/serve/wire.rs`, the core enums in
 * `crates/cyberbrain-core/src/types.rs`, the audit vocabulary in
 * `crates/cyberbrain-policy/src/audit.rs`, and the report structs in
 * `crates/cyberbrain/src/app.rs`.
 *
 * Conventions
 * ───────────
 * • Base path `/api/v1`, same origin as the page (`cyberbrain serve`, 127.0.0.1:7777).
 * • JSON in, JSON out. Timestamps are RFC 3339 UTC strings (`jiff::Timestamp`), e.g.
 *   `2026-09-05T10:00:00Z`. Durations are ISO-8601 (`P2Y`, `P90D`).
 * • **Paths render with forward slashes on every platform, including Windows**, in every
 *   JSON field documented as a path (`cyberbrain_core::path_serde::slash`). They are
 *   relative to the store root where the doc says so, otherwise absolute.
 * • Notes are addressed by `name` (slug) OR `id` (ULID) in the path; the server tries the
 *   ULID form first (26 Crockford-base32 chars), else treats it as a name.
 * • Errors: any non-2xx carries `ApiErrorBody`. The HTTP status is derived from the CLI
 *   exit code taxonomy (SPEC §8): 400 → user error (1), 500 → internal (2), 403 → policy
 *   refusal (3); 404 for an unknown note/citation; 409 for the two typed write outcomes
 *   (a stale `expected_updated`, a PII hold). `exit_code` follows the taxonomy, not the
 *   status: a PII hold is 409 with exit_code 3.
 * • Every mutating route accepts `?dry_run=true` and runs the real code path with a no-op
 *   writer (SPEC §8 / §14.6). The response has the same shape plus a `dry_run: true`
 *   marker where the Rust struct carries one.
 * • The server never redirects and never sets a cookie; there is no auth — the socket is
 *   loopback-only and the bind address is not configurable.
 * • Additive members: TypeScript ignores JSON members it does not declare, so a newer
 *   binary may send more than is listed here. Nothing listed here is sent as `undefined`
 *   unless the field is marked optional.
 */

// ───────────────────────────────────────────────────────────────────────────── core types

/** `cyberbrain_core::Ring`. Serialised as a bare integer 0..4 (`try_from = "u8"`). */
export type Ring = 0 | 1 | 2 | 3 | 4;

/** `cyberbrain_core::NoteKind`, lowercase. */
export type NoteKind = "knowledge" | "bug" | "lesson" | "decision" | "reference" | "session";

/**
 * `cyberbrain_core::PiiState`, lowercase. **Four** values, and the default is `unscanned`,
 * not `none`: an absent `pii:` key means nobody looked. `none` is "a scan ran and found
 * nothing". The compliance layer turns on that difference; never collapse the two.
 */
export type PiiState = "unscanned" | "none" | "reviewed" | "flagged";

/** ULID, 26 chars. `cyberbrain_core::NoteId`. */
export type NoteId = string;

/** `r{ring}-{10 hex}`, SPEC §3.3. */
export type Citation = string;

/** `cyberbrain_core::PolicyProfile`, lowercase. */
export type PolicyProfile = "eu" | "ch" | "off";

/**
 * `cyberbrain_core::Frontmatter`, serialised as-is. `tags` and `links` carry
 * `skip_serializing_if = "Vec::is_empty"` and are **absent** when empty; `retention` is
 * absent when unset.
 */
export interface Frontmatter {
  id: NoteId;
  name: string;
  ring: Ring;
  kind: NoteKind;
  created: string;
  updated: string;
  /** Absent when empty. */
  tags?: string[];
  /** Derived from `[[...]]` in the body, written back by scan. Absent when empty. */
  links?: string[];
  /** ISO-8601 duration; absent means indefinite. */
  retention?: string;
  pii: PiiState;
}

// ───────────────────────────────────────────────────────────────────────────── /status

/** `serve::ops::backend_name` over `cyberbrain_llm::Backend`. */
export type InferenceBackend = "ollama" | "lm-studio" | "nvidia-pair" | "unknown";

/**
 * `serve::policy::endpoint_class`. The register's own `Locality` has one more class,
 * `overlay` (100.64.0.0/10, Tailscale-style), which needs its own consent flag exactly
 * like a public address does; the server reports it as `public` here and the
 * `EgressPath.state` text says which it really is. There is no `overlay` value on the wire.
 */
export type EndpointClass = "loopback" | "private" | "public" | "unresolved";

/** `wire::RingCount`. Counted from the files under `notes/`, blocks by core's splitter. */
export interface RingCount {
  ring: Ring;
  notes: number;
  blocks: number;
  tokens: number;
  bytes: number;
}

/**
 * GET /api/v1/status → 200 StatusReport (`wire::StatusReport`).
 * Not the same shape as `cyberbrain status --json` (`app::StatusReport`); this one is
 * reshaped for the screen and says in `caveats` which numbers are not measured.
 */
export interface StatusReport {
  version: string;
  store: {
    /** Absolute store root, forward slashes. */
    path: string;
    /** `notes_bytes + db_bytes`. */
    bytes: number;
    notes_bytes: number;
    db_bytes: number;
    models_bytes: number;
    notes: number;
    blocks: number;
    vectors: number;
    rings: RingCount[];
    /** SPEC §3.2: rings 0+1 combined budget. */
    resident_cap: { tokens: number; used: number };
  };
  index: {
    schema_version: number;
    /** Known only for scans run through this server process; a CLI scan leaves no stamp (see `caveats`). */
    last_scan: string | null;
    /** As above, or the last non-dry `index.cleared` audit row. */
    last_full_scan: string | null;
    /** `indexed_new + reindexed_changed + dropped_missing_file` of a dry incremental scan. */
    stale_notes: number;
    /** Parsed out of doctor's "index integrity" findings. Unmeasured when that check did not run (see `caveats`). */
    orphan_vectors: number;
    /** All dangling links, intent and unresolvable alike; `doctor` tells them apart. */
    dangling_links: number;
    fts_ok: boolean;
  };
  embedding: {
    /** e.g. "model2vec/potion-base-8M@d256/mean-l2"; falls back to the index's last profile, then the configured one. */
    profile_id: string;
    model: string;
    dim: number;
    pooling: string;
    backend: "static" | "candle";
    /** `true` also when there is nothing to compare (no model or no vectors) — the caveats say so. */
    /** `null` when there is nothing to compare: no model loaded, or no vectors stored.
     *  Not the same as `false`, which means the stored vectors came from a different
     *  model and semantic search is off. */
    matches_index: boolean | null;
    model_hash: string;
    /** Always `null`: the hash is verified on every load but no timestamp of that load is kept. */
    model_verified_at: string | null;
    /** `false` when no model artefact is loaded; every other member of this block is then a description of the index's last profile, not of a live model. */
    loaded: boolean;
  };
  inference: {
    configured: boolean;
    base_url: string;
    endpoint_class: EndpointClass;
    allow_public_endpoint: boolean;
    model: string | null;
    /** From the most recent probe's response fingerprint; `unknown` when no probe ran. */
    last_backend: InferenceBackend;
    last_backend_evidence: string | null;
    /** Timestamp of the newest `inference.call` audit row. */
    last_call: string | null;
    /** Result of the probe `App::status` ran for this request; `null` when it did not probe. */
    reachable: boolean | null;
    reachable_checked_at: string | null;
  };
  policy: {
    profile: PolicyProfile;
    pii_scan: boolean;
    audit_rows: number;
  };
  /** Which members above are not measured by this build, and why. Render them. */
  caveats: string[];
}

// ───────────────────────────────────────────────────────────────────────────── /recall

/** UI-owned request shape. `wire::RecallParams` deserialises the same names. */
export interface RecallParams {
  q: string;
  /** Default `retrieval.n` from config (8). 400 outside 1..=100. */
  n?: number;
  /** Restrict to one ring. 400 (`variant: "bad-ring"`) for anything else. */
  ring?: Ring;
}

/** `wire::Hit`: core's `Hit` plus `block_idx` and `sources`. */
export interface Hit {
  citation: Citation;
  note_id: NoteId;
  note_name: string;
  ring: Ring;
  /** Fused, ring-weighted score (SPEC §7 step 4). Not normalised; compare within a result. */
  score: number;
  text: string;
  /** Resolved from the citation after the fact; 0 when the citation did not resolve. */
  block_idx: number;
  /**
   * `["lexical"]` when `mode` is `lexical`; **empty** for a hybrid result, because the
   * index does not report per-hit candidate lists (a caveat says so). Empty means
   * "not reported", never "came from nowhere". `"semantic"` is never sent today.
   */
  sources: Array<"lexical" | "semantic">;
}

/** `cyberbrain_core::Conflict`. */
export interface Conflict {
  winner: Citation;
  loser: Citation;
  reason: string;
}

/**
 * GET /api/v1/recall?q=…&n=8&ring=2 → 200 RecallResult (`wire::RecallResult`).
 * 400 if `q` is empty or `n` is outside 1..=100.
 */
export interface RecallResult {
  hits: Hit[];
  conflicts: Conflict[];
  /** Checks that were skipped and why. Rendered verbatim; never empty out of politeness. */
  caveats: string[];
  /** `lexical` when a caveat says the semantic half did not run; `hybrid` otherwise. */
  mode: "hybrid" | "lexical";
  /** Wall clock of the whole request, fractional. */
  elapsed_ms: number;
  /** Echo of the effective parameters. */
  params: { q: string; n: number; ring: Ring | null; k_lex: number; k_sem: number };
}

/**
 * GET /api/v1/recall/{citation} → 200 CitationExpansion (`wire::CitationExpansion`) | 404
 * Same as `cyberbrain recall --id`.
 */
export interface CitationExpansion {
  citation: Citation;
  ring: Ring;
  block_idx: number;
  block_text: string;
  token_count: number;
  note: NoteDetail;
}

// ───────────────────────────────────────────────────────────────────────────── /notes

/** `wire::NoteSummary`. Unlike `Frontmatter`, `tags` is always present here. */
export interface NoteSummary {
  id: NoteId;
  name: string;
  ring: Ring;
  kind: NoteKind;
  tags: string[];
  updated: string;
  created: string;
  /** Absent when unset. */
  retention?: string;
  pii: PiiState;
  blocks: number;
  /** File size on disk. */
  bytes: number;
  links_out: number;
  links_in: number;
  /** Outbound `[[links]]` with no target note, intent and unresolvable alike. */
  dangling: number;
}

/** UI-owned request shape. `wire::NoteListParams` deserialises the same names. */
export interface NoteListParams {
  ring?: Ring;
  kind?: NoteKind;
  /** Case-insensitive substring match on name and tags; not a recall. */
  q?: string;
  /** Anything else is a 400. */
  sort?: "updated" | "name" | "ring";
}

/** GET /api/v1/notes?ring=&kind=&q=&sort= → 200 NoteSummary[] */

/** `wire::OutboundLink`. */
export interface OutboundLink {
  /** The literal text inside `[[…]]`. */
  target: string;
  /** Null when dangling: intent, not error (SPEC §3.1) — unless the name can never be a note; `doctor` says which. */
  resolved: { id: NoteId; ring: Ring } | null;
}

/** `wire::InboundLink`. */
export interface InboundLink {
  from: { id: NoteId; name: string; ring: Ring };
}

/** `wire::BlockRef`. */
export interface BlockRef {
  citation: Citation;
  idx: number;
  token_count: number;
  /** First line of the block, `#` stripped, at most 80 chars. */
  preview: string;
}

/**
 * GET /api/v1/notes/{name|id} → 200 NoteDetail (`wire::NoteDetail`) | 404
 */
export interface NoteDetail {
  front: Frontmatter;
  body: string;
  /** Relative to the store root, forward slashes, e.g. `notes/r2/pg18-moves-pgdata.md`. */
  path: string;
  outbound: OutboundLink[];
  inbound: InboundLink[];
  blocks: BlockRef[];
  /** Present (and `true`) only when this describes what a `?dry_run=true` write would have written; the disk is unchanged. */
  dry_run?: true;
}

/** UI-owned request shape; parsed by hand in `serve::notes::parse_write`. */
export interface NoteWriteRequest {
  body: string;
  /**
   * Optional frontmatter edits. `id`, `created`, `updated`, `links` and `pii` are
   * server-owned: sending any of them is 400 `bad-frontmatter`, as is any unknown key.
   * `retention: null` (or `""`) clears the retention.
   *
   * On PUT, `name` must equal the current name: a rename is refused with 400
   * `bad-request` (it would create a second note with a new id rather than move this one).
   * A `ring` different from the current one is refused by the store with 400
   * `bad-frontmatter` ("already exists in ring rN; remove it first to move the note").
   */
  front?: Partial<Pick<Frontmatter, "name" | "ring" | "kind" | "tags">> & { retention?: string | null };
  /**
   * Optimistic concurrency: the `updated` you last saw. 409 `write-conflict` (with
   * `current_updated`) if it moved. The server treats an absent value as "do not check";
   * the UI always sends it.
   */
  expected_updated: string;
}

/**
 * `wire::PiiFinding`, rendered by `serve::notes::render_findings`. `kind` folds the policy
 * crate's `ipv4`/`ipv6` into `ip`. `line`/`col` are 1-based, col in chars.
 */
export interface PiiFinding {
  kind: "email" | "ip" | "api-key" | "iban" | "phone";
  /** Masked excerpt, e.g. `j***@example.org`. Never the raw match. */
  excerpt: string;
  line: number;
  col: number;
}

/**
 * PUT /api/v1/notes/{name|id}
 *   body NoteWriteRequest
 *   200 NoteDetail            written and reindexed in the same request (audit `note.write`)
 *   409 ApiErrorBody          code "write-conflict" + `current_updated` (exit 1), or
 *                             code "pii-held" + `hold: PiiHold` (exit 3; SPEC §12.4, eu/ch only)
 *   400 ApiErrorBody          code "ring-cap-exceeded" + `cap` (SPEC §3.2);
 *                             code "bad-frontmatter" (server-owned key, unknown key, bad
 *                             value, ring change, invalid name or retention);
 *                             code "bad-request" (rename, malformed body)
 *   403 ApiErrorBody          code "policy-refusal"
 *   404                       unknown note
 *
 * POST /api/v1/notes  body NoteWriteRequest & { front: { name, ring, kind } } → 201 NoteDetail
 *   (200 when `?dry_run=true`). `expected_updated` ignored. A name that already exists is
 *   409 "write-conflict" + `current_updated`: creation never updates in place.
 */
export interface PiiHold {
  /** ULID. */
  hold_id: string;
  /** Note name. */
  note: string;
  findings: PiiFinding[];
  /** 15 minutes after the held write; after this the client must resubmit. */
  expires_at: string;
  /** Present (and `true`) when the held write was a `?dry_run=true`; resolving it writes nothing. */
  dry_run?: true;
}

/**
 * POST /api/v1/holds/{hold_id}  body PiiHoldResolution
 *   200 NoteDetail    written according to `action` (201 when the hold came from a POST);
 *                     `pii` becomes "none" (redact), "reviewed" (mark-reviewed) or
 *                     "flagged" (proceed). Audit `note.write.resolved`.
 *   204               action "discard": nothing written; audit `note.write.discarded`.
 *   400               unknown action
 *   404               unknown or expired hold
 *   409               the note moved since the hold was taken (`write-conflict`)
 * `?dry_run=true` on the resolution previews the choice without writing.
 */
export interface PiiHoldResolution {
  action: "redact" | "mark-reviewed" | "proceed" | "discard";
}

/**
 * DELETE /api/v1/notes/{name|id}?dry_run= → 200 ForgetReport (`wire::ForgetReport`)
 * Same path as `cyberbrain forget` (SPEC §12.2). Audit `note.erase.requested` then
 * `note.erase.completed` or `note.erase.failed`.
 */
export interface ForgetReport {
  dry_run: boolean;
  /** `path` is store-relative with forward slashes; empty when the file was already gone. */
  note: { id: NoteId; name: string; path: string };
  removed: {
    file: boolean;
    blocks: number;
    vectors: number;
    fts_rows: number;
    /** Inbound link rows that now point nowhere. They are kept (SPEC §12.2) and become dangling. */
    links_in: number;
    links_out: number;
    derivatives: number;
  };
  /** What the eraser could not confirm, in words; on a dry run also the audit rows a real run would append. */
  notes: string[];
}

// ───────────────────────────────────────────────────────────────────────────── /graph

/** `wire::GraphNode`. */
export interface GraphNode {
  id: NoteId;
  name: string;
  ring: Ring;
  kind: NoteKind;
  links_in: number;
  links_out: number;
}

/** `wire::GraphEdge`. */
export interface GraphEdge {
  from: NoteId;
  to: NoteId;
}

/** `wire::DanglingLink`. A `[[link]]` with no target: rendered as intent, never hidden (SPEC §13.3). */
export interface DanglingLink {
  from: NoteId;
  to_name: string;
}

/** GET /api/v1/graph → 200 Graph (`wire::Graph`). Whole store; the UI filters client-side. */
export interface Graph {
  nodes: GraphNode[];
  edges: GraphEdge[];
  dangling: DanglingLink[];
}

// ───────────────────────────────────────────────────────────────────────────── /policy

/** `cyberbrain_core::EgressPurpose`, kebab-case. The list is closed by construction. */
export type EgressPurpose = "model-download" | "local-inference";

/** `wire::EgressPath`. */
export interface EgressPath {
  purpose: EgressPurpose;
  /** `EgressPurpose::describe`. */
  description: string;
  /** For inference the configured base URL; for downloads the configured model source, or "none configured". */
  destination: string;
  destination_class: EndpointClass;
  /** What is sent. Plain language, from the register, not inferred. */
  data: string;
  /** Always all three: the gate is not a profile feature. */
  permitted_by: PolicyProfile[];
  /** Whether the path can be used right now, given the configuration. */
  enabled: boolean;
  /** `state` when disabled; null when enabled. */
  disabled_reason: string | null;
  /** Timestamp of the newest `egress.permitted` row for this purpose. */
  last_used: string | null;
  /** Count of `egress.permitted` rows for this purpose, all time. */
  uses_total: number;
  /** Sum of `detail.bytes_out` over `egress.completed` rows for this purpose. A permitted request whose ticket was never closed adds nothing here. */
  bytes_out_total: number;
  /** The register's own wording of the enable state, also when enabled. */
  state: string;
  /** Whether note content can be in a request on this path. */
  carries_note_content: boolean;
}

/**
 * GET /api/v1/policy/egress → 200 EgressRegister (`wire::EgressRegister`)
 */
export interface EgressRegister {
  profile: PolicyProfile;
  /** Timestamp of the first audit row (normally `store.init`); "now" when the log is empty. */
  since: string;
  paths: EgressPath[];
  /** Count of `policy.refusal` rows. */
  refused_total: number;
  /** `b3:` + blake3 over the compile-time register text. */
  register_hash: string;
}

/** `cyberbrain_policy::profile::Topic`, kebab-case on the wire. */
export type ObligationTopic =
  | "scope"
  | "legal-basis"
  | "erasure"
  | "access"
  | "breach-notification"
  | "processing-register"
  | "data-protection-officer"
  | "impact-assessment"
  | "cross-border-transfer"
  | "sanctions"
  | "ai-regulation";

/** How sure the author is of a line. `low` never drives behaviour in the binary. */
export type Confidence = "low" | "medium" | "high";

/** One thing the profile claims about the law. `note` is empty when nothing needs saying. */
export interface Obligation {
  topic: ObligationTopic;
  summary: string;
  basis: string;
  confidence: Confidence;
  note: string;
}

/**
 * GET /api/v1/policy/obligations → 200 ObligationsView (`app::ObligationsView`)
 *
 * The catalogue the active profile encodes. It is the answer to "what does eu actually
 * mean here", and it carries its own uncertainty: the confidence is on every line so the
 * gap between "verified in the primary text" and "checked from memory" is visible rather
 * than averaged away.
 */
export interface ObligationsView {
  profile: PolicyProfile;
  /** The law the profile names, e.g. "GDPR, Regulation (EU) 2016/679". */
  law: string;
  obligations: Obligation[];
}

/**
 * The audit vocabulary the log actually speaks. `cyberbrain_policy::AuditAction::as_str`
 * plus the actions the binary writes through `record_raw`. The row's `action` column is
 * an open string on purpose (rows from a newer binary must read back), so treat unknown
 * names as data, not as errors.
 */
export type AuditActionKnown =
  | "note.write"
  | "note.write.held"
  | "note.write.resolved"
  | "note.write.discarded"
  | "note.proposed"
  | "note.proposal.accepted"
  | "note.proposal.rejected"
  | "note.erase.requested"
  | "note.erase.completed"
  | "note.erase.failed"
  | "egress.permitted"
  | "egress.completed"
  | "egress.failed"
  | "egress.abandoned"
  | "policy.refusal"
  | "inference.call"
  | "subject.access"
  | "retention.expired"
  | "audit.export"
  | "store.init"
  | "index.cleared"
  | "index.note-dropped"
  | "index.embedding-profile-changed"
  | "consent.model-download";

export type AuditAction = AuditActionKnown | (string & {});

/** Every known action, for a dropdown. Kept in the order of the Rust enum. */
export const AUDIT_ACTIONS: readonly AuditActionKnown[] = [
  "note.write",
  "note.write.held",
  "note.write.resolved",
  "note.write.discarded",
  "note.proposed",
  "note.proposal.accepted",
  "note.proposal.rejected",
  "note.erase.requested",
  "note.erase.completed",
  "note.erase.failed",
  "egress.permitted",
  "egress.completed",
  "egress.failed",
  "egress.abandoned",
  "policy.refusal",
  "inference.call",
  "subject.access",
  "retention.expired",
  "audit.export",
  "store.init",
  "index.cleared",
  "index.note-dropped",
  "index.embedding-profile-changed",
  "consent.model-download",
];

/**
 * What `?action=` accepts (`serve::policy::action_matches`): an exact action name, or a
 * **family prefix** — `note` selects `note.*`, `note.erase` selects `note.erase.*`,
 * `egress` selects `egress.*`. The pre-server vocabulary (`write`, `erase`, `scan`,
 * `model-download`, `inference`, `policy-refusal`, `egress-attempt`, `egress-refused`,
 * `retention-apply`, `hold-resolved`, `subject-access`) is still translated. A filter that
 * matches nothing in a non-empty log is refused with 400 and a message listing the names
 * that are there: "no such name" and "never happened" are different answers.
 */
export type AuditActionFilter = AuditAction | "note" | "note.write" | "note.erase" | "egress" | "index";

/** Families for the UI's dropdown, in display order. The wording lives in the dictionary. */
export const AUDIT_ACTION_FAMILIES: ReadonlyArray<AuditActionFilter> = [
  "note",
  "note.write",
  "note.proposal",
  "note.erase",
  "egress",
  "policy.refusal",
  "inference.call",
  "retention.expired",
  "subject.access",
  "index",
  "store.init",
  "audit.export",
  "consent.model-download",
];

/**
 * `wire::AuditRow`. `seq` is the row's 1-based position in the append-only log, oldest
 * first — the same number as the store's AUTOINCREMENT id, because deletes are refused by
 * trigger. `actor` is `Actor`'s Display form: `operator`, `agent:<name>`, `cli`,
 * `hook:<event>`, `mcp`, `retention`, `system:<name>`. The server's own writes are `operator`.
 */
export interface AuditRow {
  seq: number;
  ts: string;
  actor: string;
  action: AuditAction;
  /** `note:<name>`, `note:<id>`, `<purpose>:<destination>`, `inference:<endpoint>`, `store:<path>`, `index`, `audit`, an identifier hash… */
  subject: string;
  /**
   * The row's `detail` **without** the `_chain` block, flattened to one level by
   * `serve::policy::flat_detail`: scalars stay scalars, and any nested object or array
   * arrives as its **JSON text** in a string (e.g. `findings` on `note.write.held`,
   * `outcome_detail` on `inference.call`). Parse it if you need the structure.
   */
  detail: Record<string, string | number | boolean | null>;
}

/** UI-owned request shape. `wire::AuditParams` deserialises the same names. */
export interface AuditParams {
  /** Default 100, clamped to 1..=1000. */
  limit?: number;
  /** Return rows with seq < before. */
  before?: number;
  /** See `AuditActionFilter`. */
  action?: AuditActionFilter;
  /** Prefix match on `actor`. */
  actor?: string;
  /** Case-insensitive substring over actor, action, subject and detail. */
  q?: string;
}

/**
 * GET /api/v1/policy/audit?limit=&before=&action=&actor=&q= → 200 AuditPage (`wire::AuditPage`)
 * Rows newest first. `total` counts every row the filters match, before `before` is applied.
 * 400 when `action` matches nothing in a non-empty log (see `AuditActionFilter`).
 */
export interface AuditPage {
  rows: AuditRow[];
  total: number;
  next_before: number | null;
}

/** `wire::NoteRef`. */
export interface NoteRef {
  id: NoteId;
  name: string;
  ring: Ring;
}

/**
 * `wire::PiiReportEntry`. One per note whose `pii` is **not** `none` — that includes
 * `unscanned` notes (nobody looked) alongside `reviewed` and `flagged`. `findings` is what
 * a scan of the body finds *now*, whatever the state says; for an `unscanned` note it is
 * the first time anyone looked.
 */
export interface PiiReportEntry {
  note: NoteRef;
  state: PiiState;
  findings: PiiFinding[];
  /** The `updated` stamp of the write that set the state; null for `unscanned` (no scan ever ran). */
  reviewed_at: string | null;
}

/**
 * GET /api/v1/policy/pii → 200 PiiReport (`wire::PiiReport`)
 * Entries sorted flagged, reviewed, unscanned, then by name. `holds` are the pending
 * in-memory holds of this server process, oldest first.
 */
export interface PiiReport {
  scan_enabled: boolean;
  entries: PiiReportEntry[];
  holds: PiiHold[];
}

/** `wire::RetentionEntry`, from `cyberbrain_policy::RetentionItem`. */
export interface RetentionEntry {
  note: NoteRef;
  /** The frontmatter value as written, valid or not. */
  retention: string;
  /** created + retention. When `invalid` is set this is the note's `created` (there is no expiry). */
  expires_at: string;
  due: boolean;
  /** Present when `retention` does not parse: the note the operator meant to expire never will. `due` is then `false`. */
  invalid?: string;
}

/**
 * GET /api/v1/policy/retention → 200 RetentionQueue (`wire::RetentionQueue`)
 * POST /api/v1/policy/retention/apply?dry_run=true body {} → 200 RetentionApplyReport
 *   Erases every due note through the forget path (SPEC §12.5); audit `retention.expired`
 *   per note plus the erase rows. A body with a non-empty `names` is refused with 400: the
 *   sweep is one path and a subset is not what its audit row records. Use DELETE on a
 *   single note instead.
 */
export interface RetentionQueue {
  /** Due first, then pending by expiry, then invalid. */
  entries: RetentionEntry[];
  due: number;
  /** Notes without a `retention` field: the denominator (SPEC §14.4). */
  indefinite: number;
}

/** `wire::RetentionApplyReport`. */
export interface RetentionApplyReport {
  dry_run: boolean;
  removed: ForgetReport[];
  /** Erase failures, invalid retentions, and unreadable notes, each with its reason. */
  skipped: Array<{ name: string; reason: string }>;
}

/**
 * `wire::ModelCard`, reshaped from `cyberbrain_policy::ModelCard`. Every key is always
 * present; the nullable ones are what the crate could not vouch for. The policy card's
 * `version`, `hash_verified` and `artefact_path` are not exposed here.
 */
export interface ModelCard {
  /** `cyberbrain_policy::ModelRole`, kebab-case. */
  role: "embedding" | "inference";
  name: string;
  /** Where the artefact came from, e.g. the download URL or the endpoint base URL. */
  source: string;
  /** SPDX identifier or licence name exactly as the source states it; `"not stated"` when the crate did not read one. Never guessed. */
  license: string;
  /** blake3 of the artefact, 64 lowercase hex chars, no prefix; null for an endpoint model we do not hold. */
  hash: string | null;
  format: string | null;
  dim: number | null;
  pooling: string | null;
  bytes: number | null;
  intended_use: string;
  /** The card's limitations and notes joined with "; ". */
  limitations: string;
  /** Always `null`: the hash is verified on every load and no timestamp of that load is kept. */
  verified_at: string | null;
  /** Always `true`: only models in use get a card. */
  active: boolean;
}

/** GET /api/v1/policy/model-card → 200 ModelCard[]. Empty when no model is loaded. */

/**
 * `wire::SubjectAccessHit`. `where` is `block` for note hits (with the citation and the
 * note name in `ref`) and `audit` for audit rows (`ref` is `audit@<ts>`, no citation).
 * `note` is declared for completeness and is never sent today.
 */
export interface SubjectAccessHit {
  where: "note" | "block" | "audit";
  ref: string;
  citation: Citation | null;
  excerpt: string;
}

/**
 * GET /api/v1/policy/subject?q=<identifier> → 200 SubjectAccessReport (`wire::SubjectAccessReport`)
 * SPEC §12.3. Writes an audit row `subject.access`. 400 when `q` is empty.
 */
export interface SubjectAccessReport {
  identifier: string;
  hits: SubjectAccessHit[];
  searched: { notes: number; blocks: number; audit_rows: number };
  /** The policy crate's own caveats (heuristics, what was not searched) plus any trimmed candidates. */
  caveats: string[];
  /** The legal response deadline for the active profile, in words. */
  response_deadline: string;
}

// ───────────────────────────────────────────────────────────────────────────── /doctor, /scan

/**
 * `serve::ops::doctor_check` over `App::doctor`'s check names. Kebab-cased from the
 * human name with five renames; the plural/singular inconsistency between `dangling-link`
 * and `unresolvable-links` is the server's. Open string: a new check arrives as its
 * kebab-cased name.
 */
export type DoctorCheckKnown =
  | "notes-tree"
  | "unreadable-note"
  | "stale-index"
  /** A valid name nobody has written yet: intent. Wait for it. */
  | "dangling-link"
  /** A name that can never be a note (capitals, underscores, a path): a typo or another tool's naming. Waiting will not resolve it. */
  | "unresolvable-links"
  | "ring-cap"
  /** From the "index integrity" check: orphan vectors, FTS drift. */
  | "orphan-vector"
  | "profile-mismatch"
  | "audit-chain"
  | "retention";

export type DoctorCheck = DoctorCheckKnown | (string & {});

/** `wire::DoctorFinding`. */
export interface DoctorFinding {
  /** `App` emits `error` and `warning`; the server maps `warning` to `warn` and anything else to `info`. */
  severity: "info" | "warn" | "error";
  check: DoctorCheck;
  /** The from-note name for `dangling-link`; a path for tree/note checks; `r0+r1`, `index`, `audit.db`, or `store`. */
  subject: string;
  message: string;
}

/**
 * GET /api/v1/doctor → 200 DoctorReport (`wire::DoctorReport`). Runs the checks; read-only.
 */
export interface DoctorReport {
  /** `true` when there are no findings at all (warnings included). */
  ok: boolean;
  checked_at: string;
  findings: DoctorFinding[];
  /** The checks that ran, in their human names ("dangling links", "unresolvable links", "index integrity", …), so an empty findings list has a population. */
  checks_run: string[];
}

/** `app::SkippedFile`. `path` has forward slashes. */
export interface SkippedFile {
  path: string;
  reason: string;
}

/** `cyberbrain_index::EmbeddingProfile`. */
export interface EmbeddingProfile {
  id: string;
  dim: number;
  model_hash: string;
}

/** `cyberbrain_index::IndexStats`. */
export interface IndexStats {
  schema_version: number;
  generation: number;
  notes: number;
  blocks: number;
  fts_rows: number;
  vectors: number;
  links: number;
  dangling_links: number;
  embedding: EmbeddingProfile | null;
}

/** `app::ScanReport`: every count names the side of the boundary it counts (SPEC §14.3). */
export interface ScanDetail {
  dry_run: boolean;
  full: boolean;
  /** Entries the walker listed under `notes/r{0..4}/` plus what it declined to list. */
  files_listed: number;
  indexed_new: number;
  reindexed_changed: number;
  /** Content unchanged, but blocks had no vectors and a model is now present. */
  revectorised: number;
  unchanged: number;
  /** mtime moved, bytes identical: not reindexed. */
  touched_only: number;
  /** Notes the index knew whose file is gone. */
  dropped_missing_file: string[];
  skipped: SkippedFile[];
  links_written_back: number;
  link_writeback_failed: string[];
  oversized_blocks: Array<{ note: string; block_idx: number; approx_tokens: number; reason: string }>;
  embedder: { loaded: boolean; profile_id: string | null; dim: number | null; reason: string | null };
  profile_change: { previous: EmbeddingProfile | null; current: EmbeddingProfile; changed: boolean; vectors_wiped: number } | null;
  cleared: { notes: number; blocks: number; fts_rows: number; vectors: number; links_out: number; links_in_unresolved: number } | null;
  /** The index after the scan. */
  index: IndexStats;
  /** Dry run only: the audit actions a real run would have appended. */
  audit_preview: string[];
  elapsed_ms: number;
}

/**
 * POST /api/v1/scan?full=false&dry_run=false → 200 ScanReport (`wire::ScanReport`)
 * The top-level counters are the screen's summary; `detail` is the real report.
 */
export interface ScanReport {
  full: boolean;
  elapsed_ms: number;
  /** `detail.files_listed`. */
  scanned: number;
  /** `detail.reindexed_changed + detail.revectorised`. */
  changed: number;
  /** `detail.indexed_new`. */
  added: number;
  /** `detail.dropped_missing_file.length`. */
  removed: number;
  /**
   * **Index totals after the scan**, not what this scan wrote: `App::scan` counts notes,
   * not blocks. Exact only for a full rebuild; the caveats say so for an incremental scan.
   */
  blocks_written: number;
  /** As `blocks_written`. */
  vectors_written: number;
  dry_run: boolean;
  detail: ScanDetail;
  caveats: string[];
}

// ───────────────────────────────────────────────────────────────────────────── find (MCP only)

/**
 * `app::FindHit`. `find` is exposed over MCP (`structuredContent` of the `find` tool) and
 * the CLI, **not** over HTTP: there is no `/api/v1/find` route. Declared here so a client
 * of the MCP surface has a typed shape; derived from `app.rs`, never the other way round.
 * Line numbers are 1-based and `start_line..=end_line` is inclusive.
 */
export interface FindHit {
  /** Relative to `FindReport.root`, forward slashes. */
  path: string;
  start_line: number;
  end_line: number;
  /** The line that names the symbol; lies inside the range. */
  line: number;
  /** `function`, `method`, `class`, `struct`, `enum`, `trait`, `interface`, `type`, `impl`, `module`, `namespace`, `macro`, `const`, `static`, `variable`, `table`, `view`, `index`, `trigger`, `schema`, `section`, `key`, `heading`. */
  kind: string;
  language: string;
  name: string;
  /** The enclosing named thing (impl target, class, TOML table, parent key path). */
  scope: string | null;
  matched: "exact" | "case-insensitive" | "contains";
  /** The defining line, trimmed, at most 160 characters. */
  snippet: string;
}

/** `app::FindSkipped`: what `find` declined to read, by reason. */
export interface FindSkipped {
  ignored_entries: number;
  gitignored_entries: number;
  hidden_entries: number;
  store_entries: number;
  symlinks: number;
  lockfiles: number;
  too_large: number;
  binary: number;
  unsupported: number;
  unsupported_by_extension: Record<string, number>;
  unreadable: SkippedFile[];
}

/** `app::FindReport`. */
export interface FindReport {
  /** The symbol as given. */
  symbol: string;
  /** The name part after scope splitting (`find` for `App::find`). */
  name: string;
  scope: string | null;
  /** The tree that was scanned; absolute, forward slashes. */
  root: string;
  /** Best first, at most `limit`. */
  hits: FindHit[];
  matched_total: number;
  truncated: boolean;
  limit: number;
  files_scanned: number;
  bytes_scanned: number;
  definitions_indexed: number;
  skipped: FindSkipped;
  /** Root-relative paths of the ignore files honoured, in walk order. */
  ignore_files: string[];
  caveats: string[];
  elapsed_ms: number;
}

// ───────────────────────────────────────────────────────────────────────────── errors

/** `serve::error`. The closed set of `code`; the CLI's finer variant name rides along as `variant`. */
export type ApiErrorCode =
  | "bad-request"
  | "not-found"
  | "write-conflict"
  | "pii-held"
  | "ring-cap-exceeded"
  | "bad-frontmatter"
  | "policy-refusal"
  /** 500, not 400: an embedding profile mismatch is internal (exit 2). */
  | "profile-mismatch"
  | "internal";

export interface ApiErrorBody {
  error: {
    code: ApiErrorCode;
    message: string;
    /** CLI exit code the same failure would produce: 1 user, 2 internal, 3 policy. */
    exit_code: 1 | 2 | 3;
    /** `cyberbrain_core::Error` variant name (`no-such-note`, `frontmatter`, `bad-ring`, `io`, …) when the error came from core. */
    variant?: string;
    /** Present only for code "pii-held". */
    hold?: PiiHold;
    /** Present only for code "ring-cap-exceeded". `used` is measured from the store, `would_be` from the refused write. */
    cap?: { tokens: number; used: number; would_be: number };
    /** Present only for code "write-conflict": the `updated` the note carries now. */
    current_updated?: string;
  };
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly body: ApiErrorBody["error"],
  ) {
    super(body.message);
    this.name = "ApiError";
  }
}

// ───────────────────────────────────────────────────────────────────────────── the client

/** Every call the UI makes. Implemented twice: `http.ts` and `mock/index.ts`. UI-owned. */
/** Totals for one retrieval op, over the rows still in the ledger. */
export interface UsageTotals {
  ops: number;
  /** What the caller was handed. */
  returned: number;
  /** What the notes or files behind those hits hold in full. */
  full: number;
  hits: number;
}

export interface UsageSummary {
  /** Oldest row still on disk. A share is never all-time unless this says so. */
  since: string | null;
  rows: number;
  /** Counted in tokens: the index knows an exact count per block. */
  recall: UsageTotals;
  /** Counted in lines: that is the unit the agent is told to read. */
  find: UsageTotals;
  unreadable_rows: number;
}

export interface TaskUsage {
  calls: number;
  failed: number;
  prompt_tokens: number;
  /** Part of `prompt_tokens` the server answered from its prompt cache. */
  cached_prompt_tokens: number;
  completion_tokens: number;
  elapsed_ms: number;
  /** Calls the endpoint reported no counts for at all. */
  calls_without_counts: number;
  /** Calls that reported counts but said nothing about cache hits. */
  calls_without_cache_report: number;
}

export interface InferenceUsage {
  tasks: Record<string, TaskUsage>;
  first: string | null;
  last: string | null;
}

/** One measured model call. Fields that could not be measured are null, never zero. */
export interface LoadRow {
  at: string;
  task: string;
  wall_ms: number;
  /** Cores busy machine-wide during the call, everything else on the box included. */
  machine_cores: number | null;
  machine_mem_delta_mb: number | null;
  /** Cores the configured cgroup burned: exact attribution, present only when configured. */
  endpoint_cores: number | null;
  endpoint_mem_bytes: number | null;
  endpoint_mem_peak_bytes: number | null;
}

export interface LoadSummary {
  calls: number;
  wall_ms: number;
  machine_cores_avg: number | null;
  endpoint_cores_avg: number | null;
  last: LoadRow | null;
  calls_without_attribution: number;
  /** Cores this machine has, so a core count reads as a share. */
  cores_total: number | null;
}

/** Vendor-reported, so every field is optional. Only Ollama answers this today. */
export interface LoadedModel {
  name: string;
  size: number | null;
  /** Of `size`, the part in video memory. Zero on a CPU-only host. */
  size_vram: number | null;
  context_length: number | null;
  parameter_size: string | null;
  quantization_level: string | null;
  expires_at: string | null;
}

/** One UTC calendar day. Days where nothing happened are present and zero: a gap and a zero
 *  are different facts and the axis has to show which one it is. */
export interface DayBucket {
  /** `YYYY-MM-DD`, UTC. */
  date: string;
  recall: UsageTotals;
  find: UsageTotals;
  calls: number;
  prompt_tokens: number;
  cached_prompt_tokens: number;
  completion_tokens: number;
  wall_ms: number;
  endpoint_cores: number | null;
  machine_cores: number | null;
}

export interface UsageReport {
  retrieval: UsageSummary;
  inference: InferenceUsage;
  load: LoadSummary;
  loaded_models: LoadedModel[] | null;
  /** Oldest first, one per day. */
  days: DayBucket[];
}

/**
 * Whether this machine reports its audit trail to a company hub.
 *
 * `GET /api/v1/hub`. Small on purpose: the dashboard asks on every load, and the question
 * the person has ("does anything leave this machine, and when did it last") is not worth
 * walking the whole store for.
 */
export interface HubStatus {
  enrolled: boolean;
  /** Base URL of the hub. Present when enrolled. */
  hub?: string;
  /** The id this store believes it is. The token is what authenticates, not this. */
  device?: string | null;
  /** RFC 3339, or null when nothing has been delivered yet. */
  last_delivery?: string | null;
}

/**
 * `POST /api/v1/command` request. Mirrors `serve::command::CommandRequest`.
 *
 * The line as typed, without the leading `cyberbrain`. The server splits it (quotes and
 * backslash escapes, nothing else — there is no shell), checks it, and runs the same binary
 * that is serving this page against this store.
 */
export interface CommandRequest {
  line: string;
}

/** `POST /api/v1/command` response. Mirrors `serve::command::CommandResult`. */
export interface CommandResult {
  /** The line after splitting, so a person can see how their quotes were read. */
  argv: string[];
  stdout: string;
  stderr: string;
  /** The command's own exit code, not the request's: 0 success, 1 user error, 2 internal, 3 policy refusal. A non-zero code arrives with HTTP 200. */
  exit_code: number;
  /** Output was cut at 256 KB. */
  truncated: boolean;
}

export interface CyberbrainApi {
  /** "mock" or "http"; shown in the UI so fabricated data is never mistaken for real. */
  readonly transport: "mock" | "http";

  status(): Promise<StatusReport>;
  hubStatus(): Promise<HubStatus>;
  /** `days` of history for the daily buckets, 1 to 365; the server defaults to 30. */
  usage(days?: number): Promise<UsageReport>;
  recall(params: RecallParams): Promise<RecallResult>;
  expand(citation: Citation): Promise<CitationExpansion>;

  listNotes(params?: NoteListParams): Promise<NoteSummary[]>;
  getNote(nameOrId: string): Promise<NoteDetail>;
  writeNote(nameOrId: string, req: NoteWriteRequest): Promise<NoteDetail>;
  resolveHold(holdId: string, res: PiiHoldResolution): Promise<NoteDetail | null>;
  forget(nameOrId: string, dryRun: boolean): Promise<ForgetReport>;

  graph(): Promise<Graph>;

  egress(): Promise<EgressRegister>;
  obligations(): Promise<ObligationsView>;
  audit(params?: AuditParams): Promise<AuditPage>;
  pii(): Promise<PiiReport>;
  retention(): Promise<RetentionQueue>;
  /** `names` is accepted by the signature for parity with the server, which answers it with 400 (see `RetentionQueue`). The UI never passes it. */
  applyRetention(dryRun: boolean, names?: string[]): Promise<RetentionApplyReport>;
  modelCards(): Promise<ModelCard[]>;
  subjectAccess(identifier: string): Promise<SubjectAccessReport>;

  doctor(): Promise<DoctorReport>;
  scan(full: boolean): Promise<ScanReport>;
  /** Run a command line against this store. A command that fails resolves; only a refused or malformed line rejects. */
  command(line: string): Promise<CommandResult>;
}
