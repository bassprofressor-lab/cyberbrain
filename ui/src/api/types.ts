/**
 * Cyberbrain HTTP API contract — the single source of truth for what the UI expects.
 *
 * The backend does not exist yet; the Rust side is to be built against THIS file. Field
 * names follow the serde shapes already fixed in `crates/cyberbrain-core/src/types.rs`
 * (`Ring` serialises as a bare integer, `NoteKind`/`PiiState` lowercase, `EgressPurpose`
 * kebab-case, `RecallResult { hits, conflicts, caveats }`). Everything else is proposed
 * here and marked where it goes beyond the core types.
 *
 * Conventions
 * ───────────
 * • Base path `/api/v1`, same origin as the page (`cyberbrain serve`, 127.0.0.1:7777).
 * • JSON in, JSON out, `Content-Type: application/json`. Timestamps are RFC 3339 UTC strings
 *   (what `jiff::Timestamp` serialises to). Durations are ISO-8601 (`P2Y`, `P90D`).
 * • Notes are addressed by `name` (slug) OR `id` (ULID) in the path; the server tries the
 *   ULID form first (26 Crockford-base32 chars), else treats it as a name.
 * • Errors: any non-2xx carries `ApiErrorBody`. HTTP status mirrors the CLI exit codes
 *   (SPEC §8): 400 → user error (1), 500 → internal (2), 403 → policy refusal (3).
 *   404 for an unknown note/citation, 409 for a write conflict or a PII hold (§12.4).
 * • Every mutating route accepts `?dry_run=true` and then runs the real code path with a
 *   no-op writer (SPEC §8 / §14.6), returning the same shape it would have returned.
 * • The server never redirects and never sets a cookie; there is no auth — the socket is
 *   loopback-only. If that changes, the UI needs an `Authorization` seam in `client.ts`.
 */

// ───────────────────────────────────────────────────────────────────────────── core types

/** Trust tier, SPEC §3.2. Serialised as 0..4. */
export type Ring = 0 | 1 | 2 | 3 | 4;

export type NoteKind = "knowledge" | "bug" | "lesson" | "decision" | "reference" | "session";

export type PiiState = "none" | "reviewed" | "flagged";

/** ULID, 26 chars. */
export type NoteId = string;

/** `r{ring}-{10 hex}`, SPEC §3.3. */
export type Citation = string;

/** The YAML head of a note, SPEC §3.1. Mirrors `Frontmatter` in core. */
export interface Frontmatter {
  id: NoteId;
  name: string;
  ring: Ring;
  kind: NoteKind;
  created: string;
  updated: string;
  tags: string[];
  /** Derived from `[[...]]` in the body, written back by scan. */
  links: string[];
  /** ISO-8601 duration; absent means indefinite. */
  retention?: string;
  pii: PiiState;
}

// ───────────────────────────────────────────────────────────────────────────── /status

export type InferenceBackend = "ollama" | "lm-studio" | "nvidia-pair" | "unknown";

export type EndpointClass = "loopback" | "private" | "public" | "unresolved";

export interface RingCount {
  ring: Ring;
  notes: number;
  blocks: number;
  tokens: number;
  bytes: number;
}

/**
 * GET /api/v1/status → 200 StatusReport
 * Same content as `cyberbrain status --json`.
 */
export interface StatusReport {
  version: string;
  store: {
    path: string;
    /** Sum of `notes/` and `cyberbrain.db` in bytes. */
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
    last_scan: string | null;
    last_full_scan: string | null;
    /** Notes whose (mtime, hash) differ from the index right now. */
    stale_notes: number;
    /** Vectors whose block no longer exists. Doctor should say 0. */
    orphan_vectors: number;
    dangling_links: number;
    fts_ok: boolean;
  };
  embedding: {
    /** e.g. "model2vec/potion-base-8M@d256/mean-l2" */
    profile_id: string;
    model: string;
    dim: number;
    pooling: string;
    backend: "static" | "candle";
    /** Whether `meta.profile` equals the configured model. False disables semantic search. */
    matches_index: boolean;
    model_hash: string;
    model_verified_at: string | null;
  };
  inference: {
    configured: boolean;
    base_url: string;
    endpoint_class: EndpointClass;
    allow_public_endpoint: boolean;
    model: string | null;
    /** Which server answered the most recent call, by response fingerprint. */
    last_backend: InferenceBackend;
    last_backend_evidence: string | null;
    last_call: string | null;
    /** Result of the most recent reachability probe, never performed by the UI. */
    reachable: boolean | null;
    reachable_checked_at: string | null;
  };
  policy: {
    profile: PolicyProfile;
    pii_scan: boolean;
    audit_rows: number;
  };
}

// ───────────────────────────────────────────────────────────────────────────── /recall

export interface RecallParams {
  q: string;
  /** Default 8. */
  n?: number;
  /** Restrict to one ring. */
  ring?: Ring;
}

/** Mirrors `Hit` in core. */
export interface Hit {
  citation: Citation;
  note_id: NoteId;
  note_name: string;
  ring: Ring;
  /** Fused, ring-weighted score (SPEC §7 step 4). Not normalised; compare within a result. */
  score: number;
  text: string;
  // ── proposed additions beyond core::Hit ──
  /** Index of the block inside the note; lets the UI deep-link to the block. */
  block_idx: number;
  /** Which candidate lists the block came from. Absent lists are the honest answer to "why is this here". */
  sources: Array<"lexical" | "semantic">;
}

/** Mirrors `Conflict` in core. */
export interface Conflict {
  winner: Citation;
  loser: Citation;
  reason: string;
}

/**
 * GET /api/v1/recall?q=…&n=8&ring=2 → 200 RecallResult
 * 400 if `q` is empty or `n` > 100. Mirrors `RecallResult` in core plus timing/mode.
 */
export interface RecallResult {
  hits: Hit[];
  conflicts: Conflict[];
  /** Checks that were skipped and why. Rendered verbatim; never empty out of politeness. */
  caveats: string[];
  // ── proposed additions ──
  /** "hybrid" normally; "lexical" when the embedding profile mismatches the index (§5). */
  mode: "hybrid" | "lexical";
  elapsed_ms: number;
  /** Echo of the effective parameters. */
  params: { q: string; n: number; ring: Ring | null; k_lex: number; k_sem: number };
}

/**
 * GET /api/v1/recall/{citation} → 200 CitationExpansion | 404
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

export interface NoteSummary {
  id: NoteId;
  name: string;
  ring: Ring;
  kind: NoteKind;
  tags: string[];
  updated: string;
  created: string;
  retention?: string;
  pii: PiiState;
  blocks: number;
  bytes: number;
  links_out: number;
  links_in: number;
  /** Outbound `[[links]]` with no target note. */
  dangling: number;
}

export interface NoteListParams {
  ring?: Ring;
  kind?: NoteKind;
  /** Substring match on name and tags; not a recall. */
  q?: string;
  sort?: "updated" | "name" | "ring";
}

/** GET /api/v1/notes?ring=&kind=&q=&sort= → 200 NoteSummary[] */

export interface OutboundLink {
  /** The literal text inside `[[…]]`. */
  target: string;
  /** Null when dangling: intent, not error (SPEC §3.1). */
  resolved: { id: NoteId; ring: Ring } | null;
}

export interface InboundLink {
  from: { id: NoteId; name: string; ring: Ring };
}

export interface BlockRef {
  citation: Citation;
  idx: number;
  token_count: number;
  /** First line of the block, for the citation list. */
  preview: string;
}

/**
 * GET /api/v1/notes/{name|id} → 200 NoteDetail | 404
 */
export interface NoteDetail {
  front: Frontmatter;
  body: string;
  /** Path relative to the store root, e.g. `notes/r2/pg18-moves-pgdata.md`. */
  path: string;
  outbound: OutboundLink[];
  inbound: InboundLink[];
  blocks: BlockRef[];
}

export interface NoteWriteRequest {
  body: string;
  /** Optional frontmatter edits; `id`, `created`, `links` are server-owned and rejected with 400. */
  front?: Partial<Pick<Frontmatter, "name" | "ring" | "kind" | "tags" | "retention">>;
  /** Optimistic concurrency: the `updated` you last saw. 409 `write-conflict` if it moved. */
  expected_updated: string;
}

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
 *   200 NoteDetail            written (audit row `write`)
 *   409 ApiErrorBody          code "write-conflict" (updated moved) or
 *                             code "pii-held" with `hold: PiiHold` (SPEC §12.4, eu/ch only)
 *   400 ApiErrorBody          code "ring-cap-exceeded" (SPEC §3.2), "bad-frontmatter"
 *   403 ApiErrorBody          code "policy-refusal"
 *
 * POST /api/v1/notes  body NoteWriteRequest & { front: { name, ring, kind } } → 201 NoteDetail
 *   same error cases; `expected_updated` ignored.
 */
export interface PiiHold {
  hold_id: string;
  note: string;
  findings: PiiFinding[];
  /** The hold expires; after this the client must resubmit. */
  expires_at: string;
}

/**
 * POST /api/v1/holds/{hold_id}  body PiiHoldResolution
 *   200 NoteDetail    written according to `action`; `pii` becomes "none" (redact),
 *                     "reviewed" (mark-reviewed) or "flagged" (proceed)
 *   204               action "discard": nothing written
 *   404               unknown or expired hold
 */
export interface PiiHoldResolution {
  action: "redact" | "mark-reviewed" | "proceed" | "discard";
}

/**
 * DELETE /api/v1/notes/{name|id}?dry_run= → 200 ForgetReport
 * Same path as `cyberbrain forget` (SPEC §12.2).
 */
export interface ForgetReport {
  dry_run: boolean;
  note: { id: NoteId; name: string; path: string };
  removed: { file: boolean; blocks: number; vectors: number; fts_rows: number; links_in: number; links_out: number; derivatives: number };
}

// ───────────────────────────────────────────────────────────────────────────── /graph

export interface GraphNode {
  id: NoteId;
  name: string;
  ring: Ring;
  kind: NoteKind;
  links_in: number;
  links_out: number;
}

export interface GraphEdge {
  from: NoteId;
  to: NoteId;
}

/** A `[[link]]` with no target: rendered as intent, never hidden (SPEC §13.3). */
export interface DanglingLink {
  from: NoteId;
  to_name: string;
}

/** GET /api/v1/graph → 200 Graph. Whole store; the UI filters client-side. */
export interface Graph {
  nodes: GraphNode[];
  edges: GraphEdge[];
  dangling: DanglingLink[];
}

// ───────────────────────────────────────────────────────────────────────────── /policy

export type PolicyProfile = "eu" | "ch" | "off";

/** Mirrors `EgressPurpose` in core (kebab-case). The list is closed by construction. */
export type EgressPurpose = "model-download" | "local-inference";

export interface EgressPath {
  purpose: EgressPurpose;
  description: string;
  /** Where bytes go. For inference this is the configured base URL; for downloads the artefact origin. */
  destination: string;
  destination_class: EndpointClass;
  /** What is sent. Plain language, from the register, not inferred. */
  data: string;
  permitted_by: PolicyProfile[];
  /** Live state: configured AND permitted by the active profile AND not disabled. */
  enabled: boolean;
  /** Why it is disabled, when it is. Null when enabled. */
  disabled_reason: string | null;
  last_used: string | null;
  /** Counts from the audit log, all time since store init. */
  uses_total: number;
  bytes_out_total: number;
}

/**
 * GET /api/v1/policy/egress → 200 EgressRegister
 * Same as `cyberbrain policy egress --json`.
 */
export interface EgressRegister {
  profile: PolicyProfile;
  /** Store init time; the "since" of every count on this screen. */
  since: string;
  paths: EgressPath[];
  /** Attempts refused by the wrapper (e.g. public endpoint without allow flag). */
  refused_total: number;
  /** Built-in statement the register can prove: purposes not on this list do not exist in the binary. */
  register_hash: string;
}

export type AuditAction =
  | "write"
  | "erase"
  | "scan"
  | "model-download"
  | "inference"
  | "policy-refusal"
  | "egress-attempt"
  | "egress-refused"
  | "retention-apply"
  | "hold-resolved"
  | "subject-access";

export interface AuditRow {
  /** Monotonic row id; the cursor. */
  seq: number;
  ts: string;
  /** "operator", "agent:<harness>", "hook:<event>", "mcp", "ui", "cli" */
  actor: string;
  action: AuditAction;
  /** Note name, citation, purpose, or endpoint. */
  subject: string;
  /** Free-form JSON-ish detail (tokens in/out, bytes, model, reason). Rendered as key: value. */
  detail: Record<string, string | number | boolean | null>;
}

export interface AuditParams {
  /** Default 100, max 1000. */
  limit?: number;
  /** Return rows with seq < before. */
  before?: number;
  action?: AuditAction;
  actor?: string;
  q?: string;
}

/**
 * GET /api/v1/policy/audit?limit=&before=&action=&actor=&q= → 200 AuditPage
 * Rows newest first. `cyberbrain policy audit` produces the same rows.
 */
export interface AuditPage {
  rows: AuditRow[];
  total: number;
  next_before: number | null;
}

export interface PiiReportEntry {
  note: { id: NoteId; name: string; ring: Ring };
  state: PiiState;
  findings: PiiFinding[];
  /** When the state was last set; null if never scanned (profile off at write time). */
  reviewed_at: string | null;
}

/**
 * GET /api/v1/policy/pii → 200 PiiReport
 * Every note whose `pii` is not "none", plus pending holds.
 */
export interface PiiReport {
  scan_enabled: boolean;
  entries: PiiReportEntry[];
  holds: PiiHold[];
}

export interface RetentionEntry {
  note: { id: NoteId; name: string; ring: Ring };
  retention: string;
  /** created + retention. */
  expires_at: string;
  due: boolean;
}

/**
 * GET /api/v1/policy/retention → 200 RetentionQueue
 * POST /api/v1/policy/retention/apply?dry_run=true body { names?: string[] } → 200 RetentionApplyReport
 *   Erases every due note (or only `names`) through the forget path (SPEC §12.5).
 */
export interface RetentionQueue {
  entries: RetentionEntry[];
  due: number;
}

export interface RetentionApplyReport {
  dry_run: boolean;
  removed: ForgetReport[];
  skipped: Array<{ name: string; reason: string }>;
}

export interface ModelCard {
  role: "embedding" | "inference";
  name: string;
  /** Where the artefact came from, e.g. the download URL or "configured endpoint (not a file)". */
  source: string;
  license: string;
  /** blake3 of the artefact; null for an endpoint model we do not hold. */
  hash: string | null;
  format: string | null;
  dim: number | null;
  pooling: string | null;
  bytes: number | null;
  intended_use: string;
  limitations: string;
  /** For artefacts: when the hash was last verified on load. */
  verified_at: string | null;
  /** True when this model is what the store currently uses. */
  active: boolean;
}

/** GET /api/v1/policy/model-card → 200 ModelCard[] */

export interface SubjectAccessHit {
  where: "note" | "block" | "audit";
  ref: string;
  /** Citation for blocks/notes; audit seq for rows. */
  citation: Citation | null;
  excerpt: string;
}

/**
 * GET /api/v1/policy/subject?q=<identifier> → 200 SubjectAccessReport
 * SPEC §12.3. Writes an audit row `subject-access`.
 */
export interface SubjectAccessReport {
  identifier: string;
  hits: SubjectAccessHit[];
  searched: { notes: number; blocks: number; audit_rows: number };
}

// ───────────────────────────────────────────────────────────────────────────── /doctor, /scan

export interface DoctorReport {
  ok: boolean;
  checked_at: string;
  findings: Array<{
    severity: "info" | "warn" | "error";
    check: "dangling-link" | "ring-cap" | "stale-index" | "orphan-vector" | "profile-mismatch" | "duplicate-name";
    subject: string;
    message: string;
  }>;
}

/** GET /api/v1/doctor → 200 DoctorReport (runs the checks; read-only) */

export interface ScanReport {
  full: boolean;
  elapsed_ms: number;
  scanned: number;
  changed: number;
  added: number;
  removed: number;
  blocks_written: number;
  vectors_written: number;
}

/** POST /api/v1/scan?full=false → 200 ScanReport */

// ───────────────────────────────────────────────────────────────────────────── errors

export type ApiErrorCode =
  | "bad-request"
  | "not-found"
  | "write-conflict"
  | "pii-held"
  | "ring-cap-exceeded"
  | "bad-frontmatter"
  | "policy-refusal"
  | "profile-mismatch"
  | "internal";

export interface ApiErrorBody {
  error: {
    code: ApiErrorCode;
    message: string;
    /** CLI exit code the same failure would produce: 1 user, 2 internal, 3 policy. */
    exit_code: 1 | 2 | 3;
    /** Present only for code "pii-held". */
    hold?: PiiHold;
    /** Present only for code "ring-cap-exceeded". */
    cap?: { tokens: number; used: number; would_be: number };
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

/** Every call the UI makes. Implemented twice: `http.ts` and `mock/index.ts`. */
export interface CyberbrainApi {
  /** "mock" or "http"; shown in the UI so fabricated data is never mistaken for real. */
  readonly transport: "mock" | "http";

  status(): Promise<StatusReport>;
  recall(params: RecallParams): Promise<RecallResult>;
  expand(citation: Citation): Promise<CitationExpansion>;

  listNotes(params?: NoteListParams): Promise<NoteSummary[]>;
  getNote(nameOrId: string): Promise<NoteDetail>;
  writeNote(nameOrId: string, req: NoteWriteRequest): Promise<NoteDetail>;
  resolveHold(holdId: string, res: PiiHoldResolution): Promise<NoteDetail | null>;
  forget(nameOrId: string, dryRun: boolean): Promise<ForgetReport>;

  graph(): Promise<Graph>;

  egress(): Promise<EgressRegister>;
  audit(params?: AuditParams): Promise<AuditPage>;
  pii(): Promise<PiiReport>;
  retention(): Promise<RetentionQueue>;
  applyRetention(dryRun: boolean, names?: string[]): Promise<RetentionApplyReport>;
  modelCards(): Promise<ModelCard[]>;
  subjectAccess(identifier: string): Promise<SubjectAccessReport>;

  doctor(): Promise<DoctorReport>;
  scan(full: boolean): Promise<ScanReport>;
}
