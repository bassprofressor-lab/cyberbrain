//! The composition (SPEC §8.2). This is the only place that knows about all six crates and
//! the only place a seam is tied. Commands, hooks, MCP and `serve` are thin adapters over
//! [`App`]; none of them may reach around it.
//!
//! Startup order, and why (SPEC §8.2):
//!
//! 1. **Config** — everything below is parameterised by it.
//! 2. **`AuditStore`** (`audit.db`) — opens before anything that could produce a record.
//! 3. **`AuditLog`** (policy) over that store, through [`StoreAuditSink`]; the only writer.
//! 4. **`Egress`** from config and log, handed out as `Arc<dyn EgressGate>`.
//! 5. **`Index`** (`cyberbrain.db`), migrating if needed.
//! 6. **Embedder**, lazily: only when a command needs vectors, never on a hot-path hook.
//! 7. **`LlmClient`**, lazily and last; absence is a caveat, not an error.
//!
//! Steps 3 and 4 both happen inside `Policy::new`, which is why it is constructed exactly
//! once here and handed the sink from step 2.

use crate::audit_bridge::{AUDIT_DB_FILE, StoreAuditSink};
use crate::hostload;
use crate::usage;
use crate::writers::{
    FsNoteWriter, IndexWriter, NoopIndexWriter, NoopNoteWriter, NoteWriter, SqliteIndexWriter,
    StoreEraser, lock_index,
};
use cyberbrain_core::Slash;
use cyberbrain_core::blocks::{MAX_BLOCK_TOKENS, OversizedReason, blocks_of};
use cyberbrain_core::config::DEFAULT_STORE_DIR;
use cyberbrain_core::frontmatter;
use cyberbrain_core::links::link_targets;
use cyberbrain_core::store::{DB_FILE, NOTES_DIR, write_atomic};
use cyberbrain_core::{
    Citation, Config, EgressGate, Embedder, Error, Frontmatter, Note, NoteId, NoteKind, PiiState,
    RecallResult, Result, Ring, Store,
};
use cyberbrain_embed::{ArtefactManifest, ModelPaths, StaticEmbedder};
use cyberbrain_index::{
    AuditStore, EmbeddingProfile, Erased, Index, IndexStats, NoteStamp, ProfileChange,
    RecallOptions, content_hash,
};
use cyberbrain_llm::{LlmClient, LlmConfig, Probe};
use cyberbrain_policy::profile::ProfileExt;
use cyberbrain_policy::{
    Actor, AuditAction, AuditFilter, EgressEntry, EraseReason, EraseRequest, ErasureReport,
    ExportFormat, Finding, Identifier, MemoryAuditSink, ModelCard, ModelInventory, ModelRole,
    OperatorChoice, Policy, PolicyConfig, PolicyStatus, RetentionItem, RetentionQueue,
    SubjectAccessReport, SubjectBlock, SubjectSource, WriteVerdict,
};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Name of the manifest the embedder is verified against, next to the two model files.
/// Core's config carries no digest fields (see the report), so the manifest lives with the
/// artefact: `<model_dir>/manifest.json` = `{"weights_blake3": ..., "tokenizer_blake3": ...}`.
pub const MANIFEST_FILE: &str = "manifest.json";

// ---------------------------------------------------------------------------------------
// Store discovery

fn is_store(p: &Path) -> bool {
    p.join(NOTES_DIR).is_dir()
}

/// `--store` / `CYBERBRAIN_STORE` (the CLI folds both into `explicit`), else `.cyberbrain`
/// walking up from the working directory. Never creates anything.
pub fn discover_store(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if is_store(p) {
            return Ok(p.to_path_buf());
        }
        return Err(Error::Config(format!(
            "{} is not a cyberbrain store (no notes/ directory inside it); create one with \
             `cyberbrain init --path {}`",
            Slash(p),
            Slash(p)
        )));
    }
    discover_store_from(None)
}

/// [`discover_store`], starting the walk somewhere other than the process's directory.
///
/// A hook is told which project the session is in, in its payload. The harness happens to
/// start the hook in that directory today, so both agree and the distinction is invisible.
/// It is not guaranteed to: a hook that resolves its store from the process's directory
/// would, the day that changes, quietly read and write the wrong project's memory. The
/// session's own statement of where it is wins.
pub fn discover_store_from(start: Option<&Path>) -> Result<PathBuf> {
    let cwd = match start {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().map_err(|e| Error::Io {
            path: PathBuf::from("."),
            source: e,
        })?,
    };
    for dir in cwd.ancestors() {
        let candidate = dir.join(DEFAULT_STORE_DIR);
        if is_store(&candidate) {
            return Ok(candidate);
        }
    }
    Err(Error::Config(format!(
        "no {DEFAULT_STORE_DIR} store found in {} or any directory above it; run \
         `cyberbrain init` in the project root, or point at one with --store or \
         CYBERBRAIN_STORE",
        Slash(&cwd)
    )))
}

// ---------------------------------------------------------------------------------------
// Report types. All `Serialize`, so `--json` and the HTTP API print the same numbers.

#[derive(Debug, Clone, Serialize)]
pub struct InitReport {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub store: PathBuf,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub config: PathBuf,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub audit_db: PathBuf,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub index_db: PathBuf,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedFile {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OversizedBlock {
    pub note: String,
    pub block_idx: u32,
    pub approx_tokens: u32,
    pub reason: &'static str,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EmbedderSummary {
    pub loaded: bool,
    pub profile_id: Option<String>,
    pub dim: Option<usize>,
    /// Why no model is loaded. Always present when `loaded` is false.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScanOptions {
    pub full: bool,
    pub dry_run: bool,
}

/// Every count names the side of the boundary it counts (SPEC §14.3).
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub dry_run: bool,
    pub full: bool,
    /// Entries the walker listed under `notes/r*/` plus what it declined to list.
    pub files_listed: usize,
    pub indexed_new: usize,
    pub reindexed_changed: usize,
    /// Content unchanged, but blocks had no vectors and a model is now present.
    pub revectorised: usize,
    pub unchanged: usize,
    /// mtime moved, bytes identical: not reindexed.
    pub touched_only: usize,
    /// Notes the index knew whose file is gone; removed from the index and recorded.
    pub dropped_missing_file: Vec<String>,
    pub skipped: Vec<SkippedFile>,
    pub links_written_back: usize,
    pub link_writeback_failed: Vec<String>,
    pub oversized_blocks: Vec<OversizedBlock>,
    pub embedder: EmbedderSummary,
    pub profile_change: Option<ProfileChange>,
    pub cleared: Option<Erased>,
    /// The index after the scan.
    pub index: IndexStats,
    /// Dry run only: the audit actions a real run would have appended.
    pub audit_preview: Vec<String>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteView {
    pub front: Frontmatter,
    pub kind: String,
    pub body: String,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    /// Citations of its blocks as the index has them; empty when not indexed.
    pub blocks: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BlockView {
    pub citation: String,
    pub idx: u32,
    pub text: String,
    pub token_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Expanded {
    pub citation: String,
    pub block: BlockView,
    pub note: NoteView,
}

#[derive(Debug, Clone, Default)]
pub struct RecallRequest {
    pub n: Option<usize>,
    pub ring: Option<Ring>,
}

#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub ring: Ring,
    pub kind: NoteKind,
    pub name: String,
    pub body: String,
    pub tags: Vec<String>,
    pub retention: Option<String>,
    /// Write despite findings, stamping `flagged`. The CLI's `--force`.
    pub force: bool,
    /// The operator's answer to a hold, when the caller already asked (the UI, §8.1).
    pub choice: Option<OperatorChoice>,
    /// §8.1: refuse to overwrite a note somebody else changed in the meantime.
    pub expected_updated: Option<jiff::Timestamp>,
    pub dry_run: bool,
}

/// What `propose` did, or what it is waiting to be told.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Proposed {
    Written(ProposeReport),
    /// The PII gate held it. Same shape as a held write, and answered the same way.
    Held {
        rendered: String,
        name: String,
        /// Never serialised: a finding carries the matched text.
        #[serde(skip)]
        findings: Vec<cyberbrain_policy::Finding>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ProposeReport {
    pub name: String,
    pub ring: Ring,
    pub kind: String,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub proposed_by: String,
    pub bytes: usize,
    pub pii: PiiState,
    pub redacted: usize,
    /// A note of this name already exists, so accepting this would change it rather than
    /// add one. Worth saying at propose time, not first at review time.
    pub changes_existing: bool,
    pub dry_run: bool,
    pub audit_preview: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProposalSummary {
    pub name: String,
    pub ring: Ring,
    pub kind: String,
    /// `None` when the audit log has no `note.proposed` row for it — a file that appeared
    /// in `proposals/` some other way. It cannot be accepted; `review` says why.
    pub proposed_by: Option<String>,
    pub created: jiff::Timestamp,
    pub changes_existing: bool,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ReviewRequest {
    pub name: String,
    pub accept: bool,
    /// Required when rejecting.
    pub reason: String,
    /// Who is deciding. Never the proposer.
    pub by: String,
    /// Accept even though the note changed after the proposal was made.
    pub force: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewReport {
    pub name: String,
    pub accepted: bool,
    pub by: String,
    pub proposed_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "cyberbrain_core::path_serde::slash_opt"
    )]
    pub path: Option<PathBuf>,
    pub blocks: usize,
    pub vectors: usize,
    pub dry_run: bool,
    pub audit_preview: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WrittenNote {
    pub id: NoteId,
    pub name: String,
    pub ring: Ring,
    pub kind: String,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub bytes: usize,
    /// `true` for a new note, `false` for an update of an existing one.
    pub created: bool,
    pub updated: jiff::Timestamp,
    pub pii: PiiState,
    pub redacted: usize,
    pub blocks: usize,
    pub vectors: usize,
    pub links: usize,
    pub embedder_reason: Option<String>,
    pub dry_run: bool,
    pub audit_preview: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum WriteOutcome {
    Written(WrittenNote),
    /// SPEC §12.4: the write is held and the operator has four choices. Exit code 3 at
    /// the CLI, 409 over HTTP.
    Held {
        name: String,
        findings: Vec<Finding>,
        rendered: String,
    },
    /// `expected_updated` did not match. 409 over HTTP.
    Conflict {
        name: String,
        current_updated: jiff::Timestamp,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorFinding {
    /// `error` | `warning`
    pub severity: &'static str,
    pub check: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub clean: bool,
    pub checks_run: Vec<&'static str>,
    pub findings: Vec<DoctorFinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditSummary {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub rows: usize,
    pub schema_version: u32,
    /// `Ok(rows verified)` or the first break.
    pub chain: std::result::Result<usize, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EmbeddingStatus {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub model_dir: PathBuf,
    pub manifest_present: bool,
    pub embedder: EmbedderSummary,
    pub index_profile: Option<EmbeddingProfile>,
    /// `None` when there is nothing to compare (no model loaded or no vectors stored).
    pub matches_index: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InferenceStatus {
    pub endpoint: String,
    pub model: Option<String>,
    pub state: String,
    pub probe: Option<Probe>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatusReport {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub store: PathBuf,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub config: PathBuf,
    pub notes_on_disk: usize,
    pub notes_per_ring: [usize; 5],
    pub files_skipped: usize,
    pub resident_tokens: usize,
    pub resident_cap: usize,
    pub index: IndexStats,
    pub index_stale: bool,
    pub audit: AuditSummary,
    pub embedding: EmbeddingStatus,
    pub inference: InferenceStatus,
    pub policy: PolicyStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditView {
    pub rows: usize,
    pub verified: Option<std::result::Result<usize, String>>,
    pub rendered: String,
}

/// `policy obligations`, one profile's whole catalogue.
#[derive(Debug, Clone, Serialize)]
pub struct ObligationsView {
    pub profile: cyberbrain_core::PolicyProfile,
    /// The law the profile encodes, as the profile names it.
    pub law: String,
    pub obligations: Vec<cyberbrain_policy::profile::Obligation>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionOutcome {
    pub name: String,
    pub item: RetentionItem,
    pub result: std::result::Result<ErasureReport, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionReport {
    pub queue: RetentionQueue,
    pub unreadable: Vec<String>,
    pub applied_run: bool,
    pub dry_run: bool,
    pub applied: Vec<RetentionOutcome>,
    pub audit_preview: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCardReport {
    pub cards: Vec<ModelCard>,
    pub absent: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsentReport {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub consent: bool,
    pub model_source: Option<String>,
    pub warnings: Vec<String>,
}

/// One definition found by `find` (SPEC §10). Line numbers are 1-based and
/// `start_line..=end_line` is inclusive; `line` names the symbol and lies inside it.
#[derive(Debug, Clone, Serialize)]
pub struct FindHit {
    /// Relative to `FindReport::root`, forward slashes.
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// The line that names the symbol; `start_line` may be earlier when doc comments,
    /// attributes or decorators precede it.
    pub line: u32,
    /// `function`, `method`, `class`, `struct`, `enum`, `trait`, `interface`, `type`,
    /// `impl`, `module`, `namespace`, `macro`, `const`, `static`, `variable`, `table`,
    /// `view`, `index`, `trigger`, `schema`, `section`, `key`, `heading`.
    pub kind: &'static str,
    pub language: &'static str,
    /// The symbol as found.
    pub name: String,
    /// The enclosing named thing (impl target, class, TOML table, parent key path).
    pub scope: Option<String>,
    /// `exact`, `case-insensitive`, `contains`.
    pub matched: &'static str,
    /// The defining line, trimmed, at most 160 characters.
    pub snippet: String,
}

/// What `find` declined to read, by reason (SPEC §14.3: every count names the side of
/// the boundary it counts). A directory kept out by an ignore rule counts once as an
/// entry that was not entered; nothing claims to know how many files were inside it.
#[derive(Debug, Clone, Serialize)]
pub struct FindSkipped {
    /// Entries matched by a `.cyberbrainignore` rule, not entered.
    pub ignored_entries: usize,
    /// Entries matched by a `.gitignore` rule, not entered.
    pub gitignored_entries: usize,
    /// Dot-files and dot-directories, not entered.
    pub hidden_entries: usize,
    /// The store directory itself, not entered.
    pub store_entries: usize,
    /// Symbolic links, never followed.
    pub symlinks: usize,
    /// Lockfiles by name, not read.
    pub lockfiles: usize,
    /// Over the size cap, not read.
    pub too_large: usize,
    /// A NUL byte in the first 8 KiB, not parsed.
    pub binary: usize,
    /// No extractor for the extension, not read.
    pub unsupported: usize,
    pub unsupported_by_extension: std::collections::BTreeMap<String, usize>,
    /// Entries the filesystem refused, with the error.
    pub unreadable: Vec<SkippedFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FindReport {
    /// The symbol as given.
    pub symbol: String,
    /// The name part after scope splitting (`find` for `App::find`).
    pub name: String,
    /// The scope part, when the symbol carried one and it selected something.
    pub scope: Option<String>,
    /// The tree that was scanned.
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub root: PathBuf,
    /// Best first, at most `limit`.
    pub hits: Vec<FindHit>,
    /// Matches before truncation.
    pub matched_total: usize,
    pub truncated: bool,
    pub limit: usize,
    /// Files whose text reached an extractor.
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    /// Definitions extracted across those files, matched or not.
    pub definitions_indexed: usize,
    pub skipped: FindSkipped,
    /// Root-relative paths of the ignore files honoured, in walk order.
    pub ignore_files: Vec<String>,
    /// What the counts cannot say: no ignore file at the root, a symbol defined in
    /// several files, a scope that matched nothing, a truncated list.
    pub caveats: Vec<String>,
    pub elapsed_ms: u128,
}

impl From<cyberbrain_code::FindResult> for FindReport {
    fn from(r: cyberbrain_code::FindResult) -> Self {
        FindReport {
            symbol: r.symbol,
            name: r.name,
            scope: r.scope,
            root: r.root,
            hits: r
                .hits
                .into_iter()
                .map(|h| FindHit {
                    path: h.def.path,
                    start_line: h.def.start_line,
                    end_line: h.def.end_line,
                    line: h.def.line,
                    kind: h.def.kind.as_str(),
                    language: h.def.language.as_str(),
                    name: h.def.name,
                    scope: h.def.scope,
                    matched: h.matched.as_str(),
                    snippet: h.def.snippet,
                })
                .collect(),
            matched_total: r.matched_total,
            truncated: r.truncated,
            limit: r.limit,
            files_scanned: r.files_scanned,
            bytes_scanned: r.bytes_scanned,
            definitions_indexed: r.definitions_indexed,
            skipped: FindSkipped {
                ignored_entries: r.skipped.ignored_entries,
                gitignored_entries: r.skipped.gitignored_entries,
                hidden_entries: r.skipped.hidden_entries,
                store_entries: r.skipped.excluded_entries,
                symlinks: r.skipped.symlinks,
                lockfiles: r.skipped.lockfiles,
                too_large: r.skipped.too_large,
                binary: r.skipped.binary,
                unsupported: r.skipped.unsupported,
                unsupported_by_extension: r.skipped.unsupported_by_extension,
                unreadable: r
                    .skipped
                    .unreadable
                    .into_iter()
                    .map(|(path, reason)| SkippedFile {
                        path: PathBuf::from(path),
                        reason,
                    })
                    .collect(),
            },
            ignore_files: r.ignore_files,
            caveats: r.caveats,
            elapsed_ms: r.elapsed.as_millis(),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Lazy pieces

enum EmbedderState {
    Loaded {
        embedder: Arc<StaticEmbedder>,
        manifest: ArtefactManifest,
        paths: ModelPaths,
    },
    Absent {
        reason: String,
    },
}

#[derive(Clone)]
enum LlmState {
    Ready(Box<LlmClient>),
    Absent(String),
}

/// The audit sink a set of writers records into. Real runs share the store-backed policy;
/// dry runs get a fresh `Policy` over a `MemoryAuditSink` so the real policy code runs and
/// its rows can be shown without being kept.
enum PolicyRef<'a> {
    Real(&'a Policy),
    // Boxed because the owned `Policy` dwarfs the reference in the other variant, and every
    // value of this enum would otherwise carry that much stack whether or not it is a dry
    // run. It grew past the threshold when the policy config took the hub endpoint.
    Dry(Box<Policy>, Arc<MemoryAuditSink>),
}

impl PolicyRef<'_> {
    fn get(&self) -> &Policy {
        match self {
            PolicyRef::Real(p) => p,
            PolicyRef::Dry(p, _) => p,
        }
    }

    fn preview(&self) -> Vec<String> {
        match self {
            PolicyRef::Real(_) => Vec::new(),
            PolicyRef::Dry(_, sink) => sink.actions(),
        }
    }
}

struct Writers<'a> {
    notes: Box<dyn NoteWriter>,
    index: Box<dyn IndexWriter>,
    policy: PolicyRef<'a>,
}

fn kind_name(k: NoteKind) -> String {
    match serde_json::to_value(k) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{k:?}").to_lowercase(),
    }
}

fn stamp_of(path: &Path) -> Option<NoteStamp> {
    let md = std::fs::metadata(path).ok()?;
    let ns = md
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(NoteStamp {
        mtime_ns: i64::try_from(ns).ok()?,
        size: md.len(),
    })
}

fn oversized_reason(r: OversizedReason) -> &'static str {
    match r {
        OversizedReason::CodeFence => "a fenced code block is never split",
        OversizedReason::UnbreakableRun => "a single run of characters longer than the limit",
    }
}

fn policy_config(cfg: &Config) -> PolicyConfig {
    let mut pc = PolicyConfig::from_core(cfg)
        .with_model_download_consent(cfg.embedding.model_download_consent);
    if let Some(src) = &cfg.embedding.model_source {
        pc = pc.with_model_source(src.clone());
    }
    pc
}

// ---------------------------------------------------------------------------------------
// App

pub struct App {
    root: PathBuf,
    config: Config,
    store: Store,
    audit_sink: Arc<StoreAuditSink>,
    policy: Policy,
    gate: Arc<dyn EgressGate>,
    index: Arc<Mutex<Index>>,
    actor: Actor,
    embedder: OnceLock<EmbedderState>,
    llm: Mutex<Option<LlmState>>,
}

/// Ledger task names for the contradiction check. The abandoned one is separate on purpose:
/// a call that was cut off is not a call that cost that much, and the usage page should not
/// average the two together.
const TASK_CONTRADICTION: &str = "contradiction-check";
const TASK_CONTRADICTION_ABANDONED: &str = "contradiction-check-abandoned";

/// How the previous contradiction check on this store ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastCheck {
    /// No row in the ledger: nothing has been measured here yet.
    Unknown,
    /// It finished, and took this long.
    Completed { ms: u64 },
    /// It was still running when the budget ran out.
    Abandoned,
}

/// What to do about the contradiction check, decided before a token is spent.
#[derive(Debug, PartialEq, Eq)]
enum CheckPlan {
    /// Run it, but hand the hits over if it takes longer than this.
    Run(std::time::Duration),
    /// Do not start it: the last one measured longer than the budget, and the endpoint is
    /// not going to have become fast since.
    SkipMeasuredSlow { last_ms: u64, budget_ms: u64 },
    /// Do not start it: the last one was cut off at the budget, and the same bet on the
    /// same endpoint pays the same nothing.
    SkipAbandoned { budget_ms: u64 },
    /// Budget 0: the operator asked to wait however long the client's own timeout allows.
    RunUnbounded,
}

/// The budget is spent on the *next* call, so the decision rests on how the last one ended.
/// Without that memory a slow endpoint costs the full budget on every recall and returns
/// nothing for it, which is the same stall in smaller instalments — measured: with only
/// completed calls remembered, the second recall paid three seconds again.
fn plan_contradiction_check(budget_ms: u64, last: LastCheck) -> CheckPlan {
    if budget_ms == 0 {
        return CheckPlan::RunUnbounded;
    }
    match last {
        LastCheck::Abandoned => CheckPlan::SkipAbandoned { budget_ms },
        LastCheck::Completed { ms } if ms > budget_ms => CheckPlan::SkipMeasuredSlow {
            last_ms: ms,
            budget_ms,
        },
        _ => CheckPlan::Run(std::time::Duration::from_millis(budget_ms)),
    }
}

/// How long a measurement of the inference endpoint speaks for the next call. A day: long
/// enough that a slow endpoint is not re-probed on every recall, short enough that adding a
/// GPU is noticed by tomorrow without anyone deleting a ledger file.
const CHECK_MEASUREMENT_GOOD_FOR: std::time::Duration = std::time::Duration::from_secs(86_400);

/// Whether a ledger timestamp is old enough to say nothing about now. An unparseable one
/// counts as stale: the fallback is to try, not to stay silent forever on a bad string.
fn stale(at: &str) -> bool {
    let Ok(then) = at.parse::<jiff::Timestamp>() else {
        return true;
    };
    jiff::Timestamp::now().duration_since(then).unsigned_abs() > CHECK_MEASUREMENT_GOOD_FOR
}

/// Milliseconds a reader can hold in their head: `126 s`, `3.0 s`, `450 ms`.
fn secs(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 10_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} s", ms / 1000)
    }
}

impl App {
    /// Steps 1 to 5 of §8.2. Steps 6 and 7 happen on first use.
    pub fn open(store: Option<&Path>, actor: Actor) -> Result<App> {
        Self::open_from(store, None, actor)
    }

    /// [`open`](Self::open), with the store discovery walk starting at `start` when no
    /// explicit store was given. Used by the hooks, which are told where the session is.
    pub fn open_from(store: Option<&Path>, start: Option<&Path>, actor: Actor) -> Result<App> {
        let root = match store {
            Some(_) => discover_store(store)?,
            None => discover_store_from(start)?,
        };
        // 1. Config.
        let config = Config::load(&root)?;
        let store = Store::with_config(&root, &config)?;
        // 2. The record, before anything that could produce one.
        let audit_sink = Arc::new(StoreAuditSink::new(AuditStore::open(
            &root.join(AUDIT_DB_FILE),
        )?));
        // 3 + 4. The only audit writer, and the gate built from it.
        let policy = Policy::new(policy_config(&config), audit_sink.clone(), actor.clone());
        let gate = policy.gate();
        // 5. The cache.
        let index = Index::open(&root.join(DB_FILE))?;
        Ok(App {
            root,
            config,
            store,
            audit_sink,
            policy,
            gate,
            index: Arc::new(Mutex::new(index)),
            actor,
            embedder: OnceLock::new(),
            llm: Mutex::new(None),
        })
    }

    /// Create a store. Refuses to touch a directory that already is one.
    pub fn init(path: &Path, actor: &Actor) -> Result<InitReport> {
        if Config::path_in(path).exists() || is_store(path) {
            return Err(Error::Config(format!(
                "{} is already a cyberbrain store; nothing was changed",
                Slash(path)
            )));
        }
        let cap = Config::default().rings.resident_cap_tokens;
        Store::create(path, cap)?;
        let config = Config::write_default(path)?;
        let audit_db = path.join(AUDIT_DB_FILE);
        let sink = Arc::new(StoreAuditSink::new(AuditStore::open(&audit_db)?));
        let index_db = path.join(DB_FILE);
        let _ = Index::open(&index_db)?;
        let cfg = Config::load(path)?;
        let policy = Policy::new(policy_config(&cfg), sink, actor.clone());
        policy.audit().record_raw(
            &actor.to_string(),
            "store.init",
            format!("store:{}", Slash(path)),
            json!({ "profile": cfg.policy.profile }),
        )?;
        Ok(InitReport {
            store: path.to_path_buf(),
            config,
            audit_db,
            index_db,
            next_steps: vec![
                "write a note:   cyberbrain write --ring 2 --kind knowledge --name first-note --body 'what you learned'"
                    .into(),
                "index the tree: cyberbrain scan".into(),
                "search it:      cyberbrain recall 'what you learned'".into(),
                format!(
                    "for semantic search, place model.safetensors, tokenizer.json and {MANIFEST_FILE} under {}",
                    Slash(&cfg.model_dir())
                ),
                "read the compliance profile: cyberbrain policy egress".into(),
            ],
        })
    }

    // The accessors below are the surface hooks, MCP and `serve` build on next; nothing
    // in the CLI dispatch needs them yet, and a bin crate reports that.
    #[allow(dead_code)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[allow(dead_code)]
    pub fn config(&self) -> &Config {
        &self.config
    }

    #[allow(dead_code)]
    pub fn store(&self) -> &Store {
        &self.store
    }

    #[allow(dead_code)]
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    #[allow(dead_code)]
    pub fn gate(&self) -> Arc<dyn EgressGate> {
        self.gate.clone()
    }

    #[allow(dead_code)]
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    fn writers(&self, dry_run: bool) -> Writers<'_> {
        if dry_run {
            let sink = Arc::new(MemoryAuditSink::new());
            let policy = Policy::new(
                policy_config(&self.config),
                sink.clone(),
                self.actor.clone(),
            );
            Writers {
                notes: Box::new(NoopNoteWriter {
                    store: self.store.clone(),
                }),
                index: Box::new(NoopIndexWriter {
                    index: self.index.clone(),
                }),
                policy: PolicyRef::Dry(Box::new(policy), sink),
            }
        } else {
            Writers {
                notes: Box::new(FsNoteWriter {
                    store: self.store.clone(),
                }),
                index: Box::new(SqliteIndexWriter {
                    index: self.index.clone(),
                }),
                policy: PolicyRef::Real(&self.policy),
            }
        }
    }

    // ----- step 6: the embedder, lazily --------------------------------------------------

    fn embedder(&self) -> &EmbedderState {
        self.embedder.get_or_init(|| self.load_embedder())
    }

    fn load_embedder(&self) -> EmbedderState {
        let dir = self.config.model_dir();
        let paths = ModelPaths::in_dir(&dir);
        let manifest_path = dir.join(MANIFEST_FILE);
        if !paths.weights.is_file() || !paths.tokenizer.is_file() {
            return EmbedderState::Absent {
                reason: format!(
                    "no model artefact at {} (expected model.safetensors and tokenizer.json); \
                     search is lexical only",
                    Slash(&dir)
                ),
            };
        }
        let manifest: ArtefactManifest = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(m) => m,
                Err(e) => {
                    return EmbedderState::Absent {
                        reason: format!("{} is not a manifest: {e}", Slash(&manifest_path)),
                    };
                }
            },
            Err(_) => {
                return EmbedderState::Absent {
                    reason: format!(
                        "model files are present but {} is missing; refusing to load \
                         unverified weights (SPEC §6.1)",
                        Slash(&manifest_path)
                    ),
                };
            }
        };
        match StaticEmbedder::load(&paths, &manifest) {
            Ok(e) => EmbedderState::Loaded {
                embedder: Arc::new(e),
                manifest,
                paths,
            },
            Err(e) => EmbedderState::Absent {
                reason: e.to_string(),
            },
        }
    }

    fn embedder_summary(&self) -> EmbedderSummary {
        match self.embedder() {
            EmbedderState::Loaded { embedder, .. } => EmbedderSummary {
                loaded: true,
                profile_id: Some(embedder.profile_id().to_string()),
                dim: Some(embedder.dim()),
                reason: None,
            },
            EmbedderState::Absent { reason } => EmbedderSummary {
                loaded: false,
                profile_id: None,
                dim: None,
                reason: Some(reason.clone()),
            },
        }
    }

    fn embedding_profile(&self) -> Option<EmbeddingProfile> {
        match self.embedder() {
            EmbedderState::Loaded {
                embedder, manifest, ..
            } => Some(EmbeddingProfile {
                id: embedder.profile_id().to_string(),
                dim: embedder.dim(),
                model_hash: manifest.weights_blake3.clone(),
            }),
            EmbedderState::Absent { .. } => None,
        }
    }

    /// Record the loaded model's profile in the index, logging a change. The index wipes
    /// vectors from a previous profile itself; the row saying so is written here, because
    /// the index writes no audit rows (SPEC §4, one writer).
    fn declare_profile(&self, w: &Writers<'_>) -> Result<Option<ProfileChange>> {
        let Some(profile) = self.embedding_profile() else {
            return Ok(None);
        };
        let change = w.index.set_embedding_profile(&profile)?;
        if change.changed {
            w.policy.get().audit().record_raw(
                &Actor::Cli.to_string(),
                "index.embedding-profile-changed",
                format!("embedding:{}", change.current.id),
                json!({
                    "previous": change.previous,
                    "current": change.current,
                    "vectors_wiped": change.vectors_wiped,
                }),
            )?;
        }
        Ok(Some(change))
    }

    fn embed_blocks(&self, texts: &[&str]) -> Result<Option<Vec<Vec<f32>>>> {
        match self.embedder() {
            EmbedderState::Loaded { embedder, .. } => embedder.embed(texts).map(Some),
            EmbedderState::Absent { .. } => Ok(None),
        }
    }

    // ----- step 7: the inference client, lazily and last ---------------------------------

    async fn llm(&self) -> LlmState {
        let cached = self.llm.lock().ok().and_then(|g| g.clone());
        if let Some(s) = cached {
            return s;
        }
        let state = self.connect_llm().await;
        if let Ok(mut g) = self.llm.lock() {
            *g = Some(state.clone());
        }
        state
    }

    async fn connect_llm(&self) -> LlmState {
        let inf = &self.config.inference;
        let Some(model) = inf
            .model
            .as_deref()
            .map(str::trim)
            .filter(|m| !m.is_empty())
        else {
            return LlmState::Absent(
                "no inference model is configured (inference.model in cyberbrain.toml); the \
                 endpoint is never contacted without one"
                    .into(),
            );
        };
        let cfg = LlmConfig {
            base_url: inf.base_url.clone(),
            model: model.to_string(),
            timeout: std::time::Duration::from_millis(inf.timeout_ms),
            allow_public_endpoint: inf.allow_public_endpoint,
            allow_overlay_network: inf.allow_overlay_network,
            ..LlmConfig::default()
        };
        let audit: Arc<dyn cyberbrain_llm::AuditSink> = Arc::new(self.policy.audit().clone());
        match LlmClient::connect(cfg, audit, self.gate.clone()).await {
            Ok(c) => LlmState::Ready(Box::new(c)),
            Err(e) => LlmState::Absent(e.to_string()),
        }
    }

    // ----- scan --------------------------------------------------------------------------

    pub fn scan(&self, opts: ScanOptions) -> Result<ScanReport> {
        let started = Instant::now();
        let w = self.writers(opts.dry_run);
        let listing = self.store.list()?;
        let mut report = ScanReport {
            dry_run: opts.dry_run,
            full: opts.full,
            files_listed: listing.entries.len() + listing.skipped.len(),
            indexed_new: 0,
            reindexed_changed: 0,
            revectorised: 0,
            unchanged: 0,
            touched_only: 0,
            dropped_missing_file: Vec::new(),
            skipped: listing
                .skipped
                .iter()
                .map(|s| SkippedFile {
                    path: s.path.clone(),
                    reason: s.reason.clone(),
                })
                .collect(),
            links_written_back: 0,
            link_writeback_failed: Vec::new(),
            oversized_blocks: Vec::new(),
            embedder: EmbedderSummary::default(),
            profile_change: None,
            cleared: None,
            index: lock_index(&self.index)?.stats()?,
            audit_preview: Vec::new(),
            elapsed_ms: 0,
        };

        if opts.full {
            let cleared = w.index.clear()?;
            w.policy.get().audit().record_raw(
                &Actor::Cli.to_string(),
                "index.cleared",
                "index",
                json!({ "erased": cleared, "dry_run": opts.dry_run }),
            )?;
            report.cleared = Some(cleared);
        }

        // The embedder is loaded here and nowhere earlier: scan needs vectors.
        report.embedder = self.embedder_summary();
        report.profile_change = self.declare_profile(&w)?;
        let model_present = report.embedder.loaded;

        let mut seen: HashSet<NoteId> = HashSet::new();
        for entry in &listing.entries {
            let mut note = match self.store.read_path(&entry.path) {
                Ok(n) => n,
                Err(e) => {
                    report.skipped.push(SkippedFile {
                        path: entry.path.clone(),
                        reason: match &e {
                            Error::Frontmatter { reason, .. } => {
                                format!("unreadable frontmatter: {reason}")
                            }
                            other => other.to_string(),
                        },
                    });
                    continue;
                }
            };
            if !seen.insert(note.front.id) {
                report.skipped.push(SkippedFile {
                    path: entry.path.clone(),
                    reason: format!(
                        "id {} is already used by another note in this tree; ids are unique",
                        note.front.id
                    ),
                });
                continue;
            }

            // Links are derived from the body and written back (SPEC §3.1).
            let targets = link_targets(&note.body);
            if targets != note.front.links {
                note.front.links = targets;
                match w.notes.write(&note) {
                    Ok(_) => report.links_written_back += 1,
                    Err(e) => report
                        .link_writeback_failed
                        .push(format!("{}: {e}", note.front.name)),
                }
            }

            // One hash decides. `content_hash` is the function the index stores, so it is
            // the one the comparison must use.
            let hash = content_hash(&note);
            let existing = lock_index(&self.index)?.note(&note.front.id)?;
            enum Decision {
                New,
                Changed,
                NeedsVectors,
                TouchedOnly,
                Unchanged,
            }
            let decision = match &existing {
                None => Decision::New,
                Some(rec) if rec.hash != hash => Decision::Changed,
                Some(rec) if model_present && rec.vector_count < rec.block_count => {
                    Decision::NeedsVectors
                }
                Some(rec) if rec.stamp != stamp_of(&note.path) => Decision::TouchedOnly,
                Some(_) => Decision::Unchanged,
            };
            match decision {
                Decision::Unchanged => {
                    report.unchanged += 1;
                    continue;
                }
                Decision::TouchedOnly => {
                    report.touched_only += 1;
                    continue;
                }
                _ => {}
            }

            let (blocks, oversized) = blocks_of(&note, MAX_BLOCK_TOKENS);
            for o in oversized {
                report.oversized_blocks.push(OversizedBlock {
                    note: note.front.name.clone(),
                    block_idx: o.block_idx,
                    approx_tokens: o.approx_tokens,
                    reason: oversized_reason(o.reason),
                });
            }
            let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
            let vectors = match self.embed_blocks(&texts) {
                Ok(v) => v,
                Err(e) => {
                    report.skipped.push(SkippedFile {
                        path: entry.path.clone(),
                        reason: format!("embedding failed: {e}"),
                    });
                    continue;
                }
            };
            match w.index.upsert_note(&note, &blocks, vectors.as_deref()) {
                Ok(_) => match decision {
                    Decision::New => report.indexed_new += 1,
                    Decision::Changed => report.reindexed_changed += 1,
                    Decision::NeedsVectors => report.revectorised += 1,
                    _ => unreachable!(),
                },
                Err(e) => report.skipped.push(SkippedFile {
                    path: entry.path.clone(),
                    reason: format!("the index refused it: {e}"),
                }),
            }
        }

        // Rows whose file is gone. The index is a cache of the files; a note deleted by
        // hand is dropped here and the drop is recorded, because a row vanishing without a
        // record is indistinguishable from a bug.
        let known = lock_index(&self.index)?.notes()?;
        for rec in known {
            if seen.contains(&rec.front.id) {
                continue;
            }
            let erased = w.index.delete_note(&rec.front.id)?;
            w.policy.get().audit().record_raw(
                &Actor::Cli.to_string(),
                "index.note-dropped",
                format!("note:{}", erased.id),
                json!({
                    "name": erased.name,
                    "ring": erased.ring,
                    "path": erased.path,
                    "reason": "file missing at scan",
                    "counts": erased.counts,
                    "dry_run": opts.dry_run,
                }),
            )?;
            report.dropped_missing_file.push(erased.name);
        }

        report.index = lock_index(&self.index)?.stats()?;
        report.audit_preview = w.policy.preview();
        report.elapsed_ms = started.elapsed().as_millis();
        Ok(report)
    }

    /// The comparison half of `scan`, for `doctor` and `status`: how many files differ
    /// from the index. Reads everything, writes nothing.
    fn staleness(&self) -> Result<(usize, usize, usize, Vec<SkippedFile>)> {
        let listing = self.store.list()?;
        let ix = lock_index(&self.index)?;
        let mut not_indexed = 0;
        let mut changed = 0;
        let mut unreadable = Vec::new();
        let mut seen = HashSet::new();
        for e in &listing.entries {
            match self.store.read_path(&e.path) {
                Ok(n) => {
                    seen.insert(n.front.id);
                    let mut probe = n.clone();
                    probe.front.links = link_targets(&n.body);
                    match ix.note(&n.front.id)? {
                        None => not_indexed += 1,
                        Some(rec) if rec.hash != content_hash(&probe) => changed += 1,
                        Some(_) => {}
                    }
                }
                Err(err) => unreadable.push(SkippedFile {
                    path: e.path.clone(),
                    reason: err.to_string(),
                }),
            }
        }
        let gone = ix
            .notes()?
            .into_iter()
            .filter(|r| !seen.contains(&r.front.id))
            .count();
        Ok((not_indexed, changed, gone, unreadable))
    }

    // ----- recall ------------------------------------------------------------------------

    pub async fn recall(&self, query: &str, req: &RecallRequest) -> Result<RecallResult> {
        let r = &self.config.retrieval;
        let opts = RecallOptions {
            n: req.n.unwrap_or(r.n),
            k_lex: r.k_lex,
            k_sem: r.k_sem,
            ring: req.ring,
            min_cosine: 0.0,
        };
        let embedder_state = self.embedder();
        let mut result = {
            let ix = lock_index(&self.index)?;
            let embedder: Option<&dyn Embedder> = match embedder_state {
                EmbedderState::Loaded { embedder, .. } => Some(embedder.as_ref()),
                EmbedderState::Absent { .. } => None,
            };
            ix.recall(query, embedder, &opts)?
        };
        if let EmbedderState::Absent { reason } = embedder_state {
            result
                .caveats
                .push(format!("embedder not loaded: {reason}"));
        }
        // Contradiction check (SPEC §7): only with a configured local model, and its
        // absence is said out loud. It runs under a budget, because the hits are ready in
        // milliseconds and this call is the only reason a recall ever feels slow.
        if result.hits.len() >= 2 {
            match self.llm().await {
                LlmState::Ready(client) => {
                    let budget_ms = self.config.inference.contradiction_budget_ms;
                    let last = match hostload::LoadLog::new(&self.root)
                        .last_of(&[TASK_CONTRADICTION, TASK_CONTRADICTION_ABANDONED])
                    {
                        None => LastCheck::Unknown,
                        // A measurement from another day says nothing about this one: a
                        // faster model, a GPU, a machine that was busy last time. Without
                        // this, one slow afternoon would switch the check off for good and
                        // nothing would ever try again.
                        Some(r) if stale(&r.at) => LastCheck::Unknown,
                        Some(r) if r.task == TASK_CONTRADICTION_ABANDONED => LastCheck::Abandoned,
                        Some(r) => LastCheck::Completed { ms: r.wall_ms },
                    };
                    match plan_contradiction_check(budget_ms, last) {
                        CheckPlan::SkipMeasuredSlow { last_ms, budget_ms } => {
                            result.caveats.push(format!(
                                "contradiction check skipped: the last one took {}, over the {} \
                                 budget (inference.contradiction_budget_ms); the hits are not \
                                 checked against each other",
                                secs(last_ms),
                                secs(budget_ms)
                            ));
                        }
                        CheckPlan::SkipAbandoned { budget_ms } => {
                            result.caveats.push(format!(
                                "contradiction check skipped: the last one was still running when \
                                 its {} budget ran out (inference.contradiction_budget_ms); the \
                                 hits are not checked against each other",
                                secs(budget_ms)
                            ));
                        }
                        plan => {
                            let probe = self.load_probe();
                            let started = std::time::Instant::now();
                            let checked = match plan {
                                CheckPlan::Run(budget) => tokio::time::timeout(
                                    budget,
                                    cyberbrain_llm::tasks::find_conflicts(&client, &result.hits),
                                )
                                .await
                                .ok(),
                                _ => Some(
                                    cyberbrain_llm::tasks::find_conflicts(&client, &result.hits)
                                        .await,
                                ),
                            };
                            match checked {
                                Some((conflicts, caveats)) => {
                                    self.record_load(TASK_CONTRADICTION, probe, started.elapsed());
                                    result.conflicts = conflicts;
                                    result.caveats.extend(caveats);
                                }
                                None => {
                                    // Abandoned, not completed: recorded under its own task so
                                    // the ledger keeps saying what happened, and so the next
                                    // call in another process knows without paying again.
                                    self.record_load(
                                        TASK_CONTRADICTION_ABANDONED,
                                        probe,
                                        started.elapsed(),
                                    );
                                    result.caveats.push(format!(
                                        "contradiction check gave up after {} \
                                         (inference.contradiction_budget_ms); the hits are not \
                                         checked against each other",
                                        secs(budget_ms)
                                    ));
                                }
                            }
                        }
                    }
                }
                LlmState::Absent(reason) => result
                    .caveats
                    .push(format!("contradiction check skipped: {reason}")),
            }
        } else {
            result.caveats.push(
                "contradiction check skipped: fewer than two hits, nothing to compare".into(),
            );
        }
        self.record_recall_usage(&result);
        Ok(result)
    }

    /// Ledger row for one recall: the tokens handed over against the tokens the notes those
    /// hits came from hold in full. Both sides come from the index, which counts tokens per
    /// block, so neither is an estimate. Failure to measure is failure to record, never a
    /// zero: a zero here would read as "saved nothing".
    fn record_recall_usage(&self, result: &RecallResult) {
        if result.hits.is_empty() {
            return;
        }
        let cited: std::collections::HashSet<&str> =
            result.hits.iter().map(|h| h.citation.as_str()).collect();
        let notes: std::collections::BTreeSet<&NoteId> =
            result.hits.iter().map(|h| &h.note_id).collect();
        let Ok(ix) = lock_index(&self.index) else {
            return;
        };
        let (mut returned, mut full) = (0u64, 0u64);
        for id in &notes {
            let Ok(blocks) = ix.blocks_of(id) else {
                return;
            };
            for b in blocks {
                full += u64::from(b.token_count);
                if cited.contains(b.citation.to_string().as_str()) {
                    returned += u64::from(b.token_count);
                }
            }
        }
        drop(ix);
        self.usage().append(&usage::UsageRow {
            at: usage::now(),
            op: "recall".into(),
            unit: "tokens".into(),
            returned,
            full,
            hits: result.hits.len() as u64,
            sources: notes.len() as u64,
        });
    }

    /// The before-half of a load measurement. Cheap enough (two small reads, three when a
    /// cgroup is configured) to take around every model call.
    fn load_probe(&self) -> (Option<hostload::HostSample>, Option<hostload::CgroupSample>) {
        (hostload::read_host(), self.cgroup_sample())
    }

    fn cgroup_sample(&self) -> Option<hostload::CgroupSample> {
        let dir = self.config.inference.load_cgroup.as_ref()?;
        hostload::read_cgroup(Path::new(dir))
    }

    fn record_load(
        &self,
        task: &str,
        before: (Option<hostload::HostSample>, Option<hostload::CgroupSample>),
        wall: std::time::Duration,
    ) {
        let after = (hostload::read_host(), self.cgroup_sample());
        let row = hostload::row(task, wall, (before.0, after.0), (before.1, after.1));
        hostload::LoadLog::new(&self.root).append(&row);
    }

    /// The last `days` calendar days of everything the two ledgers and the audit log know,
    /// oldest first, with empty days present and zero. Three sources are merged here rather
    /// than in the page, because the page must not be the place where "no data" and "zero"
    /// get to look alike.
    pub fn usage_by_day(&self, days: usize) -> Vec<usage::DayBucket> {
        let axis = usage::day_axis(days);
        let mut by_date: std::collections::BTreeMap<String, usage::DayBucket> = axis
            .iter()
            .map(|d| {
                (
                    d.clone(),
                    usage::DayBucket {
                        date: d.clone(),
                        ..Default::default()
                    },
                )
            })
            .collect();

        // Retrieval ledger.
        if let Ok(text) = std::fs::read_to_string(self.root.join("usage.jsonl")) {
            for line in text.lines() {
                let Ok(r) = serde_json::from_str::<usage::UsageRow>(line) else {
                    continue;
                };
                let Some(day) = usage::day_of(&r.at).and_then(|d| by_date.get_mut(d)) else {
                    continue;
                };
                let t = if r.op == "find" {
                    &mut day.find
                } else {
                    &mut day.recall
                };
                t.ops += 1;
                t.returned += r.returned;
                t.full += r.full;
                t.hits += r.hits;
            }
        }

        // Model calls, from the audit log.
        let filter = AuditFilter {
            action: Some("inference.call".into()),
            ..AuditFilter::default()
        };
        if let Ok(rows) = self.policy.audit().read(&filter) {
            for r in rows {
                let d = &r.detail;
                let call = d.get("call").and_then(|v| v.as_str()).unwrap_or("");
                if call != "chat-completion" && call != "chat-completion-stream" {
                    continue;
                }
                let ts = r.ts.to_string();
                let Some(day) = usage::day_of(&ts).and_then(|d| by_date.get_mut(d)) else {
                    continue;
                };
                let num = |k: &str| d.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
                day.calls += 1;
                day.prompt_tokens += num("prompt_tokens");
                day.cached_prompt_tokens += num("cached_prompt_tokens");
                day.completion_tokens += num("completion_tokens");
            }
        }

        // Load ledger: averaged, so the day carries a rate and not a sum of rates.
        let mut cores: std::collections::BTreeMap<String, (f64, u64, f64, u64)> =
            std::collections::BTreeMap::new();
        if let Ok(text) = std::fs::read_to_string(self.root.join("load.jsonl")) {
            for line in text.lines() {
                let Ok(r) = serde_json::from_str::<hostload::LoadRow>(line) else {
                    continue;
                };
                let Some(date) = usage::day_of(&r.at).map(str::to_owned) else {
                    continue;
                };
                if let Some(day) = by_date.get_mut(&date) {
                    day.wall_ms += r.wall_ms;
                }
                let e = cores.entry(date).or_insert((0.0, 0, 0.0, 0));
                if let Some(c) = r.endpoint_cores {
                    e.0 += c;
                    e.1 += 1;
                }
                if let Some(c) = r.machine_cores {
                    e.2 += c;
                    e.3 += 1;
                }
            }
        }
        for (date, (ep, epn, ma, man)) in cores {
            let Some(day) = by_date.get_mut(&date) else {
                continue;
            };
            if epn > 0 {
                day.endpoint_cores = Some(ep / epn as f64);
            }
            if man > 0 {
                day.machine_cores = Some(ma / man as f64);
            }
        }
        by_date.into_values().collect()
    }

    pub fn load_summary(&self) -> hostload::LoadSummary {
        hostload::LoadLog::new(&self.root).summary()
    }

    /// What the endpoint says it currently holds in memory. `None` when no model is
    /// configured, the endpoint is unreachable, or it does not answer the vendor route.
    pub async fn loaded_models(&self) -> Option<Vec<cyberbrain_llm::LoadedModel>> {
        match self.llm().await {
            LlmState::Ready(client) => client.loaded_models().await,
            LlmState::Absent(_) => None,
        }
    }

    fn usage(&self) -> usage::UsageLog {
        usage::UsageLog::new(&self.root)
    }

    pub fn usage_summary(&self) -> usage::UsageSummary {
        self.usage().summary()
    }

    /// What the local model actually cost, read back out of the audit log. Grouped by task,
    /// because "contradiction-check" and "session-summary" are paid for separately. Rows the
    /// endpoint did not report counts for are counted as calls and not as tokens, and the
    /// number of those is carried so the totals cannot be mistaken for complete.
    pub fn inference_usage(&self) -> usage::InferenceUsage {
        let filter = AuditFilter {
            action: Some("inference.call".into()),
            ..AuditFilter::default()
        };
        let Ok(rows) = self.policy.audit().read(&filter) else {
            return usage::InferenceUsage::default();
        };
        let mut out = usage::InferenceUsage::default();
        for r in rows {
            let d = &r.detail;
            let call = d.get("call").and_then(|v| v.as_str()).unwrap_or("");
            if call != "chat-completion" && call != "chat-completion-stream" {
                continue;
            }
            let task = d
                .get("task")
                .and_then(|v| v.as_str())
                .unwrap_or("unnamed")
                .to_string();
            let e = out.tasks.entry(task).or_default();
            let num = |k: &str| d.get(k).and_then(|v| v.as_u64());
            e.calls += 1;
            e.elapsed_ms += num("elapsed_ms").unwrap_or(0);
            if d.get("outcome").and_then(|v| v.as_str()) != Some("ok") {
                e.failed += 1;
            }
            match (num("prompt_tokens"), num("completion_tokens")) {
                (Some(p), c) => {
                    e.prompt_tokens += p;
                    e.completion_tokens += c.unwrap_or(0);
                    match num("cached_prompt_tokens") {
                        Some(c) => e.cached_prompt_tokens += c,
                        None => e.calls_without_cache_report += 1,
                    }
                }
                _ => e.calls_without_counts += 1,
            }
            if out.first.is_none() {
                out.first = Some(r.ts.to_string());
            }
            out.last = Some(r.ts.to_string());
        }
        out
    }

    /// `recall --id`: a citation back to its block and the whole note it came from.
    pub fn recall_id(&self, citation: &str) -> Result<Expanded> {
        let cit: Citation = citation.parse()?;
        let (block, rec) = lock_index(&self.index)?.resolve(&cit)?.ok_or_else(|| {
            Error::NoSuchNote(format!(
                "citation {cit} (not in the index; run `cyberbrain scan` if the note exists)"
            ))
        })?;
        let note = self.note_view_at(&rec.path)?;
        Ok(Expanded {
            citation: cit.to_string(),
            block: BlockView {
                citation: block.citation.to_string(),
                idx: block.idx,
                text: block.text,
                token_count: block.token_count,
            },
            note,
        })
    }

    fn note_view_at(&self, path: &Path) -> Result<NoteView> {
        let n = self.store.read_path(path)?;
        let blocks = lock_index(&self.index)?
            .blocks_of(&n.front.id)?
            .into_iter()
            .map(|b| b.citation.to_string())
            .collect();
        Ok(NoteView {
            kind: kind_name(n.front.kind),
            front: n.front,
            body: n.body,
            path: n.path,
            blocks,
        })
    }

    /// A name, or an id. Names are the common case; ids are tried when the name does not
    /// parse or does not exist.
    fn resolve_target(&self, target: &str) -> Result<Note> {
        if let Ok(n) = self.store.read(target) {
            return Ok(n);
        }
        if let Ok(id) = NoteId::from_string(target) {
            if let Some(rec) = lock_index(&self.index)?.note(&id)? {
                return self.store.read_path(&rec.path);
            }
            return self.store.read_by_id(id);
        }
        Err(Error::NoSuchNote(target.to_string()))
    }

    pub fn export(&self, target: &str) -> Result<NoteView> {
        let n = self.resolve_target(target)?;
        self.note_view_at(&n.path)
    }

    // ----- write -------------------------------------------------------------------------

    pub fn write(&self, req: WriteRequest) -> Result<WriteOutcome> {
        let name = req.name.trim().to_string();
        frontmatter::validate_name(&name).map_err(|why| Error::Frontmatter {
            path: PathBuf::from(format!("{name}.md")),
            reason: format!("name `{name}`: {why}"),
        })?;
        if let Some(r) = &req.retention {
            frontmatter::validate_retention(r).map_err(|why| Error::Frontmatter {
                path: PathBuf::from(format!("{name}.md")),
                reason: format!("retention `{r}`: {why}"),
            })?;
        }
        let w = self.writers(req.dry_run);
        let policy = w.policy.get();

        let existing = match self.store.read(&name) {
            Ok(n) => Some(n),
            Err(Error::NoSuchNote(_)) => None,
            Err(e) => return Err(e),
        };
        if let (Some(cur), Some(expected)) = (&existing, req.expected_updated)
            && cur.front.updated != expected
        {
            return Ok(WriteOutcome::Conflict {
                name,
                current_updated: cur.front.updated,
            });
        }

        // SPEC §12.4: the scan runs before any byte is written.
        let (body, pii, redacted) = match policy.check_write(&name, &req.body)? {
            WriteVerdict::Proceed { pii, .. } => (req.body.clone(), pii, 0),
            WriteVerdict::Held { findings } => {
                let choice = req.choice.or(if req.force {
                    Some(OperatorChoice::ProceedFlagged)
                } else {
                    None
                });
                match choice {
                    None => {
                        return Ok(WriteOutcome::Held {
                            rendered: cyberbrain_policy::write_gate::render_hold(
                                &req.body, &findings,
                            ),
                            name,
                            findings,
                        });
                    }
                    Some(c) => {
                        let r = policy.resolve_hold(&name, &req.body, &findings, c)?;
                        (r.body, r.pii, r.redacted)
                    }
                }
            }
        };

        let now = jiff::Timestamp::now();
        let mut tags: Vec<String> = Vec::new();
        for t in req.tags {
            let t = t.trim().to_string();
            if !t.is_empty() && !tags.contains(&t) {
                tags.push(t);
            }
        }
        let front = Frontmatter {
            id: existing
                .as_ref()
                .map(|n| n.front.id)
                .unwrap_or_else(NoteId::generate),
            name: name.clone(),
            ring: req.ring,
            kind: req.kind,
            created: existing.as_ref().map(|n| n.front.created).unwrap_or(now),
            updated: now,
            tags,
            links: link_targets(&body),
            retention: req.retention,
            pii,
        };
        let note = Note {
            front,
            body,
            path: PathBuf::new(),
        };
        let bytes = frontmatter::render(&note.front, &note.body)?.len();
        let path = w.notes.write(&note)?;
        let note = Note { path, ..note };
        policy.record_write(&note.front, bytes)?;

        // A write reindexes the note in the same request (§8.1).
        let (blocks, _) = blocks_of(&note, MAX_BLOCK_TOKENS);
        let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
        self.declare_profile(&w)?;
        let vectors = self.embed_blocks(&texts)?;
        let outcome = w.index.upsert_note(&note, &blocks, vectors.as_deref())?;
        Ok(WriteOutcome::Written(WrittenNote {
            id: note.front.id,
            name,
            ring: note.front.ring,
            kind: kind_name(note.front.kind),
            path: note.path,
            bytes,
            created: existing.is_none(),
            updated: now,
            pii,
            redacted,
            blocks: outcome.blocks,
            vectors: outcome.vectors,
            links: outcome.links,
            embedder_reason: self.embedder_summary().reason,
            dry_run: req.dry_run,
            audit_preview: w.policy.preview(),
        }))
    }

    // ----- review: propose, list, accept, reject -----------------------------------------
    //
    // A proposal is a note that is not a note yet. It lives in `proposals/`, outside the
    // notes tree, and that placement is the whole safety of this: `Frontmatter` does not
    // deny unknown fields, so a state written into a note's own header would be read and
    // ignored by every older binary, and an unapproved ring 0 note would be injected as a
    // live invariant. A directory an older `scan` never walks cannot be ignored.
    //
    // It follows that a proposal is not in the index, so `recall` and `find` cannot return
    // one. That is the intent, not a side effect: an agent that finds an unapproved
    // invariant treats it as one.

    /// Write a note into `proposals/` for somebody else to accept.
    ///
    /// The PII gate runs here rather than at acceptance, so the person who wrote the text is
    /// the one who answers for it. It runs again at acceptance over the same body, because
    /// the profile may have changed in between.
    pub fn propose(&self, req: WriteRequest, who: &str) -> Result<Proposed> {
        let name = req.name.trim().to_string();
        frontmatter::validate_name(&name).map_err(|why| Error::Frontmatter {
            path: PathBuf::from(format!("{name}.md")),
            reason: format!("name `{name}`: {why}"),
        })?;
        if let Some(r) = &req.retention {
            frontmatter::validate_retention(r).map_err(|why| Error::Frontmatter {
                path: PathBuf::from(format!("{name}.md")),
                reason: format!("retention `{r}`: {why}"),
            })?;
        }
        if self.store.read_proposal(&name).is_ok() {
            return Err(Error::Config(format!(
                "a proposal named {name} is already waiting; `cyberbrain review {name} --reject` \
                 it first, or propose under another name"
            )));
        }

        let w = self.writers(req.dry_run);
        let policy = w.policy.get();

        // SPEC §12.4: the scan runs before any byte is written, here as anywhere.
        let (body, pii, redacted) = match policy.check_write(&name, &req.body)? {
            WriteVerdict::Proceed { pii, .. } => (req.body.clone(), pii, 0),
            WriteVerdict::Held { findings } => {
                let choice = req.choice.or(if req.force {
                    Some(OperatorChoice::ProceedFlagged)
                } else {
                    None
                });
                match choice {
                    None => {
                        return Ok(Proposed::Held {
                            rendered: cyberbrain_policy::write_gate::render_hold(
                                &req.body, &findings,
                            ),
                            name,
                            findings,
                        });
                    }
                    Some(c) => {
                        let r = policy.resolve_hold(&name, &req.body, &findings, c)?;
                        (r.body, r.pii, r.redacted)
                    }
                }
            }
        };

        let now = jiff::Timestamp::now();
        let mut tags: Vec<String> = Vec::new();
        for t in req.tags {
            let t = t.trim().to_string();
            if !t.is_empty() && !tags.contains(&t) {
                tags.push(t);
            }
        }
        let replaces = match self.store.read(&name) {
            Ok(n) => Some(n.front.updated),
            Err(Error::NoSuchNote(_)) => None,
            Err(e) => return Err(e),
        };
        let note = Note {
            front: Frontmatter {
                id: NoteId::generate(),
                name: name.clone(),
                ring: req.ring,
                kind: req.kind,
                created: now,
                updated: now,
                tags,
                links: link_targets(&body),
                retention: req.retention,
                pii,
            },
            body,
            path: PathBuf::new(),
        };
        let bytes = frontmatter::render(&note.front, &note.body)?.len();

        // The real path with the file write no-op'd, rather than a simulation beside it
        // (SPEC §8): everything above ran, including the gate and the rendering.
        let path = if req.dry_run {
            self.store.proposal_path(&name)?
        } else {
            self.store.write_proposal(&note)?
        };

        // Who proposed it lives here and only here. The chain is hashed, which makes it a
        // worse thing to forge than a line of YAML in a file anyone can edit — and it means
        // no note format changed for this feature.
        policy.audit().record(
            &self.actor,
            AuditAction::NoteProposed,
            format!("note:{name}"),
            serde_json::json!({
                "by": who,
                "ring": req.ring.as_u8(),
                "kind": kind_name(req.kind),
                "bytes": bytes,
                "pii": pii,
                "changes_existing": replaces.is_some(),
                "dry_run": req.dry_run,
            }),
        )?;

        Ok(Proposed::Written(ProposeReport {
            name,
            ring: note.front.ring,
            kind: kind_name(note.front.kind),
            path,
            proposed_by: who.to_string(),
            bytes,
            pii,
            redacted,
            changes_existing: replaces.is_some(),
            dry_run: req.dry_run,
            audit_preview: w.policy.preview(),
        }))
    }

    /// Everything waiting, oldest first, with who proposed it.
    pub fn proposals(&self) -> Result<Vec<ProposalSummary>> {
        let mut out = Vec::new();
        for note in self.store.list_proposals()? {
            let name = note.front.name.clone();
            out.push(ProposalSummary {
                proposed_by: self.proposer_of(&name)?,
                changes_existing: self.store.read(&name).is_ok(),
                name,
                ring: note.front.ring,
                kind: kind_name(note.front.kind),
                created: note.front.created,
                path: note.path,
            });
        }
        Ok(out)
    }

    /// Who the audit log says proposed the proposal that is **open right now**, if any.
    ///
    /// `None` means there is no open proposal of that name — either nothing was ever
    /// proposed under it, or what was has already been accepted or rejected. Either way the
    /// two-person rule cannot be applied, and `review` says so rather than waving it
    /// through.
    ///
    /// The lifecycle is what makes this sound, and asking only whether a `note.proposed`
    /// row exists was not enough. Those rows stay in the log forever — they have to, it is
    /// hash-chained — so a name that was once proposed and then rejected kept answering
    /// this question for good. Anyone could put a file of that name back into `proposals/`
    /// by hand, with any content and any ring, and `review --accept` would find the old row,
    /// be satisfied, compare the two-person rule against a person who had nothing to do with
    /// it, and write an unapproved ring 0 note whose audit trail then named that person as
    /// its proposer. So the question is not "was this ever proposed" but "is the newest
    /// thing that happened to this name a proposal".
    fn proposer_of(&self, name: &str) -> Result<Option<String>> {
        // Every row about this name in one read: they come back in sequence order, so the
        // last of the three that concern a proposal is the current state. Comparing
        // timestamps across three separate reads would be the same question asked worse —
        // a row carries no sequence number, and two rows can share a timestamp.
        let filter = AuditFilter {
            subject: Some(format!("note:{name}")),
            ..AuditFilter::default()
        };
        let rows = self.policy.audit().read(&filter)?;
        let last = rows.iter().rev().find(|e| {
            e.action == AuditAction::NoteProposed.as_str()
                || e.action == AuditAction::NoteProposalAccepted.as_str()
                || e.action == AuditAction::NoteProposalRejected.as_str()
        });
        Ok(match last {
            Some(e) if e.action == AuditAction::NoteProposed.as_str() => e
                .detail
                .get("by")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            // Accepted, rejected, or never proposed: whatever is in `proposals/` under this
            // name now did not get there through `propose`.
            _ => None,
        })
    }

    /// Accept or reject a proposal.
    pub fn review(&self, req: ReviewRequest) -> Result<ReviewReport> {
        let note = self.store.read_proposal(&req.name)?;
        let name = note.front.name.clone();

        let Some(proposer) = self.proposer_of(&name)? else {
            return Err(Error::Config(format!(
                "the audit log has no record of {name} being proposed, so there is nobody to \
                 check this against. A file that appeared in proposals/ without going through \
                 `cyberbrain propose` is not a proposal; delete it or propose it properly"
            )));
        };
        // The hub's rule, in the same words, for the same reason (§14.1 of docs/HUB.md).
        if proposer == req.by {
            return Err(Error::PolicyRefusal {
                profile: "review".to_string(),
                reason: format!(
                    "a proposal cannot be reviewed by the person who made it. {name} was \
                     proposed by {proposer}"
                ),
            });
        }

        let w = self.writers(req.dry_run);
        let policy = w.policy.get();

        if !req.accept {
            let reason = req.reason.trim();
            if reason.is_empty() {
                return Err(Error::Config(
                    "rejecting needs a reason: it is the only place the proposer will look".into(),
                ));
            }
            if !req.dry_run {
                self.store.remove_proposal(&name)?;
            }
            policy.audit().record(
                &self.actor,
                AuditAction::NoteProposalRejected,
                format!("note:{name}"),
                serde_json::json!({
                    "by": req.by, "proposed_by": proposer, "reason": reason,
                    "dry_run": req.dry_run,
                }),
            )?;
            return Ok(ReviewReport {
                name,
                accepted: false,
                by: req.by,
                proposed_by: proposer,
                reason: Some(reason.to_string()),
                path: None,
                blocks: 0,
                vectors: 0,
                dry_run: req.dry_run,
                audit_preview: w.policy.preview(),
            });
        }

        // A proposal written against a note that has moved on since would overwrite the
        // newer text. Same failure `expected_updated` exists to prevent, over a longer
        // interval: the review may be days after the proposal.
        if let Ok(existing) = self.store.read(&name)
            && existing.front.updated > note.front.created
            && !req.force
        {
            return Err(Error::Config(format!(
                "{name} changed after this was proposed ({} against {}); accepting would \
                 overwrite the newer text. Read both, then `--force` if the proposal is \
                 still right",
                existing.front.updated, note.front.created
            )));
        }

        // Scanned again over the same body: the proposal may have sat for a week and the
        // profile may have changed under it.
        let pii = match policy.check_write(&name, &note.body)? {
            WriteVerdict::Proceed { pii, .. } => pii,
            WriteVerdict::Held { findings } => {
                return Err(Error::PolicyRefusal {
                    profile: "pii".to_string(),
                    reason: format!(
                        "{name} holds {} possible personal data item(s) and cannot be accepted \
                         as it stands. Reject it with a reason; the proposer resolves it and \
                         proposes again",
                        findings.len()
                    ),
                });
            }
        };

        // The id is minted here, not at propose time: a citation must point at something
        // that exists, and until this moment nothing did.
        let now = jiff::Timestamp::now();
        let existing = self.store.read(&name).ok();
        let accepted = Note {
            front: Frontmatter {
                id: existing
                    .as_ref()
                    .map(|n| n.front.id)
                    .unwrap_or_else(NoteId::generate),
                created: existing.as_ref().map(|n| n.front.created).unwrap_or(now),
                updated: now,
                pii,
                ..note.front.clone()
            },
            body: note.body.clone(),
            path: PathBuf::new(),
        };
        let bytes = frontmatter::render(&accepted.front, &accepted.body)?.len();
        let ring = accepted.front.ring;

        let (path, blocks, vectors) = if req.dry_run {
            (self.store.note_path(ring, &name)?, 0, 0)
        } else {
            let path = w.notes.write(&accepted)?;
            let accepted = Note {
                path: path.clone(),
                ..accepted
            };
            policy.record_write(&accepted.front, bytes)?;
            let (blocks, _) = blocks_of(&accepted, MAX_BLOCK_TOKENS);
            let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
            self.declare_profile(&w)?;
            let vectors = self.embed_blocks(&texts)?;
            let outcome = w
                .index
                .upsert_note(&accepted, &blocks, vectors.as_deref())?;
            self.store.remove_proposal(&name)?;
            (path, outcome.blocks, outcome.vectors)
        };

        policy.audit().record(
            &self.actor,
            AuditAction::NoteProposalAccepted,
            format!("note:{name}"),
            serde_json::json!({
                "by": req.by, "proposed_by": proposer,
                "ring": ring.as_u8(), "bytes": bytes,
                "dry_run": req.dry_run,
            }),
        )?;

        Ok(ReviewReport {
            name,
            accepted: true,
            by: req.by,
            proposed_by: proposer,
            reason: None,
            path: Some(path),
            blocks,
            vectors,
            dry_run: req.dry_run,
            audit_preview: w.policy.preview(),
        })
    }

    // ----- erasure: one path -------------------------------------------------------------

    /// `forget`. The retention sweep and the API's `DELETE` reach the same eraser through
    /// `Policy`; this method is the operator's entry to it.
    pub fn forget(&self, target: &str, dry_run: bool) -> Result<ErasureReport> {
        let req = self.erase_request(target, EraseReason::OperatorForget, dry_run)?;
        let w = self.writers(dry_run);
        let mut eraser = StoreEraser {
            notes: w.notes.as_ref(),
            index: w.index.as_ref(),
        };
        let mut report = w.policy.get().forget(&mut eraser, &req)?;
        if dry_run {
            for a in w.policy.preview() {
                report
                    .notes
                    .push(format!("audit row a real run would append: {a}"));
            }
        }
        Ok(report)
    }

    fn erase_request(
        &self,
        target: &str,
        reason: EraseReason,
        dry_run: bool,
    ) -> Result<EraseRequest> {
        // Prefer the file (authoritative); fall back to the index for a note whose file is
        // already gone, so `forget` can still clean the rows up.
        let (id, name, ring, path) = match self.resolve_target(target) {
            Ok(n) => (n.front.id, n.front.name, n.front.ring, n.path),
            Err(Error::NoSuchNote(_)) => {
                let ix = lock_index(&self.index)?;
                let rec = match ix.note_by_name(target)? {
                    Some(r) => Some(r),
                    None => match NoteId::from_string(target) {
                        Ok(id) => ix.note(&id)?,
                        Err(_) => None,
                    },
                };
                let rec = rec.ok_or_else(|| Error::NoSuchNote(target.to_string()))?;
                (rec.front.id, rec.front.name, rec.front.ring, rec.path)
            }
            Err(e) => return Err(e),
        };
        Ok(EraseRequest {
            note_id: id,
            name,
            ring,
            path,
            reason,
            dry_run,
        })
    }

    // ----- doctor ------------------------------------------------------------------------

    pub fn doctor(&self) -> Result<DoctorReport> {
        let mut findings = Vec::new();
        let mut checks = Vec::new();
        let mut push = |severity: &'static str, check: &'static str, detail: String| {
            findings.push(DoctorFinding {
                severity,
                check,
                detail,
            })
        };

        checks.push("notes tree");
        let listing = self.store.list()?;
        for s in &listing.skipped {
            push(
                "warning",
                "notes tree",
                format!("{}: {}", Slash(&s.path), s.reason),
            );
        }

        checks.push("stale index");
        let (not_indexed, changed, gone, unreadable) = self.staleness()?;
        for u in unreadable {
            push(
                "error",
                "unreadable note",
                format!("{}: {}", Slash(&u.path), u.reason),
            );
        }
        if not_indexed + changed + gone > 0 {
            push(
                "warning",
                "stale index",
                format!(
                    "{not_indexed} notes not indexed, {changed} changed on disk since the last \
                     scan, {gone} indexed notes whose file is gone; run `cyberbrain scan`"
                ),
            );
        }

        // Two different facts, and lumping them together hides the actionable one. A link
        // to a name that could exist is intent: somebody will write that note. A link to a
        // name that can never be a note — underscores, capitals, a path — is a typo or a
        // leftover from another tool's naming, and no amount of writing notes will ever
        // resolve it. Reporting both as "does not exist (valid: it names intent)" tells the
        // operator to wait for something that is never coming.
        checks.push("dangling links");
        checks.push("unresolvable links");
        {
            let ix = lock_index(&self.index)?;
            for l in ix.dangling_links()? {
                let from = ix
                    .note(&l.from_note)?
                    .map(|r| r.front.name)
                    .unwrap_or_else(|| l.from_note.to_string());
                match cyberbrain_core::validate_name(&l.to_name) {
                    Ok(()) => push(
                        "warning",
                        "dangling links",
                        format!(
                            "{from} links to [[{}]] which does not exist yet (valid name: it names intent)",
                            l.to_name
                        ),
                    ),
                    Err(reason) => {
                        // Offer the kebab-case form when a note actually sits under it.
                        // Most of these come from another tool's file names, and the
                        // note the author meant is often already in the store.
                        let normalised = normalise_link_target(&l.to_name);
                        let hint = match ix.note_by_name(&normalised)? {
                            Some(_) => format!("; did you mean [[{normalised}]]?"),
                            None => String::new(),
                        };
                        push(
                            "warning",
                            "unresolvable links",
                            format!(
                                "{from} links to [[{}]], which can never resolve: a note name {reason}{hint}",
                                l.to_name
                            ),
                        )
                    }
                }
            }
        }

        checks.push("ring cap");
        let resident = self.store.resident_tokens()?;
        let cap = self.store.resident_cap();
        if resident > cap {
            push(
                "error",
                "ring cap",
                format!(
                    "rings 0+1 hold ~{resident} tokens, over the cap of {cap}; a hand edit crossed it"
                ),
            );
        } else if resident * 10 >= cap * 8 {
            push(
                "warning",
                "ring cap",
                format!(
                    "rings 0+1 hold ~{resident} of {cap} tokens ({}%)",
                    resident * 100 / cap
                ),
            );
        }

        checks.push("index integrity");
        for p in lock_index(&self.index)?.integrity()? {
            push("error", "index integrity", p);
        }

        checks.push("embedding profile");
        let stored = lock_index(&self.index)?.embedding_profile()?;
        match (stored, self.embedder()) {
            (Some(p), EmbedderState::Loaded { embedder, .. }) => {
                if let Err(e) = lock_index(&self.index)?.check_embedder(embedder.as_ref()) {
                    let _ = p;
                    push("error", "embedding profile", e.to_string());
                }
            }
            (Some(p), EmbedderState::Absent { reason }) => push(
                "warning",
                "embedding profile",
                format!(
                    "index vectors come from {} but no model is loaded ({reason}); semantic search is off",
                    p.id
                ),
            ),
            (None, EmbedderState::Loaded { .. }) => {
                if lock_index(&self.index)?.stats()?.blocks > 0 {
                    push(
                        "warning",
                        "embedding profile",
                        "a model is present but the index holds no vectors; run `cyberbrain scan`"
                            .into(),
                    );
                }
            }
            (None, EmbedderState::Absent { .. }) => {}
        }

        checks.push("audit chain");
        if let Err(e) = self.policy.verify_audit() {
            push("error", "audit chain", e.to_string());
        }

        checks.push("retention");
        let (queue, _) = self.retention_queue()?;
        for i in &queue.items {
            if let cyberbrain_policy::RetentionStatus::Invalid { reason } = &i.status {
                push("warning", "retention", format!("{}: {reason}", i.name));
            }
        }
        if queue.due > 0 {
            push(
                "warning",
                "retention",
                format!(
                    "{} note(s) past their retention; nothing expires by itself, run `cyberbrain policy retention`",
                    queue.due
                ),
            );
        }

        Ok(DoctorReport {
            clean: findings.is_empty(),
            checks_run: checks,
            findings,
        })
    }

    // ----- status ------------------------------------------------------------------------

    pub async fn status(&self) -> Result<StatusReport> {
        let listing = self.store.list()?;
        let mut per_ring = [0usize; 5];
        for e in &listing.entries {
            per_ring[e.ring.as_u8() as usize] += 1;
        }
        let index = lock_index(&self.index)?.stats()?;
        let (not_indexed, changed, gone, _) = self.staleness()?;
        let embedder = self.embedder_summary();
        let matches_index = match (&index.embedding, self.embedder()) {
            (Some(_), EmbedderState::Loaded { embedder, .. }) => Some(
                lock_index(&self.index)?
                    .check_embedder(embedder.as_ref())
                    .is_ok(),
            ),
            _ => None,
        };
        let model_dir = self.config.model_dir();
        let inf = &self.config.inference;
        let inference = match self.llm().await {
            LlmState::Ready(c) => {
                let probe = c.probe().await;
                InferenceStatus {
                    endpoint: inf.base_url.clone(),
                    model: inf.model.clone(),
                    state: if probe.reachable {
                        "reachable".into()
                    } else {
                        "configured but unreachable".into()
                    },
                    probe: Some(probe),
                }
            }
            LlmState::Absent(reason) => InferenceStatus {
                endpoint: inf.base_url.clone(),
                model: inf.model.clone(),
                state: format!("not in use: {reason}"),
                probe: None,
            },
        };
        Ok(StatusReport {
            store: self.root.clone(),
            config: self.store.config_path(),
            notes_on_disk: listing.entries.len(),
            notes_per_ring: per_ring,
            files_skipped: listing.skipped.len(),
            resident_tokens: self.store.resident_tokens()?,
            resident_cap: self.store.resident_cap(),
            index_stale: not_indexed + changed + gone > 0,
            index,
            audit: AuditSummary {
                path: self.root.join(AUDIT_DB_FILE),
                rows: self.audit_sink.count()?,
                schema_version: self.audit_sink.schema_version()?,
                chain: self.policy.verify_audit().map_err(|e| e.to_string()),
            },
            embedding: EmbeddingStatus {
                manifest_present: model_dir.join(MANIFEST_FILE).is_file(),
                model_dir,
                embedder,
                index_profile: lock_index(&self.index)?.embedding_profile()?,
                matches_index,
            },
            inference,
            policy: self.policy.status(),
        })
    }

    // ----- find --------------------------------------------------------------------------

    /// The tree `find` scans. The store lives at `<project>/.cyberbrain` by default
    /// (SPEC §4), so the project is the store's parent. A store placed elsewhere with
    /// `--store` says nothing about where the code is; the working directory is the
    /// only other candidate and it is what an agent's hooks run in. The report names
    /// the root it used either way, so the caller can see which tree answered.
    fn code_root(&self) -> PathBuf {
        let is_default_store = self
            .root
            .file_name()
            .is_some_and(|n| n == DEFAULT_STORE_DIR);
        match (is_default_store, self.root.parent()) {
            (true, Some(parent)) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            (true, Some(_)) => PathBuf::from("."),
            _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    /// `find <symbol>` (SPEC §10): line ranges of the definitions of `symbol` in the
    /// project tree, so the agent reads a slice instead of a file.
    ///
    /// Synchronous and index-free: hooks and MCP call it and neither has a runtime to
    /// spare, and a scan of the tree as it is now cannot hand back a stale range. The
    /// store itself is excluded from the walk; its notes belong to `recall`. Nothing in
    /// `cyberbrain.toml` configures the code index yet (see the report), so the crate's
    /// defaults apply: `.cyberbrainignore` and `.gitignore` honoured, hidden entries
    /// skipped, files over 1 MiB not read.
    pub fn find(&self, symbol: &str, limit: usize) -> Result<FindReport> {
        let opts = cyberbrain_code::FindOptions {
            exclude: vec![self.root.clone()],
            ..cyberbrain_code::FindOptions::default()
        };
        let result = cyberbrain_code::find(&self.code_root(), symbol, limit, &opts)?;
        let report = FindReport::from(result);
        self.record_find_usage(&report);
        Ok(report)
    }

    /// Ledger row for one find, counted in lines: the spans the caller is told to read
    /// against the length of the files they sit in. The files were just walked, so reading
    /// their line count back costs a warm read of at most `limit` files; a file that cannot
    /// be read is left out of both sides rather than counted as free.
    fn record_find_usage(&self, report: &FindReport) {
        if report.hits.is_empty() {
            return;
        }
        let mut returned = 0u64;
        let mut full = 0u64;
        let mut counted: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for h in &report.hits {
            returned += u64::from(h.end_line.saturating_sub(h.start_line)) + 1;
            if counted.insert(h.path.as_str()) {
                match std::fs::read_to_string(report.root.join(&h.path)) {
                    Ok(text) => full += text.lines().count() as u64,
                    Err(_) => {
                        counted.remove(h.path.as_str());
                    }
                }
            }
        }
        if full == 0 {
            return;
        }
        self.usage().append(&usage::UsageRow {
            at: usage::now(),
            op: "find".into(),
            unit: "lines".into(),
            returned,
            full,
            hits: report.hits.len() as u64,
            sources: counted.len() as u64,
        });
    }

    // ----- policy ------------------------------------------------------------------------

    pub fn policy_egress(&self) -> Vec<EgressEntry> {
        self.policy.egress().register()
    }

    /// What the active profile claims about the law, with the basis and how sure the author
    /// is of each line.
    ///
    /// The catalogue has existed since the profiles did and was read by nothing but a test:
    /// a compliance claim nobody can print is a comment. Printing it is also the honest
    /// move, because roughly a third of the lines carry a confidence below `high`, and a
    /// reader who cannot see that gap will assume there is none.
    pub fn policy_obligations(&self) -> ObligationsView {
        let profile = self.config.policy.profile;
        ObligationsView {
            profile,
            law: profile.law().to_string(),
            obligations: profile.obligations(),
        }
    }

    /// The audit log, rendered. Reading through `export` records the export itself
    /// (SPEC §12.6), after the rows were rendered.
    /// Point this store at a hub, from an invitation. Returns the inference endpoint if the
    /// invitation carried one.
    pub fn enrol_with_hub(
        &self,
        hub_url: &str,
        device: &str,
        inference_url: Option<&str>,
    ) -> Result<Option<String>> {
        let path = self.store.config_path();
        let text = std::fs::read_to_string(&path).map_err(|e| Error::Io {
            path: path.clone(),
            source: e,
        })?;
        let mut updated = crate::hub::client::set_hub_in_config(&text, hub_url, device);
        if let Some(url) = inference_url {
            updated = crate::hub::client::set_inference_url(&updated, url);
        }
        // Parsed before it is written: a config file this command broke would leave the
        // store unusable, and the person would have no idea what changed.
        cyberbrain_core::config::Config::parse(&updated, &self.root)
            .map_err(|e| Error::Config(format!("enrolment would break the config file: {e}")))?;
        std::fs::write(&path, updated).map_err(|e| Error::Io {
            path: path.clone(),
            source: e,
        })?;
        Ok(inference_url.map(str::to_owned))
    }

    /// What this machine is, as far as a company hub is concerned.
    ///
    /// The dashboard's own question. Somebody looking at their notes cannot tell from that
    /// screen whether this machine reports to anybody, and "check with `cyberbrain hub
    /// push`" is not an answer for the person the delivery is *about*.
    ///
    /// The last delivery is read out of this store's own audit log, not from a note kept on
    /// the side: the audit log is what was actually sent, so the two cannot disagree.
    pub fn hub_status(&self) -> Result<serde_json::Value> {
        let Some(url) = self.config.hub.url.clone() else {
            return Ok(serde_json::json!({ "enrolled": false }));
        };
        let filter = cyberbrain_policy::AuditFilter {
            action: Some("egress.completed".into()),
            contains: Some("audit-sync".into()),
            ..Default::default()
        };
        let last = self
            .policy
            .audit()
            .read(&filter)
            .ok()
            .and_then(|rows| rows.last().map(|e| e.ts.to_string()));
        Ok(serde_json::json!({
            "enrolled": true,
            "hub": url,
            "device": self.config.hub.device,
            "last_delivery": last,
        }))
    }

    /// Deliver audit rows to the hub this store was enrolled with.
    ///
    /// Returns the report and the exit code. A hub that is not collecting, and a gap it can
    /// still close, are **not** failures of this command: they are states a timer should see
    /// and carry on from. Only something the operator has to fix exits non-zero.
    pub async fn push_to_hub(
        &self,
        since: Option<jiff::Timestamp>,
    ) -> Result<(serde_json::Value, i32)> {
        use crate::hub::client::{self, Reply};

        let hub_url = self.config.hub.url.clone().ok_or_else(|| {
            Error::Config(
                "this store is not enrolled with a hub; run `cyberbrain hub enrol <invitation>`"
                    .into(),
            )
        })?;
        let token = client::token_for(&hub_url)?;
        let version = env!("CARGO_PKG_VERSION");

        let filter = cyberbrain_policy::AuditFilter {
            since,
            ..Default::default()
        };
        let bundle = self.export_audit_bundle(&filter)?;

        let egress = self.policy.egress();
        let actor = cyberbrain_policy::Actor::Operator;
        let reply = client::deliver(egress, &actor, &hub_url, &token, version, bundle).await?;

        Ok(match reply {
            Reply::Ok(d) => (
                serde_json::json!({
                    "state": "delivered",
                    "accepted": d.accepted,
                    "total_rows": d.total_rows,
                    "hub": d.hub,
                    "message": format!(
                        "delivered {} new row(s) to {}; the hub now holds {}",
                        d.accepted, d.hub, d.total_rows
                    ),
                }),
                0,
            ),
            Reply::NotCollecting(m) => (
                serde_json::json!({
                    "state": "not-collecting",
                    "message": format!("{m}\nNothing was lost; this store keeps its rows."),
                }),
                0,
            ),
            Reply::Gap { expected } => (
                serde_json::json!({
                    "state": "gap",
                    "expected_anchor": expected,
                    "message": format!(
                        "the hub is at {} and this delivery did not reach back that far. \
                         Send a wider period: `cyberbrain hub push` without --since covers \
                         everything.",
                        &expected[..expected.len().min(12)]
                    ),
                }),
                0,
            ),
            Reply::Refused { status, message } => (
                serde_json::json!({
                    "state": "refused",
                    "status": status,
                    "message": format!("the hub refused the delivery ({status}): {message}"),
                }),
                1,
            ),
        })
    }

    /// A period of the log as a self-checking bundle, for somebody outside to verify.
    ///
    /// The tool string goes in the header for the reader's benefit; nothing in the check
    /// depends on it, which is the point — a file that only this version can verify would
    /// not survive the retention period it exists for.
    pub fn export_audit_bundle(&self, filter: &AuditFilter) -> Result<String> {
        let tool = concat!("cyberbrain ", env!("CARGO_PKG_VERSION"));
        self.policy.export_audit_bundle(filter, tool)
    }

    pub fn policy_audit(
        &self,
        filter: &AuditFilter,
        verify: bool,
        format: ExportFormat,
    ) -> Result<AuditView> {
        let verified = verify.then(|| self.policy.verify_audit().map_err(|e| e.to_string()));
        let rows = self.policy.audit().read(filter)?.len();

        // An `--action` that matches nothing is refused, not answered with an empty table.
        // In an audit tool the two readings are opposite: "no such action name" and
        // "nothing of that kind ever happened". Someone checking whether erasures occurred
        // types the obvious name, gets zero rows and concludes the wrong thing. So when a
        // filter selects nothing out of a non-empty log, say which actions are actually in
        // it — measured from the log rather than a hard-coded list, because the binary and
        // the index write names the policy vocabulary does not contain.
        if rows == 0 && filter.action.is_some() {
            let all = self.policy.audit().read(&AuditFilter::default())?;
            if !all.is_empty() {
                let wanted = filter.action.as_deref().unwrap_or_default();
                let mut present: Vec<String> = all
                    .iter()
                    .map(|e| e.action.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                present.sort();
                if !present
                    .iter()
                    .any(|a| a == wanted || a.starts_with(&format!("{wanted}.")))
                {
                    return Err(Error::Config(format!(
                        "no audit action named {wanted:?}; the log contains: {}. Refused rather than answered with an empty table: a missing name and a thing that never happened are different answers",
                        present.join(", ")
                    )));
                }
            }
        }

        let rendered = self.policy.export_audit(filter, format)?;
        Ok(AuditView {
            rows,
            verified,
            rendered,
        })
    }

    pub fn policy_subject(&self, identifier: &str) -> Result<SubjectAccessReport> {
        let source = IndexSubjectSource { index: &self.index };
        self.policy.subject_access(&source, identifier)
    }

    /// Every note on disk with its frontmatter, for retention. Files are authoritative,
    /// so this reads the tree rather than the index.
    fn retention_queue(&self) -> Result<(RetentionQueue, Vec<String>)> {
        let listing = self.store.list()?;
        let mut notes = Vec::new();
        let mut unreadable = Vec::new();
        for e in &listing.entries {
            match self.store.read_path(&e.path) {
                Ok(n) => notes.push((n.front, n.path)),
                Err(err) => unreadable.push(format!("{}: {err}", Slash(&e.path))),
            }
        }
        let queue = self
            .policy
            .retention_queue(notes.iter().map(|(f, p)| (f, p.as_path())));
        Ok((queue, unreadable))
    }

    pub fn policy_retention(&self, apply: bool, dry_run: bool) -> Result<RetentionReport> {
        let (queue, unreadable) = self.retention_queue()?;
        let mut report = RetentionReport {
            queue,
            unreadable,
            applied_run: apply,
            dry_run,
            applied: Vec::new(),
            audit_preview: Vec::new(),
        };
        if !apply {
            return Ok(report);
        }
        let w = self.writers(dry_run);
        let mut eraser = StoreEraser {
            notes: w.notes.as_ref(),
            index: w.index.as_ref(),
        };
        for (item, result) in w
            .policy
            .get()
            .apply_retention(&mut eraser, &report.queue, dry_run)
        {
            report.applied.push(RetentionOutcome {
                name: item.name.clone(),
                item,
                result: result.map_err(|e| e.to_string()),
            });
        }
        report.audit_preview = w.policy.preview();
        Ok(report)
    }

    pub fn policy_model_card(&self) -> ModelCardReport {
        let cards = self.policy.model_cards(&[self]);
        let mut absent = Vec::new();
        if let EmbedderState::Absent { reason } = self.embedder() {
            absent.push(format!("embedding model: {reason}"));
        }
        if self.config.inference.model.is_none() {
            absent.push(
                "inference model: none configured (inference.model); the endpoint is not contacted"
                    .into(),
            );
        }
        ModelCardReport { cards, absent }
    }

    /// Persist consent in `cyberbrain.toml` (SPEC §12.1: consent lives in the file, not in
    /// memory). Edits the one key in place so the operator's comments survive.
    pub fn policy_consent(&self, grant: bool) -> Result<ConsentReport> {
        let path = self.store.config_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                cyberbrain_core::config::DEFAULT_TOML.to_string()
            }
            Err(e) => return Err(Error::Io { path, source: e }),
        };
        let new_line = format!("model_download_consent = {grant}");
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        let mut in_embedding = false;
        let mut replaced = false;
        let mut embedding_header: Option<usize> = None;
        for (i, line) in lines.iter_mut().enumerate() {
            let t = line.trim();
            if t.starts_with('[') {
                in_embedding = t == "[embedding]";
                if in_embedding {
                    embedding_header = Some(i);
                }
                continue;
            }
            if in_embedding
                && !t.starts_with('#')
                && t.split('=').next().map(str::trim) == Some("model_download_consent")
            {
                *line = new_line.clone();
                replaced = true;
            }
        }
        if !replaced {
            match embedding_header {
                Some(i) => lines.insert(i + 1, new_line),
                None => {
                    lines.push(String::new());
                    lines.push("[embedding]".into());
                    lines.push(new_line);
                }
            }
        }
        let mut out = lines.join("\n");
        out.push('\n');
        // Validate before writing: a config that no longer parses is worse than no consent.
        let parsed = Config::parse(&out, &path)?;
        write_atomic(&path, out.as_bytes())?;
        let mut warnings = Vec::new();
        if parsed.embedding.model_source.is_none() {
            warnings.push(
                "embedding.model_source is unset, so nothing can be downloaded regardless of \
                 consent; the artefact has to be placed by hand"
                    .into(),
            );
        }
        warnings.push(
            "takes effect on the next command; this process keeps the register it started with"
                .into(),
        );
        self.policy.audit().record_raw(
            &self.actor.to_string(),
            "consent.model-download",
            "model-download",
            json!({ "consent": grant, "model_source": parsed.embedding.model_source }),
        )?;
        Ok(ConsentReport {
            path,
            consent: grant,
            model_source: parsed.embedding.model_source,
            warnings,
        })
    }
}

/// SPEC §12.7: what this binary can vouch for about the models it uses. Fields it did not
/// read from the artefact stay `None` and print as *not stated*.
impl ModelInventory for App {
    fn model_cards(&self) -> Vec<ModelCard> {
        let mut cards = Vec::new();
        if let EmbedderState::Loaded {
            embedder,
            manifest,
            paths,
        } = self.embedder()
        {
            let info = embedder.info();
            let dir = self.config.model_dir();
            let mut c = ModelCard::new(
                ModelRole::Embedding,
                dir.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "model2vec".into()),
                self.config
                    .embedding
                    .model_source
                    .clone()
                    .unwrap_or_else(|| "placed by hand (no model_source configured)".into()),
                "static token embeddings for hybrid recall over the notes in this store",
            );
            c.blake3 = Some(manifest.weights_blake3.clone());
            c.hash_verified = Some(true);
            c.dimension = Some(info.dim);
            c.pooling = Some(info.pooling.to_string());
            c.format = Some(format!(
                "model2vec: safetensors [{} x {}] {} + tokenizer.json (blake3 {})",
                info.vocab_rows, info.dim, info.weights_dtype, manifest.tokenizer_blake3
            ));
            c.size_bytes = std::fs::metadata(&paths.weights)
                .ok()
                .map(|m| m.len())
                .and_then(|w| {
                    std::fs::metadata(&paths.tokenizer)
                        .ok()
                        .map(|t| w + t.len())
                });
            c.artefact_path = Some(dir);
            c.limitations.push(
                "static embeddings: no context, no word order; retrieval quality below a transformer".into(),
            );
            cards.push(c);
        }
        if let Some(model) = &self.config.inference.model {
            let mut c = ModelCard::new(
                ModelRole::Inference,
                model.clone(),
                self.config.inference.base_url.clone(),
                "session summaries, contradiction checks between retrieved blocks, ring and tag suggestions, note supersession (SPEC §11)",
            );
            c.format = Some("OpenAI-compatible HTTP, operated by the deployer".into());
            c.notes.push(
                "licence, version and weights are the endpoint operator's; not read by this tool"
                    .into(),
            );
            cards.push(c);
        }
        cards
    }
}

struct IndexSubjectSource<'a> {
    index: &'a Mutex<Index>,
}

impl SubjectSource for IndexSubjectSource<'_> {
    fn blocks_mentioning(&self, identifier: &Identifier) -> Result<Vec<SubjectBlock>> {
        Ok(lock_index(self.index)?
            .blocks_containing(identifier.raw())?
            .into_iter()
            .map(|(cit, name, text)| SubjectBlock {
                citation: cit.to_string(),
                note_id: None,
                note_name: name,
                ring: Some(cit.ring),
                text,
            })
            .collect())
    }
}

/// The kebab-case form of a link target, for suggesting what the author probably meant.
/// Underscores and spaces become hyphens, capitals fold down, everything else is dropped,
/// and runs of hyphens collapse. Only ever used to look up an existing note, never to
/// rewrite a link: a `[[target]]` in a note is the author's text and stays theirs.
fn normalise_link_target(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        match ch {
            'a'..='z' | '0'..='9' => out.push(ch),
            'A'..='Z' => out.push(ch.to_ascii_lowercase()),
            '_' | ' ' | '-' | '.' | '/' if !out.ends_with('-') => out.push('-'),
            _ => {}
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod contradiction_budget_tests {
    use super::*;

    /// Calibrated against the state that made this exist: a store whose ledger says the
    /// last check took 126 s is not asked to try again, because it would stall the recall
    /// for two minutes and then be cancelled anyway.
    #[test]
    fn a_measured_slow_endpoint_is_not_asked_again() {
        assert_eq!(
            plan_contradiction_check(3_000, LastCheck::Completed { ms: 126_184 }),
            CheckPlan::SkipMeasuredSlow {
                last_ms: 126_184,
                budget_ms: 3_000
            }
        );
    }

    /// The case the first implementation got wrong, found by timing two calls in a row: a
    /// check that was cut off leaves no completed measurement, so remembering only
    /// completed calls made every recall pay the budget again and get nothing for it.
    #[test]
    fn an_abandoned_check_is_not_retried_either() {
        assert_eq!(
            plan_contradiction_check(3_000, LastCheck::Abandoned),
            CheckPlan::SkipAbandoned { budget_ms: 3_000 }
        );
    }

    #[test]
    fn a_fast_one_runs_under_the_budget() {
        assert_eq!(
            plan_contradiction_check(3_000, LastCheck::Completed { ms: 800 }),
            CheckPlan::Run(std::time::Duration::from_millis(3_000))
        );
        // Nothing measured yet: try once. That is what makes the first call the one that
        // learns, rather than every call paying for the ignorance of the last.
        assert_eq!(
            plan_contradiction_check(3_000, LastCheck::Unknown),
            CheckPlan::Run(std::time::Duration::from_millis(3_000))
        );
    }

    /// Exactly at the budget is not over it.
    #[test]
    fn the_boundary_is_not_slow() {
        assert_eq!(
            plan_contradiction_check(3_000, LastCheck::Completed { ms: 3_000 }),
            CheckPlan::Run(std::time::Duration::from_millis(3_000))
        );
    }

    /// Yesterday's measurement is not evidence about today's endpoint.
    #[test]
    fn a_measurement_expires_after_a_day() {
        let now = jiff::Timestamp::now();
        let fresh = (now - jiff::SignedDuration::from_hours(2)).to_string();
        let old = (now - jiff::SignedDuration::from_hours(30)).to_string();
        assert!(!stale(&fresh), "two hours old still counts");
        assert!(stale(&old), "thirty hours old does not");
        assert!(
            stale("not a timestamp"),
            "an unreadable stamp means try again"
        );
    }

    #[test]
    fn zero_means_wait_however_long_it_takes() {
        assert_eq!(
            plan_contradiction_check(0, LastCheck::Completed { ms: 126_184 }),
            CheckPlan::RunUnbounded
        );
        assert_eq!(
            plan_contradiction_check(0, LastCheck::Abandoned),
            CheckPlan::RunUnbounded
        );
    }

    /// The caveat is read by a person deciding whether to care, so the number has to be
    /// one they can hold: seconds, not 126184.
    #[test]
    fn durations_read_like_durations() {
        assert_eq!(secs(450), "450 ms");
        assert_eq!(secs(3_000), "3.0 s");
        assert_eq!(secs(126_184), "126 s");
    }
}

#[cfg(test)]
mod find_tests {
    use super::*;

    /// The seam `find` ties: the tree scanned is the store's parent, the store itself is
    /// kept out of it, and the report serialises with the field names the CLI, HTTP and
    /// MCP surfaces all print.
    #[test]
    fn find_scans_the_project_and_not_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(
            project.join("src/lib.rs"),
            "/// Greets.\npub fn greet() -> &'static str {\n    \"hi\"\n}\n\nfn main() {\n    greet();\n}\n",
        )
        .unwrap();
        let store = project.join(DEFAULT_STORE_DIR);
        App::init(&store, &Actor::Cli).unwrap();
        // A note whose heading is the same word must not come back from `find`.
        std::fs::write(store.join("notes/r2/greet.md"), "# greet\n\nnot code\n").unwrap();

        let app = App::open(Some(&store), Actor::Cli).unwrap();
        let r = app.find("greet", 10).unwrap();
        assert_eq!(r.root, std::path::absolute(&project).unwrap());
        assert_eq!(r.hits.len(), 1, "{:?}", r.hits);
        let h = &r.hits[0];
        assert_eq!(
            (
                h.path.as_str(),
                h.kind,
                h.language,
                h.start_line,
                h.line,
                h.end_line,
                h.matched
            ),
            ("src/lib.rs", "function", "rust", 1, 2, 4, "exact")
        );
        assert_eq!(
            r.skipped.store_entries, 1,
            "the store is excluded, not merely hidden"
        );
        assert_eq!(r.files_scanned, 1);
        assert!(!r.truncated);
        assert!(
            r.caveats.iter().any(|c| c.contains(".cyberbrainignore")),
            "no ignore file in the project: the report must say so: {:?}",
            r.caveats
        );

        let v = serde_json::to_value(&r).unwrap();
        for key in [
            "symbol",
            "name",
            "scope",
            "root",
            "hits",
            "matched_total",
            "truncated",
            "limit",
            "files_scanned",
            "bytes_scanned",
            "definitions_indexed",
            "skipped",
            "ignore_files",
            "caveats",
            "elapsed_ms",
        ] {
            assert!(v.get(key).is_some(), "FindReport lacks `{key}`");
        }
        for key in [
            "path",
            "start_line",
            "end_line",
            "line",
            "kind",
            "language",
            "name",
            "scope",
            "matched",
            "snippet",
        ] {
            assert!(v["hits"][0].get(key).is_some(), "FindHit lacks `{key}`");
        }

        let e = app.find("", 10).unwrap_err();
        assert_eq!(e.exit_code(), 1);
    }
}

#[cfg(test)]
mod path_rendering_tests {
    use super::*;
    use cyberbrain_core::slash;

    /// A doctor finding names the file it is about, and the name is spelt with forward
    /// slashes on every platform: the report is pasted and diffed, not opened.
    #[test]
    fn doctor_findings_spell_paths_with_forward_slashes() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        App::init(&store, &Actor::Operator).unwrap();
        let stray = store.join("notes").join("r2").join("stray.txt");
        std::fs::write(&stray, "not a note").unwrap();
        let app = App::open(Some(&store), Actor::Operator).unwrap();
        let r = app.doctor().unwrap();
        let f = r
            .findings
            .iter()
            .find(|f| f.check == "notes tree")
            .unwrap_or_else(|| panic!("{r:?}"));
        assert!(!f.detail.contains('\\'), "{}", f.detail);
        assert!(f.detail.contains(&slash(&stray)), "{}", f.detail);

        // The JSON form says the same thing as the human form.
        let v = serde_json::to_value(&r).unwrap();
        for finding in v["findings"].as_array().unwrap() {
            let detail = finding["detail"].as_str().unwrap();
            assert!(!detail.contains('\\'), "{detail}");
        }
    }
}
