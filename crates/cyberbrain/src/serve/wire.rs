//! The shapes on the wire. Every struct here mirrors an interface in `ui/src/api/types.ts`,
//! which SPEC §8.1 names as the single source of truth for the HTTP API. Field names and
//! nesting follow that file; where the Rust side has *more* to say, the extra fields are
//! additive (TypeScript ignores members it does not declare) and are marked as such.
//!
//! Where the two sides could not be made to agree, the divergence is noted next to the
//! field and in the module report, never papered over with an invented value.

use cyberbrain_core::{NoteId, NoteKind, PiiState, Ring};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------
// core-ish

/// `Frontmatter` in types.ts is core's `Frontmatter` serialised as-is; core's `pii` also
/// carries `unscanned`, which types.ts does not list (see the report).
pub type Frontmatter = cyberbrain_core::Frontmatter;

// ---------------------------------------------------------------------------------------
// /status

#[derive(Debug, Clone, Serialize)]
pub struct RingCount {
    pub ring: Ring,
    pub notes: usize,
    pub blocks: usize,
    pub tokens: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResidentCap {
    pub tokens: usize,
    pub used: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoreStatus {
    pub path: String,
    pub bytes: u64,
    pub notes_bytes: u64,
    pub db_bytes: u64,
    pub models_bytes: u64,
    pub notes: usize,
    pub blocks: usize,
    pub vectors: usize,
    pub rings: Vec<RingCount>,
    pub resident_cap: ResidentCap,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexStatus {
    pub schema_version: u32,
    pub last_scan: Option<jiff::Timestamp>,
    pub last_full_scan: Option<jiff::Timestamp>,
    pub stale_notes: usize,
    pub orphan_vectors: usize,
    pub dangling_links: usize,
    pub fts_ok: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingStatus {
    pub profile_id: String,
    pub model: String,
    pub dim: usize,
    pub pooling: String,
    pub backend: &'static str,
    /// `null` when there is nothing to compare: no model loaded, or no vectors stored.
    ///
    /// It was a plain `bool` that reported `true` in that case with the truth pushed into
    /// a caveat. A boolean that means "yes or unknown" is read as "yes" by everything that
    /// does not also read the caveats, which is every client that only wants a green dot.
    pub matches_index: Option<bool>,
    pub model_hash: String,
    pub model_verified_at: Option<jiff::Timestamp>,
    /// Additive: `true` only when a model artefact is loaded. types.ts has no field for
    /// "no model at all", and every other member above is meaningless without one.
    pub loaded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct InferenceStatus {
    pub configured: bool,
    pub base_url: String,
    pub endpoint_class: &'static str,
    pub allow_public_endpoint: bool,
    pub model: Option<String>,
    pub last_backend: &'static str,
    pub last_backend_evidence: Option<String>,
    pub last_call: Option<jiff::Timestamp>,
    pub reachable: Option<bool>,
    pub reachable_checked_at: Option<jiff::Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PolicyStatus {
    pub profile: cyberbrain_core::PolicyProfile,
    pub pii_scan: bool,
    pub audit_rows: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    pub version: &'static str,
    pub store: StoreStatus,
    pub index: IndexStatus,
    pub embedding: EmbeddingStatus,
    pub inference: InferenceStatus,
    pub policy: PolicyStatus,
    /// Additive: which members above are not measured by this build, and why. A number
    /// that is not measured is still shown, so this says which ones to read with care.
    pub caveats: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// /recall

#[derive(Debug, Clone, Deserialize)]
pub struct RecallParams {
    pub q: Option<String>,
    pub n: Option<usize>,
    pub ring: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub citation: String,
    pub note_id: NoteId,
    pub note_name: String,
    pub ring: Ring,
    pub score: f32,
    pub text: String,
    pub block_idx: u32,
    pub sources: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecallEcho {
    pub q: String,
    pub n: usize,
    pub ring: Option<Ring>,
    pub k_lex: usize,
    pub k_sem: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecallResult {
    pub hits: Vec<Hit>,
    pub conflicts: Vec<cyberbrain_core::Conflict>,
    pub caveats: Vec<String>,
    pub mode: &'static str,
    pub elapsed_ms: f64,
    pub params: RecallEcho,
}

#[derive(Debug, Clone, Serialize)]
pub struct CitationExpansion {
    pub citation: String,
    pub ring: Ring,
    pub block_idx: u32,
    pub block_text: String,
    pub token_count: u32,
    pub note: NoteDetail,
}

// ---------------------------------------------------------------------------------------
// /notes

#[derive(Debug, Clone, Serialize)]
pub struct NoteSummary {
    pub id: NoteId,
    pub name: String,
    pub ring: Ring,
    pub kind: NoteKind,
    pub tags: Vec<String>,
    pub updated: jiff::Timestamp,
    pub created: jiff::Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention: Option<String>,
    pub pii: PiiState,
    pub blocks: usize,
    pub bytes: u64,
    pub links_out: usize,
    pub links_in: usize,
    pub dangling: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NoteListParams {
    pub ring: Option<u8>,
    pub kind: Option<NoteKind>,
    pub q: Option<String>,
    pub sort: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedTarget {
    pub id: NoteId,
    pub ring: Ring,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutboundLink {
    pub target: String,
    pub resolved: Option<ResolvedTarget>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboundFrom {
    pub id: NoteId,
    pub name: String,
    pub ring: Ring,
}

#[derive(Debug, Clone, Serialize)]
pub struct InboundLink {
    pub from: InboundFrom,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockRef {
    pub citation: String,
    pub idx: u32,
    pub token_count: u32,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteDetail {
    pub front: Frontmatter,
    pub body: String,
    pub path: String,
    pub outbound: Vec<OutboundLink>,
    pub inbound: Vec<InboundLink>,
    pub blocks: Vec<BlockRef>,
    /// Additive: `true` when the detail describes what a dry run *would* have written and
    /// the file on disk is unchanged.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PiiFinding {
    pub kind: &'static str,
    pub excerpt: String,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PiiHold {
    pub hold_id: String,
    pub note: String,
    pub findings: Vec<PiiFinding>,
    pub expires_at: jiff::Timestamp,
    /// Additive: the hold came from a `?dry_run=true` write and resolving it writes nothing.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PiiHoldResolution {
    pub action: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgetNote {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgetRemoved {
    pub file: bool,
    pub blocks: usize,
    pub vectors: usize,
    pub fts_rows: usize,
    pub links_in: usize,
    pub links_out: usize,
    pub derivatives: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForgetReport {
    pub dry_run: bool,
    pub note: ForgetNote,
    pub removed: ForgetRemoved,
    /// Additive: what the eraser could not confirm, in words (from `ErasureReport::notes`).
    pub notes: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// /graph

#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub id: NoteId,
    pub name: String,
    pub ring: Ring,
    pub kind: NoteKind,
    pub links_in: usize,
    pub links_out: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    pub from: NoteId,
    pub to: NoteId,
}

#[derive(Debug, Clone, Serialize)]
pub struct DanglingLink {
    pub from: NoteId,
    pub to_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Graph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub dangling: Vec<DanglingLink>,
}

// ---------------------------------------------------------------------------------------
// /policy

#[derive(Debug, Clone, Serialize)]
pub struct EgressPath {
    pub purpose: cyberbrain_core::EgressPurpose,
    pub description: &'static str,
    pub destination: String,
    pub destination_class: &'static str,
    pub data: &'static str,
    pub permitted_by: Vec<cyberbrain_core::PolicyProfile>,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
    pub last_used: Option<jiff::Timestamp>,
    pub uses_total: usize,
    pub bytes_out_total: u64,
    /// Additive: the register's own wording of the enable state, also when enabled.
    pub state: String,
    /// Additive: whether note content can be in a request on this path.
    pub carries_note_content: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct EgressRegister {
    pub profile: cyberbrain_core::PolicyProfile,
    pub since: jiff::Timestamp,
    pub paths: Vec<EgressPath>,
    pub refused_total: usize,
    pub register_hash: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditRow {
    pub seq: usize,
    pub ts: jiff::Timestamp,
    pub actor: String,
    pub action: String,
    pub subject: String,
    pub detail: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuditParams {
    pub limit: Option<usize>,
    pub before: Option<usize>,
    pub action: Option<String>,
    pub actor: Option<String>,
    pub q: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditPage {
    pub rows: Vec<AuditRow>,
    pub total: usize,
    pub next_before: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteRef {
    pub id: NoteId,
    pub name: String,
    pub ring: Ring,
}

#[derive(Debug, Clone, Serialize)]
pub struct PiiReportEntry {
    pub note: NoteRef,
    pub state: PiiState,
    pub findings: Vec<PiiFinding>,
    pub reviewed_at: Option<jiff::Timestamp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PiiReport {
    pub scan_enabled: bool,
    pub entries: Vec<PiiReportEntry>,
    pub holds: Vec<PiiHold>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionEntry {
    pub note: NoteRef,
    pub retention: String,
    pub expires_at: jiff::Timestamp,
    pub due: bool,
    /// Additive: set when the duration does not parse. types.ts has no state for that, and
    /// a note the operator meant to expire that never will is exactly what must be shown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invalid: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionQueue {
    pub entries: Vec<RetentionEntry>,
    pub due: usize,
    /// Additive: notes without a `retention` field, the denominator (SPEC §14.4).
    pub indefinite: usize,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RetentionApplyRequest {
    #[serde(default)]
    pub names: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionApplyReport {
    pub dry_run: bool,
    pub removed: Vec<ForgetReport>,
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCard {
    pub role: cyberbrain_policy::ModelRole,
    pub name: String,
    pub source: String,
    pub license: String,
    pub hash: Option<String>,
    pub format: Option<String>,
    pub dim: Option<usize>,
    pub pooling: Option<String>,
    pub bytes: Option<u64>,
    pub intended_use: String,
    pub limitations: String,
    pub verified_at: Option<jiff::Timestamp>,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubjectAccessHit {
    pub r#where: &'static str,
    pub r#ref: String,
    pub citation: Option<String>,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Searched {
    pub notes: usize,
    pub blocks: usize,
    pub audit_rows: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubjectAccessReport {
    pub identifier: String,
    pub hits: Vec<SubjectAccessHit>,
    pub searched: Searched,
    /// Additive: the policy crate's own caveats and legal framing for the report.
    pub caveats: Vec<String>,
    pub response_deadline: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubjectParams {
    pub q: Option<String>,
}

// ---------------------------------------------------------------------------------------
// /doctor, /scan

#[derive(Debug, Clone, Serialize)]
pub struct DoctorFinding {
    pub severity: &'static str,
    pub check: String,
    pub subject: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub checked_at: jiff::Timestamp,
    pub findings: Vec<DoctorFinding>,
    /// Additive: which checks ran, so an empty findings list has a population.
    pub checks_run: Vec<&'static str>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScanParams {
    #[serde(default)]
    pub full: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub full: bool,
    pub elapsed_ms: u128,
    pub scanned: usize,
    pub changed: usize,
    pub added: usize,
    pub removed: usize,
    pub blocks_written: usize,
    pub vectors_written: usize,
    /// Additive: `true` for `?dry_run=true`.
    pub dry_run: bool,
    /// Additive: the real report, every counter naming its side of the boundary.
    pub detail: crate::app::ScanReport,
    pub caveats: Vec<String>,
}

// ---------------------------------------------------------------------------------------
// shared query shapes

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct DryRun {
    #[serde(default)]
    pub dry_run: Option<bool>,
}

impl DryRun {
    pub fn is_on(self) -> bool {
        self.dry_run.unwrap_or(false)
    }
}
