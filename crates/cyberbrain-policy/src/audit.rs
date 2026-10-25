//! The audit log (SPEC §12.6): append-only, exportable, and the evidence an AI Act
//! conformity discussion asks for.
//!
//! # Shape
//!
//! Rows match the table in SPEC §5: `audit(ts, actor, action, subject, detail)`. The storage
//! lives in `cyberbrain-index` (`Index::append_audit` / `audit` / `last_audit`); this module
//! defines the row ([`AuditEvent`]), the vocabulary ([`AuditAction`], [`Actor`]) and the seam
//! ([`AuditSink`]) the binary implements over the index. The index stamps `ts` itself in
//! SQLite; this crate's chain therefore anchors on its own timestamp inside `detail`, so the
//! two never disagree about what was hashed.
//!
//! # Append-only, enforced twice
//!
//! The index refuses `UPDATE` and `DELETE` by trigger (SPEC §4). [`AuditSink`] additionally
//! has no update and no delete, which keeps *this* code honest. Neither stops someone with
//! a hex editor, so [`AuditLog`] chains rows: every row's `detail` carries
//! `_chain.{prev, hash, at}`, where `hash` is blake3 over the row and `prev`.
//! [`AuditLog::verify`] walks the log and names the first row where the chain breaks. A
//! deleted, reordered or edited row is therefore detectable, which is the most an
//! application-level log can offer.
//!
//! # What must never be in here
//!
//! The matched text of a PII finding. The audit log records *that* an email address was
//! found at bytes 40..62 of a note, never *which* one; otherwise the audit log becomes a
//! second copy of the data it exists to govern. The same rule applies to the subject-access
//! command, which logs a hash of the identifier, not the identifier. And token counts an
//! endpoint did not report stay absent: an estimate in an audit log reads as a measurement.

use crate::audit_key::{AuditKey, ROW_DOMAIN, fingerprint_of, verify_signature};
use cyberbrain_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::sync::{Arc, Mutex};

/// The chain format of a row signed with the store's key: `_chain.v` is 2, the hash is over
/// length-prefixed fields (see [`AuditEvent::chain_version`]), and `_chain.key` and
/// `_chain.sig` carry the public key and the signature. Rows without `v` are format 1 and
/// keep the rule they were written with.
pub const CHAIN_V2: u64 = 2;

/// The first field of every format-2 hash, so it can never equal a hash of anything else.
const HASH_V2_DOMAIN: &str = "cyberbrain.audit.v2";

/// Who did the thing. Stored as text (`actor` column) via [`fmt::Display`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "name")]
pub enum Actor {
    /// A human at the CLI or the web UI.
    Operator,
    /// An agent acting through a hook or MCP; `name` is whatever the harness reports.
    Agent(String),
    /// The CLI itself, e.g. `scan` writing links back.
    Cli,
    /// `cyberbrain hook <event>`.
    Hook(String),
    /// The MCP server.
    Mcp,
    /// The retention apply path. Never runs unattended; it still names itself.
    Retention,
    /// Anything else, named.
    System(String),
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Actor::Operator => f.write_str("operator"),
            Actor::Agent(n) => write!(f, "agent:{n}"),
            Actor::Cli => f.write_str("cli"),
            Actor::Hook(e) => write!(f, "hook:{e}"),
            Actor::Mcp => f.write_str("mcp"),
            Actor::Retention => f.write_str("retention"),
            Actor::System(n) => write!(f, "system:{n}"),
        }
    }
}

/// The closed vocabulary of the `action` column. Strings are stable: they are what an
/// external consumer (AgentGuard, an auditor's grep) keys on. The index writes its own
/// `index.*` actions alongside these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditAction {
    NoteWrite,
    NoteWriteHeld,
    NoteWriteResolved,
    /// Written to `proposals/`, waiting for somebody else.
    NoteProposed,
    /// A proposal became a note. The row names both people.
    NoteProposalAccepted,
    /// A proposal was turned down. The reason is in the row, because it is the only place
    /// the proposer will look for it.
    NoteProposalRejected,
    NoteEraseRequested,
    NoteEraseCompleted,
    NoteEraseFailed,
    /// The gate permitted a request. Written by `permit` before it returns `Ok`.
    EgressPermitted,
    EgressCompleted,
    EgressFailed,
    /// A ticket was dropped without being closed. Usually a panic or an early return in
    /// the caller; either way the bytes may or may not have gone out, and the log says so.
    EgressAbandoned,
    PolicyRefusal,
    InferenceCall,
    SubjectAccess,
    RetentionExpired,
    AuditExport,
}

impl AuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditAction::NoteWrite => "note.write",
            AuditAction::NoteWriteHeld => "note.write.held",
            AuditAction::NoteWriteResolved => "note.write.resolved",
            AuditAction::NoteProposed => "note.proposed",
            AuditAction::NoteProposalAccepted => "note.proposal.accepted",
            AuditAction::NoteProposalRejected => "note.proposal.rejected",
            AuditAction::NoteEraseRequested => "note.erase.requested",
            AuditAction::NoteEraseCompleted => "note.erase.completed",
            AuditAction::NoteEraseFailed => "note.erase.failed",
            AuditAction::EgressPermitted => "egress.permitted",
            AuditAction::EgressCompleted => "egress.completed",
            AuditAction::EgressFailed => "egress.failed",
            AuditAction::EgressAbandoned => "egress.abandoned",
            AuditAction::PolicyRefusal => "policy.refusal",
            AuditAction::InferenceCall => "inference.call",
            AuditAction::SubjectAccess => "subject.access",
            AuditAction::RetentionExpired => "retention.expired",
            AuditAction::AuditExport => "audit.export",
        }
    }
}

impl fmt::Display for AuditAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One row of the audit table. `action` is a string on purpose: rows written by a newer
/// binary must still read back in an older one, and the index writes its own actions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// When the row was written. Set by this crate on write; the index may overwrite it
    /// with its own stamp on read-back. Not part of the chain hash for that reason.
    pub ts: jiff::Timestamp,
    pub actor: String,
    pub action: String,
    pub subject: String,
    /// JSON object. Carries `_chain: {prev, hash, at}` once it has been through [`AuditLog`].
    pub detail: Value,
}

impl AuditEvent {
    pub fn chain_hash(&self) -> Option<&str> {
        self.detail.get("_chain")?.get("hash")?.as_str()
    }

    pub fn chain_prev(&self) -> Option<&str> {
        self.detail.get("_chain")?.get("prev")?.as_str()
    }

    /// The timestamp this crate wrote into the chain, independent of the `ts` column.
    pub fn chain_at(&self) -> Option<&str> {
        self.detail.get("_chain")?.get("at")?.as_str()
    }

    /// Which hash rule the row was written with.
    ///
    /// 1, the original, separates the fields with a newline. Since a field may itself hold a
    /// newline, moving a boundary — `actor "a\nb", action "c"` against `actor "a", action
    /// "b\nc"` — gives the same hash (K1). That was harmless while anybody could recompute
    /// the chain anyway; with signatures it would let a signed row say something else, so
    /// format 2 prefixes every field with its length. Rows of format 1 are checked by their
    /// own rule for ever: changing it would turn every existing log into a forgery.
    pub fn chain_version(&self) -> u64 {
        self.detail
            .get("_chain")
            .and_then(|c| c.get("v"))
            .and_then(Value::as_u64)
            .unwrap_or(1)
    }

    /// The public key (hex) a format-2 row was signed with.
    pub fn chain_key(&self) -> Option<&str> {
        self.detail.get("_chain")?.get("key")?.as_str()
    }

    pub fn chain_sig(&self) -> Option<&str> {
        self.detail.get("_chain")?.get("sig")?.as_str()
    }

    /// The detail without the chain block, for display and for hashing.
    pub fn detail_without_chain(&self) -> Value {
        let mut d = self.detail.clone();
        if let Value::Object(m) = &mut d {
            m.remove("_chain");
        }
        d
    }

    /// Recompute the hash from the row's content, `prev` and the chained timestamp.
    fn compute_hash(&self, prev: &str, at: &str) -> String {
        let mut h = blake3::Hasher::new();
        for part in [prev, at, &self.actor, &self.action, &self.subject] {
            h.update(part.as_bytes());
            h.update(b"\n");
        }
        // serde_json's Value::Object is a BTreeMap without `preserve_order`, so this
        // serialisation is canonical for our purposes.
        h.update(self.detail_without_chain().to_string().as_bytes());
        h.finalize().to_hex().to_string()
    }

    /// The format-2 hash: every field, the domain tag first, as an 8-byte little-endian
    /// length and then its bytes. No separator can be forged, because there is none.
    fn compute_hash_v2(&self, prev: &str, at: &str) -> String {
        let detail = self.detail_without_chain().to_string();
        let mut h = blake3::Hasher::new();
        for part in [
            HASH_V2_DOMAIN,
            prev,
            at,
            &self.actor,
            &self.action,
            &self.subject,
            &detail,
        ] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part.as_bytes());
        }
        h.finalize().to_hex().to_string()
    }
}

/// Selection for reading the log back. All fields optional and ANDed. Mirrors the index's
/// own filter so the adapter is a field-by-field copy.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Inclusive lower bound.
    pub since: Option<jiff::Timestamp>,
    /// Inclusive upper bound. With `since`, this is how a period is asked for — and a
    /// period is what an auditor asks about, never "everything since the beginning".
    pub until: Option<jiff::Timestamp>,
    /// Exact match on `action`.
    pub action: Option<String>,
    /// Exact match on `subject`.
    pub subject: Option<String>,
    /// Case-insensitive substring over actor, action, subject and detail. What
    /// `policy subject <identifier>` uses (SPEC §12.3).
    pub contains: Option<String>,
    pub limit: Option<usize>,
}

impl AuditFilter {
    pub fn matches(&self, e: &AuditEvent) -> bool {
        if let Some(s) = self.since
            && e.ts < s
        {
            return false;
        }
        if let Some(u) = self.until
            && e.ts > u
        {
            return false;
        }
        if let Some(a) = &self.action
            && &e.action != a
        {
            return false;
        }
        if let Some(s) = &self.subject
            && &e.subject != s
        {
            return false;
        }
        if let Some(c) = &self.contains {
            let hay = format!("{} {} {} {}", e.actor, e.action, e.subject, e.detail).to_lowercase();
            if !hay.contains(&c.to_lowercase()) {
                return false;
            }
        }
        true
    }
}

/// The storage seam. The binary implements it over `cyberbrain_index::Index`
/// (`append_audit`, `audit`, `last_audit`), which needs `&mut` for appends, so the adapter
/// holds the index behind a mutex.
///
/// Deliberately has no update and no delete. Implementers must:
///
/// - return rows from `read` in insertion order;
/// - store `detail` as the JSON text of the value, verbatim, including `_chain`;
/// - make `append` durable before returning `Ok` (the egress gate proceeds on `Ok`);
/// - serialise `last` + `append` against concurrent writers where they can (a
///   `BEGIN IMMEDIATE` around the pair in SQLite). Without that, two processes appending
///   at the same instant can fork the chain, which `verify` reports as a break. That is a
///   false alarm, not a hole, and it is the adapter's job to avoid it.
pub trait AuditSink: Send + Sync {
    fn append(&self, event: &AuditEvent) -> Result<()>;
    fn read(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>>;
    fn last(&self) -> Result<Option<AuditEvent>>;
}

/// In-memory sink for tests and for `--dry-run` (SPEC §8: the real code path with a no-op
/// writer). Rows appended here are visible to `read` within the process and gone after it.
#[derive(Default)]
pub struct MemoryAuditSink {
    rows: Mutex<Vec<AuditEvent>>,
    /// Test hook: make the next `append` fail, to prove callers fail closed.
    fail_next_append: Mutex<bool>,
}

impl MemoryAuditSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail_next_append(&self) {
        *self.fail_next_append.lock().unwrap() = true;
    }

    pub fn len(&self) -> usize {
        self.rows.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn rows(&self) -> Vec<AuditEvent> {
        self.rows.lock().unwrap().clone()
    }

    /// The `action` column of every row, in order. The shape most tests assert on.
    pub fn actions(&self) -> Vec<String> {
        self.rows().into_iter().map(|e| e.action).collect()
    }
}

impl AuditSink for MemoryAuditSink {
    fn append(&self, event: &AuditEvent) -> Result<()> {
        let mut fail = self.fail_next_append.lock().unwrap();
        if *fail {
            *fail = false;
            return Err(Error::Index(
                "audit sink refused the append (test hook)".into(),
            ));
        }
        self.rows.lock().unwrap().push(event.clone());
        Ok(())
    }

    fn read(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        let rows = self.rows.lock().unwrap();
        let it = rows.iter().filter(|e| filter.matches(e)).cloned();
        Ok(match filter.limit {
            Some(n) => it.take(n).collect(),
            None => it.collect(),
        })
    }

    fn last(&self) -> Result<Option<AuditEvent>> {
        Ok(self.rows.lock().unwrap().last().cloned())
    }
}

/// Hash of "nothing before this row".
pub const GENESIS: &str = "genesis";

/// How many times a row is rebuilt against a moved chain head before giving up.
///
/// Eight, because that is roughly the number of writers this tool has running at once on a
/// busy machine — a hook, an MCP server, a terminal or two — and every retry after the
/// first has one fewer competitor. Bounded rather than endless: if it still cannot get in
/// after eight, something other than ordinary contention is going on, and looping forever
/// would hide it.
const CHAIN_TRIES: u32 = 8;

/// A short, growing, slightly uneven pause between attempts.
///
/// Growing so that a burst spreads out instead of colliding again, and uneven because
/// writers that arrived together would otherwise wake together and race in lockstep. The
/// jitter comes from the clock rather than a random number generator: this crate has no
/// business pulling in one for a sleep, and the low bits of a nanosecond reading are as
/// uncorrelated between processes as anything a seed would give us.
fn back_off(attempt: u32) {
    let jitter = (jiff::Timestamp::now().as_nanosecond() % 2_000).unsigned_abs() as u64;
    let micros = u64::from(attempt) * 2_000 + jitter;
    std::thread::sleep(std::time::Duration::from_micros(micros));
}

/// The writer. Cheap to clone (it is an `Arc` around the sink); hand clones to the egress
/// gate, the write gate and the retention path.
#[derive(Clone)]
pub struct AuditLog {
    sink: Arc<dyn AuditSink>,
    /// Serialises `last` + `append` within this process so the chain cannot fork here.
    write_lock: Arc<Mutex<()>>,
    /// The store's key, when it has one. Every row appended through this log is then signed
    /// and written in chain format 2; without one, rows stay in format 1 as before.
    key: Option<Arc<AuditKey>>,
}

impl AuditLog {
    pub fn new(sink: Arc<dyn AuditSink>) -> Self {
        Self {
            sink,
            write_lock: Arc::new(Mutex::new(())),
            key: None,
        }
    }

    /// Sign every row appended from now on with `key`. Set before the log is cloned: the
    /// egress gate and the write gate each hold a clone, and they must sign alike.
    pub fn with_key(mut self, key: Option<AuditKey>) -> Self {
        self.key = key.map(Arc::new);
        self
    }

    pub fn key(&self) -> Option<&AuditKey> {
        self.key.as_deref()
    }

    pub fn in_memory() -> (Self, Arc<MemoryAuditSink>) {
        let sink = Arc::new(MemoryAuditSink::new());
        (Self::new(sink.clone()), sink)
    }

    /// Append one row, chained to the previous one. Returns the stored event.
    ///
    /// Errors propagate: a caller that cannot record what it is about to do must not do it.
    pub fn record(
        &self,
        actor: &Actor,
        action: AuditAction,
        subject: impl Into<String>,
        detail: Value,
    ) -> Result<AuditEvent> {
        self.record_raw(&actor.to_string(), action.as_str(), subject, detail)
    }

    /// As [`record`](Self::record) with free-form actor and action strings, for rows that
    /// originate in another crate's vocabulary.
    pub fn record_raw(
        &self,
        actor: &str,
        action: &str,
        subject: impl Into<String>,
        detail: Value,
    ) -> Result<AuditEvent> {
        let detail = match detail {
            Value::Object(m) => Value::Object(m),
            Value::Null => json!({}),
            other => json!({ "value": other }),
        };
        let _guard = self.write_lock.lock().map_err(|_| {
            Error::Index("audit write lock poisoned; refusing to append (fail closed)".into())
        })?;
        let subject = subject.into();
        // Reading the chain head and appending against it are two steps, and the lock above
        // only holds the second process in *this* one. Another process — a hook, an MCP
        // server, a second terminal — can append in between, and the sink then refuses this
        // row because its `prev` no longer names the head. That refusal is correct and it is
        // also transient: read the head again, hash against it, offer the row again.
        //
        // It has to be retried here rather than left to the caller, because by the time a
        // caller sees the error it has usually already done the thing it was recording. A
        // note on disk that the chain does not mention is the one state this log exists to
        // rule out, and it was reachable with two terminals and one unlucky moment.
        let mut last_seq = 0;
        for attempt in 1..=CHAIN_TRIES {
            let prev = self
                .sink
                .last()?
                .and_then(|e| e.chain_hash().map(str::to_owned))
                .unwrap_or_else(|| GENESIS.to_string());
            // Truncated to milliseconds, which is the precision an audit store can hold.
            // Keeping more here gives one event two timestamps: the one a caller is handed
            // back and the coarser one that was written. A `since` filter built from the
            // returned value would then skip the very row it came from, and a reader would
            // conclude the log had a gap where it has none.
            let now = jiff::Timestamp::now()
                .round(jiff::Unit::Millisecond)
                .unwrap_or_else(|_| jiff::Timestamp::now());
            let at = now.to_string();
            let mut event = AuditEvent {
                ts: now,
                actor: actor.to_string(),
                action: action.to_string(),
                subject: subject.clone(),
                detail: detail.clone(),
            };
            let chain = match &self.key {
                None => {
                    let hash = event.compute_hash(&prev, &at);
                    json!({ "prev": prev, "hash": hash, "at": at })
                }
                Some(key) => {
                    let hash = event.compute_hash_v2(&prev, &at);
                    let sig = key.sign(ROW_DOMAIN, hash.as_bytes());
                    json!({
                        "prev": prev,
                        "hash": hash,
                        "at": at,
                        "v": CHAIN_V2,
                        "key": key.public_hex(),
                        "sig": sig,
                    })
                }
            };
            let mut chained = detail.clone();
            if let Value::Object(m) = &mut chained {
                m.insert("_chain".into(), chain);
            }
            event.detail = chained;
            match self.sink.append(&event) {
                Ok(()) => return Ok(event),
                Err(Error::AuditContended { seq, .. }) => {
                    last_seq = seq;
                    back_off(attempt);
                }
                Err(e) => return Err(e),
            }
        }
        Err(Error::AuditContended {
            seq: last_seq,
            tries: CHAIN_TRIES,
        })
    }

    /// SPEC §11: model, endpoint and token counts of every inference call. Counts the
    /// endpoint did not report are recorded as absent, never estimated.
    #[allow(clippy::too_many_arguments)]
    pub fn record_inference(
        &self,
        actor: &Actor,
        endpoint: &str,
        model: &str,
        outcome: &str,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        total_tokens: Option<u64>,
        extra: Value,
    ) -> Result<AuditEvent> {
        let mut detail = json!({
            "endpoint": endpoint,
            "model": model,
            "outcome": outcome,
            "prompt_tokens": prompt_tokens,
            "completion_tokens": completion_tokens,
            "total_tokens": total_tokens,
        });
        if let (Value::Object(d), Value::Object(e)) = (&mut detail, extra) {
            for (k, v) in e {
                d.entry(k).or_insert(v);
            }
        }
        self.record(
            actor,
            AuditAction::InferenceCall,
            format!("inference:{endpoint}"),
            detail,
        )
    }

    pub fn read(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        self.sink.read(filter)
    }

    /// Walk the whole log and check the chain. `Ok(n)` is the number of rows verified.
    pub fn verify(&self) -> Result<usize> {
        self.verify_report().map(|r| r.rows)
    }

    /// [`verify`](Self::verify), and what it found about signatures.
    pub fn verify_report(&self) -> Result<ChainReport> {
        let rows = self.sink.read(&AuditFilter::default())?;
        verify_chain_report(GENESIS, &rows)
    }

    /// A period of the log as a self-checking bundle: header with the anchor, the rows, a
    /// footer. See [`bundle`] for the format and why it exists.
    ///
    /// Recorded like the plain export, and for the same reason — an extract is an access,
    /// and an access that leaves no trace is the one nobody can ask about later.
    pub fn export_bundle(&self, actor: &Actor, filter: &AuditFilter, tool: &str) -> Result<String> {
        self.export_bundle_with(actor, filter, tool, false)
    }

    /// [`export_bundle`](Self::export_bundle), signed with the store's key when `signed`.
    /// Asking for a signature without a key is an error, not an unsigned file: somebody who
    /// asked for one is about to hand the file on as signed.
    pub fn export_bundle_with(
        &self,
        actor: &Actor,
        filter: &AuditFilter,
        tool: &str,
        signed: bool,
    ) -> Result<String> {
        let key = match (signed, self.key()) {
            (false, _) => None,
            (true, Some(k)) => Some(k),
            (true, None) => {
                return Err(Error::Config(
                    "this store has no audit key to sign with; create one with \
                     `cyberbrain policy audit-key --init`"
                        .into(),
                ));
            }
        };
        let rows = self.sink.read(filter)?;
        let out = bundle::render_with(
            &rows,
            &bundle::Spec {
                from: filter.since.map(|t| t.to_string()),
                to: filter.until.map(|t| t.to_string()),
                tool: tool.to_string(),
                exported_at: jiff::Timestamp::now().to_string(),
                selection: bundle::Selection::of(filter),
                key,
            },
        );
        self.record(
            actor,
            AuditAction::AuditExport,
            "audit",
            json!({ "rows": rows.len(), "format": "bundle", "signed": key.is_some() }),
        )?;
        Ok(out)
    }

    /// Export the log (SPEC §12.6). The export itself is recorded, after the rows have been
    /// rendered, so the rendered output does not contain its own row.
    pub fn export(
        &self,
        actor: &Actor,
        filter: &AuditFilter,
        format: ExportFormat,
    ) -> Result<String> {
        let rows = self.sink.read(filter)?;
        let out = render(&rows, format);
        self.record(
            actor,
            AuditAction::AuditExport,
            "audit",
            json!({ "rows": rows.len(), "format": format.as_str() }),
        )?;
        Ok(out)
    }
}

/// A period of the log, in a form somebody else can check (SPEC §12.6).
///
/// # Why this is not just the rows
///
/// Rows on their own are not evidence. `verify_chain` starts at the genesis marker, and an
/// extract from the middle of a log has no genesis in it — so a bare export can be checked
/// for internal consistency and nothing else, and the check that matters ("is this the log
/// that machine actually wrote") has nowhere to start.
///
/// The bundle adds the two things that make it checkable: the **anchor**, which is the hash
/// the first row points back at, and a **footer** naming the count and the last hash, so a
/// truncated file is a failed check rather than a shorter answer.
///
/// # The format, which is the point
///
/// One JSON object per line, in this order: header, rows, footer. Nothing is nested, no
/// field is computed from another file, and the hash is blake3 over
/// `prev \n at \n actor \n action \n subject \n detail-without-_chain`. Ten years is longer
/// than most programs live; anyone should be able to rewrite the check in an afternoon, in
/// whatever language is normal by then. `scripts/verify-audit-export.py` is that check,
/// written once as proof it can be.
pub mod bundle {
    use super::{
        AuditEvent, AuditFilter, CHAIN_V2, ChainReport, GENESIS, verify_chain_report,
        verify_each_report,
    };
    use crate::audit_key::{
        AuditKey, BUNDLE_DOMAIN, fingerprint_of, key_matches, verify_signature,
    };
    use cyberbrain_core::{Error, Result};
    use serde::{Deserialize, Serialize};

    pub const KIND: &str = "cyberbrain.audit.export";
    pub const KIND_END: &str = "cyberbrain.audit.export.end";
    /// The format every bundle had before selections and signatures, and still the one
    /// written when a bundle needs neither, so a reader that only knows it keeps working.
    pub const VERSION: u32 = 1;
    /// Adds the selection (`filter` in the header), the signature (`key` in the header,
    /// `sig` in the footer) and rows of chain format 2. Written only when a bundle uses one
    /// of them.
    pub const VERSION_2: u32 = 2;

    /// Which rows of the period a bundle holds, when it is not all of them.
    ///
    /// The rows of one action are not consecutive in the chain, so a selection cannot be
    /// walked as a chain: it is checked row by row, each against the hash it names as its
    /// predecessor. That proves every row is as it was written. It cannot prove that the
    /// selection is complete, which is why the header says it is one.
    #[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
    pub struct Selection {
        /// The action, or its family: `note` selects `note.*`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub action: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub subject: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub contains: Option<String>,
    }

    impl Selection {
        /// The selection a filter makes, or `None` when it selects by period alone.
        pub fn of(filter: &AuditFilter) -> Option<Selection> {
            let s = Selection {
                action: filter.action.clone(),
                subject: filter.subject.clone(),
                contains: filter.contains.clone(),
            };
            (s != Selection::default()).then_some(s)
        }

        /// Whether a row belongs in this selection. The same rules the store's reader
        /// applies, so a bundle it wrote passes and a row slipped in from elsewhere does not.
        pub fn admits(&self, e: &AuditEvent) -> bool {
            if let Some(a) = &self.action
                && !(e.action == *a || e.action.starts_with(&format!("{a}.")))
            {
                return false;
            }
            if let Some(s) = &self.subject
                && e.subject != *s
            {
                return false;
            }
            if let Some(c) = &self.contains {
                let hay =
                    format!("{} {} {} {}", e.actor, e.action, e.subject, e.detail).to_lowercase();
                if !hay.contains(&c.to_lowercase()) {
                    return false;
                }
            }
            true
        }
    }

    /// Everything about a bundle that is not its rows.
    #[derive(Debug, Clone, Default)]
    pub struct Spec<'k> {
        pub from: Option<String>,
        pub to: Option<String>,
        pub tool: String,
        pub exported_at: String,
        pub selection: Option<Selection>,
        /// Sign the bundle with this key.
        pub key: Option<&'k AuditKey>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Header {
        pub kind: String,
        pub version: u32,
        /// What wrote the file. Informational: nothing in the check depends on it.
        pub tool: String,
        pub exported_at: String,
        /// Requested period. `None` on either side means unbounded there.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub from: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub to: Option<String>,
        /// Hash the first row points back at, or `genesis` when the extract starts at the
        /// beginning of the log.
        pub anchor: String,
        pub rows: usize,
        /// Present when the bundle holds a selection rather than the whole period.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub filter: Option<Selection>,
        /// The public key (hex) the footer's signature was made with. Present when signed.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub key: Option<String>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Footer {
        pub kind: String,
        pub rows: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub first_hash: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_hash: Option<String>,
        /// Ed25519 over the domain tag and every byte of the file before this line.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub sig: Option<String>,
    }

    /// What a check found. `rows` is what it verified, not what the file claimed.
    #[derive(Debug, Clone, Serialize, PartialEq)]
    pub struct Report {
        pub rows: usize,
        pub anchor: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub from: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub to: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_hash: Option<String>,
        pub tool: String,
        pub exported_at: String,
        pub version: u32,
        /// The selection the rows were taken by, when they are not the whole period. The
        /// rows were then checked one by one; gaps between them are expected.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub filter: Option<Selection>,
        /// The public key the bundle is signed with, when it is. The signature holds; whether
        /// this is the key you expected is [`verify_expecting`]'s question.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub fingerprint: Option<String>,
        /// Signed and unsigned rows, and the keys of the signed ones.
        pub chain: ChainReport,
    }

    /// Render rows as a bundle. `tool` names the writer, `exported_at` is its own timestamp.
    pub fn render(
        rows: &[AuditEvent],
        from: Option<String>,
        to: Option<String>,
        tool: &str,
        exported_at: &str,
    ) -> String {
        render_with(
            rows,
            &Spec {
                from,
                to,
                tool: tool.to_string(),
                exported_at: exported_at.to_string(),
                selection: None,
                key: None,
            },
        )
    }

    /// [`render`], with everything a bundle can say about itself.
    pub fn render_with(rows: &[AuditEvent], spec: &Spec) -> String {
        // The anchor comes out of the first row rather than a second read of the log: a row
        // already carries the hash it follows. One source, no chance of the two disagreeing.
        let anchor = rows
            .first()
            .and_then(|r| r.chain_prev())
            .unwrap_or(GENESIS)
            .to_string();
        // The lowest version that can say what this bundle says. A plain period stays at 1,
        // so a reader written against 1 checks it as before.
        let version = if spec.selection.is_some()
            || spec.key.is_some()
            || rows.iter().any(|r| r.chain_version() >= CHAIN_V2)
        {
            VERSION_2
        } else {
            VERSION
        };
        let header = Header {
            kind: KIND.to_string(),
            version,
            tool: spec.tool.clone(),
            exported_at: spec.exported_at.clone(),
            from: spec.from.clone(),
            to: spec.to.clone(),
            anchor,
            rows: rows.len(),
            filter: spec.selection.clone(),
            key: spec.key.map(AuditKey::public_hex),
        };
        let mut footer = Footer {
            kind: KIND_END.to_string(),
            rows: rows.len(),
            first_hash: rows.first().and_then(|r| r.chain_hash()).map(str::to_owned),
            last_hash: rows.last().and_then(|r| r.chain_hash()).map(str::to_owned),
            sig: None,
        };

        let mut s = serde_json::to_string(&header).expect("header serialises");
        s.push('\n');
        for r in rows {
            s.push_str(&serde_json::to_string(r).expect("AuditEvent serialises"));
            s.push('\n');
        }
        // The signature covers the bytes as written, not a re-serialisation: a checker in
        // another language then needs no JSON rules to check it, only the file.
        footer.sig = spec.key.map(|k| k.sign(BUNDLE_DOMAIN, s.as_bytes()));
        s.push_str(&serde_json::to_string(&footer).expect("footer serialises"));
        s.push('\n');
        s
    }

    /// Check a bundle on its own: no store, no configuration, no network.
    ///
    /// Everything it needs is in the file, which is the property that has to survive the
    /// next ten years.
    pub fn verify(text: &str) -> Result<Report> {
        verify_rows(text).map(|(report, _)| report)
    }

    /// [`verify`], and the rows it checked.
    ///
    /// A receiver needs both: the verdict to decide whether to accept, and the rows to keep
    /// if it does. Parsing twice would be two chances to disagree with itself.
    pub fn verify_rows(text: &str) -> Result<(Report, Vec<AuditEvent>)> {
        let bad = |m: String| Error::Index(format!("audit export: {m}"));

        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        let header_line = lines.next().ok_or_else(|| bad("file is empty".into()))?;
        let header: Header = serde_json::from_str(header_line)
            .map_err(|e| bad(format!("first line is not a header: {e}")))?;
        if header.kind != KIND {
            return Err(bad(format!(
                "first line is {:?}, expected {KIND:?}",
                header.kind
            )));
        }
        if header.version != VERSION && header.version != VERSION_2 {
            return Err(bad(format!(
                "format version {} is not {VERSION} or {VERSION_2}; use a reader that knows it",
                header.version
            )));
        }
        // A version-1 reader would walk a selection as a chain and fail, so a selection in a
        // file that claims version 1 is a file somebody edited.
        if header.version == VERSION && header.filter.is_some() {
            return Err(bad(
                "a version 1 file carries a selection, which version 1 cannot express".into(),
            ));
        }
        if header.version == VERSION && header.key.is_some() {
            return Err(bad(
                "a version 1 file carries a signing key, which version 1 cannot express".into(),
            ));
        }

        let rest: Vec<&str> = lines.collect();
        let (footer_line, row_lines) = rest
            .split_last()
            .ok_or_else(|| bad("no footer; the file is truncated".into()))?;
        let footer: Footer = serde_json::from_str(footer_line)
            .map_err(|e| bad(format!("last line is not a footer: {e}")))?;
        if footer.kind != KIND_END {
            return Err(bad(format!(
                "last line is {:?}, expected {KIND_END:?}; the file is truncated",
                footer.kind
            )));
        }

        let mut rows = Vec::with_capacity(row_lines.len());
        for (i, l) in row_lines.iter().enumerate() {
            rows.push(
                serde_json::from_str::<AuditEvent>(l)
                    .map_err(|e| bad(format!("row {} is not an audit row: {e}", i + 1)))?,
            );
        }

        // Three claims, checked in the order in which a mismatch is most informative.
        if header.rows != rows.len() {
            return Err(bad(format!(
                "header says {} rows, file carries {}",
                header.rows,
                rows.len()
            )));
        }
        if footer.rows != rows.len() {
            return Err(bad(format!(
                "footer says {} rows, file carries {}",
                footer.rows,
                rows.len()
            )));
        }
        // The signature first: if it fails, nothing else in the file is worth reporting.
        match (&header.key, &footer.sig) {
            (None, None) => {}
            (Some(key), Some(sig)) => {
                let body_end =
                    text.trim_end().rfind('\n').map(|i| i + 1).ok_or_else(|| {
                        bad("a signed file needs a footer line of its own".into())
                    })?;
                verify_signature(key, BUNDLE_DOMAIN, &text.as_bytes()[..body_end], sig)
                    .map_err(|e| bad(format!("{e}; the file was changed after it was signed")))?;
            }
            (Some(_), None) => {
                return Err(bad(
                    "the header names a signing key, but the footer carries no signature; it \
                     was removed"
                        .into(),
                ));
            }
            (None, Some(_)) => {
                return Err(bad(
                    "the footer carries a signature, but the header names no key to check it with"
                        .into(),
                ));
            }
        }

        let chain = match &header.filter {
            None => verify_chain_report(&header.anchor, &rows)?,
            Some(sel) => {
                let first_prev = rows.first().and_then(|r| r.chain_prev()).unwrap_or(GENESIS);
                if first_prev != header.anchor {
                    return Err(bad(
                        "the anchor is not the hash the first row follows".into()
                    ));
                }
                if let Some(i) = rows.iter().position(|r| !sel.admits(r)) {
                    return Err(bad(format!(
                        "row {} does not belong to the selection the header names",
                        i + 1
                    )));
                }
                verify_each_report(&rows)?
            }
        };
        let verified = chain.rows;

        let last_hash = rows.last().and_then(|r| r.chain_hash()).map(str::to_owned);
        if footer.last_hash != last_hash {
            return Err(bad(
                "footer's last hash is not the last row's hash; rows were removed from the end"
                    .into(),
            ));
        }

        Ok((
            Report {
                rows: verified,
                anchor: header.anchor,
                from: header.from,
                to: header.to,
                last_hash,
                tool: header.tool,
                exported_at: header.exported_at,
                version: header.version,
                filter: header.filter,
                fingerprint: header.key.as_deref().and_then(|k| fingerprint_of(k).ok()),
                key: header.key,
                chain,
            },
            rows,
        ))
    }

    /// [`verify`], and the file must be signed with the key `expected` names — its
    /// fingerprint or the whole public key — and so must every signed row in it.
    ///
    /// This is the check that makes a signature worth something. Without an expected key,
    /// a signature only shows that *somebody* signed; a forger signs too.
    pub fn verify_expecting(text: &str, expected: &str) -> Result<Report> {
        let report = verify(text)?;
        let bad = |m: String| Error::Index(format!("audit export: {m}"));
        let Some(key) = &report.key else {
            return Err(bad(format!(
                "the file is not signed, and key {expected} was expected"
            )));
        };
        if !key_matches(expected, key) {
            return Err(bad(format!(
                "the file is signed with key {}, not with the expected {expected}",
                report.fingerprint.as_deref().unwrap_or(key)
            )));
        }
        for (k, fp) in report.chain.keys.iter().zip(report.chain.fingerprints()) {
            if !key_matches(expected, k) {
                return Err(bad(format!(
                    "rows in the file are signed with key {fp}, not with the expected {expected}"
                )));
            }
        }
        Ok(report)
    }
}

/// The inference layer hands its rows straight to the log (feature `llm`). Token counts
/// pass through as the endpoint reported them; `None` stays `null`.
#[cfg(feature = "llm")]
impl cyberbrain_llm::AuditSink for AuditLog {
    fn record_inference(&self, event: cyberbrain_llm::InferenceEvent) {
        let outcome = match &event.outcome {
            cyberbrain_llm::CallOutcome::Ok => "ok".to_string(),
            other => serde_json::to_value(other)
                .ok()
                .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
                .unwrap_or_else(|| "unknown".into()),
        };
        let usage = event.usage;
        let extra = json!({
            "purpose": event.purpose,
            "call": event.call,
            "task": event.task,
            "resolved": event.resolved,
            "public_waived": event.public_waived,
            "elapsed_ms": event.elapsed.as_millis() as u64,
            "outcome_detail": event.outcome,
            "cached_prompt_tokens": usage.and_then(|u| u.cached_prompt_tokens),
        });
        // The trait is infallible by design (the llm crate must never block on us). A
        // failed append is the one thing that cannot be recorded in the log itself; it goes
        // to stderr so it is at least not silent.
        if let Err(e) = self.record_inference(
            &Actor::System("llm".into()),
            &event.endpoint,
            &event.model,
            &outcome,
            usage.and_then(|u| u.prompt_tokens).map(u64::from),
            usage.and_then(|u| u.completion_tokens).map(u64::from),
            usage.and_then(|u| u.total_tokens).map(u64::from),
            extra,
        ) {
            eprintln!("cyberbrain: audit append failed for inference call: {e}");
        }
    }
}

/// Check a sequence of rows for chain integrity. Rows without `_chain` (written before
/// chaining existed, or by a foreign writer such as the index's own `index.*` rows) break
/// the chain and are reported as such, because silently skipping them would let an attacker
/// strip `_chain` from an edited row. See the report: the index writes unchained rows into
/// the same table, so a mixed log verifies only up to the first of those.
pub fn verify_chain(rows: &[AuditEvent]) -> Result<usize> {
    verify_chain_from(GENESIS, rows)
}

/// [`verify_chain`], starting from a hash other than the genesis marker.
///
/// This is what makes an extract checkable. A period taken out of the middle of a log has no
/// genesis in it; what it has is the hash its first row points back at. Hand that in as the
/// anchor and the arithmetic works exactly as it does over the whole log — with one honest
/// limit: it proves the extract is internally intact and starts where it says it does. That
/// the anchor itself belongs to the real history is a question only the full log answers.
pub fn verify_chain_from(anchor: &str, rows: &[AuditEvent]) -> Result<usize> {
    verify_chain_report(anchor, rows).map(|r| r.rows)
}

/// What a check found beyond "it holds": how many rows were signed, how many were written
/// before the store had a key, and with which keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChainReport {
    pub rows: usize,
    /// Format-1 rows: written before signing began, or in a store without a key. Not a
    /// fault — the chain still covers them — but nobody's signature does.
    pub unsigned: usize,
    /// Format-2 rows, each with a signature that holds.
    pub signed: usize,
    /// The public keys (hex) the signed rows carry, in the order they first appear. One,
    /// normally. Whether it is the *right* one is for the caller to decide against a key it
    /// trusts; a row can only prove which key signed it.
    pub keys: Vec<String>,
}

impl ChainReport {
    /// Fingerprints of [`keys`](Self::keys), for people.
    pub fn fingerprints(&self) -> Vec<String> {
        self.keys
            .iter()
            .map(|k| fingerprint_of(k).unwrap_or_else(|_| k.clone()))
            .collect()
    }
}

/// [`verify_chain_from`], with what it found about signatures.
pub fn verify_chain_report(anchor: &str, rows: &[AuditEvent]) -> Result<ChainReport> {
    let mut walk = Walk::default();
    let mut prev = anchor.to_string();
    for (i, row) in rows.iter().enumerate() {
        // The reader counts from one, and so does every listing this number is compared
        // against; a zero-based index here sent someone looking at the wrong row.
        prev = walk.row(&prev, row, i + 1)?.to_string();
    }
    Ok(walk.report)
}

/// The state a check carries from row to row.
#[derive(Default)]
struct Walk {
    report: ChainReport,
    /// The first signed row seen. After it, an unsigned row is a stripped signature.
    first_signed: Option<usize>,
}

impl Walk {
    /// One row against the hash it must follow. Returns the row's own hash, which the next
    /// row must name.
    fn row<'r>(&mut self, prev: &str, row: &'r AuditEvent, row_no: usize) -> Result<&'r str> {
        let broken = |why: &str| {
            Error::Index(format!(
                "audit chain broken at row {row_no} ({} {} {}): {why}",
                row.ts, row.action, row.subject
            ))
        };
        // Not named `at` in the message: that is already the chain's own timestamp.
        let (Some(p), Some(h), Some(at)) = (row.chain_prev(), row.chain_hash(), row.chain_at())
        else {
            return Err(broken("row carries no _chain"));
        };
        if p != prev {
            return Err(broken(
                "prev does not match the row before it; a row was removed, reordered or inserted",
            ));
        }
        match row.chain_version() {
            1 => {
                // Once the log is signed it stays signed. An unsigned row after a signed one
                // is either a signature somebody removed — and recomputed the old-style hash
                // for — or a writer without the key, such as an older binary still running.
                // Both are worth a person's attention; neither may pass as "pre-signing".
                if let Some(first) = self.first_signed {
                    return Err(broken(&format!(
                        "row is unsigned, but row {first} before it was signed; a signature was \
                         removed, or a process without the store's key (an older cyberbrain \
                         still running?) appended it"
                    )));
                }
                if h != row.compute_hash(prev, at) {
                    return Err(broken(
                        "content does not match its hash; the row was edited",
                    ));
                }
                self.report.unsigned += 1;
            }
            CHAIN_V2 => {
                let (Some(key), Some(sig)) = (row.chain_key(), row.chain_sig()) else {
                    return Err(broken(
                        "row is in the signed format but carries no signature",
                    ));
                };
                if h != row.compute_hash_v2(prev, at) {
                    return Err(broken(
                        "content does not match its hash; the row was edited",
                    ));
                }
                verify_signature(key, ROW_DOMAIN, h.as_bytes(), sig).map_err(|e| {
                    broken(&format!(
                        "{e}; the row was re-hashed by somebody without the key that signed it"
                    ))
                })?;
                self.report.signed += 1;
                if !self.report.keys.iter().any(|k| k == key) {
                    self.report.keys.push(key.to_string());
                }
                self.first_signed.get_or_insert(row_no);
            }
            v => {
                return Err(broken(&format!(
                    "chain format {v} is newer than this reader; use a newer cyberbrain"
                )));
            }
        }
        self.report.rows += 1;
        Ok(h)
    }
}

/// Check rows that are a selection rather than a stretch of the chain: each one against the
/// hash it names as its predecessor, without asking that predecessor to be the row before.
///
/// Proves that every row is as it was written. It does not prove that nothing between them
/// is missing, because missing rows are what a selection is.
pub fn verify_each(rows: &[AuditEvent]) -> Result<usize> {
    verify_each_report(rows).map(|r| r.rows)
}

/// [`verify_each`], with what it found about signatures. The order of the selection is the
/// order of the log, so an unsigned row after a signed one is refused here as well.
pub fn verify_each_report(rows: &[AuditEvent]) -> Result<ChainReport> {
    let mut walk = Walk::default();
    for (i, row) in rows.iter().enumerate() {
        let Some(prev) = row.chain_prev() else {
            return Err(Error::Index(format!(
                "audit row {} ({} {} {}): row carries no _chain",
                i + 1,
                row.ts,
                row.action,
                row.subject
            )));
        };
        walk.row(prev, row, i + 1)?;
    }
    Ok(walk.report)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// One JSON object per line. The form to hand to another tool.
    Jsonl,
    /// A JSON array.
    Json,
    /// Tab-separated, one row per line, for a human.
    Text,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ExportFormat::Jsonl => "jsonl",
            ExportFormat::Json => "json",
            ExportFormat::Text => "text",
        }
    }
}

pub fn render(rows: &[AuditEvent], format: ExportFormat) -> String {
    match format {
        ExportFormat::Jsonl => {
            let mut s = String::new();
            for r in rows {
                s.push_str(&serde_json::to_string(r).expect("AuditEvent serialises"));
                s.push('\n');
            }
            s
        }
        ExportFormat::Json => serde_json::to_string_pretty(rows).expect("AuditEvent serialises"),
        ExportFormat::Text => {
            let mut s = String::new();
            for r in rows {
                s.push_str(&format!(
                    "{}\t{}\t{}\t{}\t{}\n",
                    r.ts,
                    r.actor,
                    r.action,
                    r.subject,
                    r.detail_without_chain()
                ));
            }
            s
        }
    }
}

/// A sink that loses the race a fixed number of times and then lets the row through.
///
/// It is the other process, made deterministic: the same refusal the real sink raises when
/// the chain head moved between the read and the insert.
#[cfg(test)]
struct ContendingSink {
    refuse: std::sync::Mutex<u32>,
    inner: MemoryAuditSink,
    attempts: std::sync::atomic::AtomicU32,
}

#[cfg(test)]
impl ContendingSink {
    fn new(refuse: u32) -> Self {
        Self {
            refuse: std::sync::Mutex::new(refuse),
            inner: MemoryAuditSink::new(),
            attempts: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

#[cfg(test)]
impl AuditSink for ContendingSink {
    fn append(&self, event: &AuditEvent) -> Result<()> {
        self.attempts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut left = self.refuse.lock().unwrap();
        if *left > 0 {
            *left -= 1;
            return Err(Error::AuditContended { seq: 41, tries: 1 });
        }
        self.inner.append(event)
    }

    fn last(&self) -> Result<Option<AuditEvent>> {
        self.inner.last()
    }

    fn read(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        self.inner.read(filter)
    }
}

#[cfg(test)]
mod tests {

    /// A row that loses the race is rebuilt against the head that is actually there.
    ///
    /// Calibrated against the state before the retry existed: with `record_raw` returning
    /// on the first refusal, this asserts `is_ok()` on an `Err` and fails on the line below.
    #[test]
    fn a_row_that_loses_the_race_is_offered_again() {
        let sink = Arc::new(ContendingSink::new(3));
        let log = AuditLog::new(sink.clone());
        let e = log.record_raw("t", "note.write", "n", json!({"a": 1}));
        assert!(
            e.is_ok(),
            "three lost races is not a reason to give up: {e:?}"
        );
        assert_eq!(
            sink.attempts.load(std::sync::atomic::Ordering::SeqCst),
            4,
            "three refusals and the one that got in"
        );
        // And the row that got in is chained to the head, not to what the first attempt saw.
        let stored = sink.last().unwrap().expect("a row");
        assert_eq!(stored.chain_prev(), Some(GENESIS));
        assert_eq!(verify_chain(&[stored]).unwrap(), 1);
    }

    /// Bounded, and it says so rather than looping.
    #[test]
    fn a_row_that_never_gets_in_says_which_row_won() {
        let sink = Arc::new(ContendingSink::new(u32::MAX));
        let log = AuditLog::new(sink.clone());
        match log.record_raw("t", "note.write", "n", json!({})) {
            Err(Error::AuditContended { seq, tries }) => {
                assert_eq!(seq, 41);
                assert_eq!(tries, CHAIN_TRIES);
            }
            other => panic!("expected a contention error, got {other:?}"),
        }
        assert_eq!(
            sink.attempts.load(std::sync::atomic::Ordering::SeqCst),
            CHAIN_TRIES,
            "it must stop, and it must have tried"
        );
    }
    use super::*;

    // ---- the export bundle (SPEC §12.6) ----

    /// Three rows, and the hash the second one follows — the shape of every extract.
    fn three_rows() -> (Vec<AuditEvent>, String) {
        let (log, sink) = AuditLog::in_memory();
        for (i, subject) in ["a", "b", "c"].into_iter().enumerate() {
            log.record(
                &Actor::Operator,
                AuditAction::NoteWrite,
                subject,
                json!({ "n": i }),
            )
            .unwrap();
        }
        let rows = sink.rows();
        let anchor = rows[0].chain_hash().unwrap().to_string();
        (rows, anchor)
    }

    fn bundle_of(rows: &[AuditEvent]) -> String {
        bundle::render(rows, None, None, "test", "2026-09-07T00:00:00Z")
    }

    #[test]
    fn a_whole_log_exports_and_verifies() {
        let (rows, _) = three_rows();
        let text = bundle_of(&rows);
        let report = bundle::verify(&text).unwrap();
        assert_eq!(report.rows, 3);
        assert_eq!(report.anchor, GENESIS, "a full export starts at genesis");
        assert_eq!(report.last_hash.as_deref(), rows[2].chain_hash());
    }

    /// The case the anchor exists for: a period out of the middle, with no genesis in it.
    #[test]
    fn an_extract_from_the_middle_verifies_against_its_anchor() {
        let (rows, anchor) = three_rows();
        let text = bundle_of(&rows[1..]);
        let report = bundle::verify(&text).unwrap();
        assert_eq!(report.rows, 2);
        assert_eq!(report.anchor, anchor);
        // And the same rows without the header do not verify as a whole log would: the
        // anchor is the only thing standing between "an extract" and "unverifiable".
        assert!(verify_chain(&rows[1..]).is_err());
    }

    #[test]
    fn an_edited_row_breaks_the_bundle() {
        let (mut rows, _) = three_rows();
        rows[1].subject = "edited".into();
        let err = bundle::verify(&bundle_of(&rows)).unwrap_err().to_string();
        assert!(err.contains("row 2"), "{err}");
        assert!(err.contains("does not match its hash"), "{err}");
    }

    #[test]
    fn a_removed_row_breaks_the_bundle_even_with_the_counts_adjusted() {
        let (rows, _) = three_rows();
        let kept = [rows[0].clone(), rows[2].clone()];
        // Counts are consistent with the file, so only the chain can catch this.
        let err = bundle::verify(&bundle_of(&kept)).unwrap_err().to_string();
        assert!(err.contains("was removed, reordered or inserted"), "{err}");
    }

    #[test]
    fn a_truncated_file_is_not_a_shorter_answer() {
        let (rows, _) = three_rows();
        let text = bundle_of(&rows);
        let without_footer: String = text
            .lines()
            .take(text.lines().count() - 1)
            .map(|l| format!("{l}\n"))
            .collect();
        let err = bundle::verify(&without_footer).unwrap_err().to_string();
        assert!(err.contains("footer"), "{err}");
    }

    #[test]
    fn a_forged_anchor_does_not_help() {
        let (rows, _) = three_rows();
        let text = bundle_of(&rows[1..]).replace(
            &format!("\"anchor\":\"{}\"", rows[0].chain_hash().unwrap()),
            "\"anchor\":\"0000000000000000000000000000000000000000000000000000000000000000\"",
        );
        let err = bundle::verify(&text).unwrap_err().to_string();
        assert!(err.contains("row 1"), "{err}");
    }

    #[test]
    fn a_header_that_undercounts_is_refused() {
        let (rows, _) = three_rows();
        let text = bundle_of(&rows).replace("\"rows\":3", "\"rows\":2");
        let err = bundle::verify(&text).unwrap_err().to_string();
        assert!(err.contains("rows"), "{err}");
    }

    #[test]
    fn a_future_format_version_is_refused_rather_than_guessed() {
        let (rows, _) = three_rows();
        let text = bundle_of(&rows).replace("\"version\":1", "\"version\":99");
        let err = bundle::verify(&text).unwrap_err().to_string();
        assert!(err.contains("version 99"), "{err}");
    }

    #[test]
    fn an_empty_period_is_a_valid_bundle() {
        // "Nothing happened in that week" is an answer an auditor may need, and it has to
        // be checkable like any other.
        let text = bundle_of(&[]);
        let report = bundle::verify(&text).unwrap();
        assert_eq!(report.rows, 0);
        assert_eq!(report.last_hash, None);
    }

    #[test]
    fn a_period_selects_by_both_ends() {
        // Stamped by hand rather than by the clock: three rows written in a loop can share
        // a timestamp — the column is milliseconds — and a period test whose rows are not
        // distinguishable measures nothing. That collision is real, and it is why both
        // bounds are inclusive: an export takes one row too many rather than one too few.
        let (mut rows, _) = three_rows();
        for (i, r) in rows.iter_mut().enumerate() {
            r.ts = format!("2026-09-0{}T00:00:00Z", i + 1).parse().unwrap();
        }
        let f = AuditFilter {
            since: Some(rows[1].ts),
            until: Some(rows[1].ts),
            ..Default::default()
        };
        let selected: Vec<_> = rows.iter().filter(|r| f.matches(r)).collect();
        assert_eq!(
            selected.len(),
            1,
            "both bounds are inclusive and both apply"
        );
        assert_eq!(selected[0].subject, "b");

        let open_ended = AuditFilter {
            since: Some(rows[1].ts),
            ..Default::default()
        };
        assert_eq!(rows.iter().filter(|r| open_ended.matches(r)).count(), 2);
    }

    #[test]
    fn exporting_a_bundle_is_itself_recorded() {
        let (log, sink) = AuditLog::in_memory();
        log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        let text = log
            .export_bundle(&Actor::Operator, &AuditFilter::default(), "test")
            .unwrap();
        // The row for the export is written after the rows are rendered, so the file does
        // not contain its own export row — but the log does.
        assert_eq!(sink.actions(), ["note.write", "audit.export"]);
        assert_eq!(bundle::verify(&text).unwrap().rows, 1);
    }

    #[test]
    fn rows_chain_and_verify() {
        let (log, sink) = AuditLog::in_memory();
        log.record(
            &Actor::Operator,
            AuditAction::NoteWrite,
            "a",
            json!({"x": 1}),
        )
        .unwrap();
        log.record(&Actor::Cli, AuditAction::NoteWrite, "b", json!({"x": 2}))
            .unwrap();
        log.record(&Actor::Mcp, AuditAction::NoteWrite, "c", Value::Null)
            .unwrap();
        assert_eq!(log.verify().unwrap(), 3);
        let rows = sink.rows();
        assert_eq!(rows[0].chain_prev(), Some(GENESIS));
        assert_eq!(rows[1].chain_prev(), rows[0].chain_hash());
        assert_eq!(rows[2].chain_prev(), rows[1].chain_hash());
    }

    #[test]
    fn the_chain_survives_the_index_restamping_ts() {
        // The index writes its own `ts`. Verification must not depend on ours surviving.
        let (log, sink) = AuditLog::in_memory();
        log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        let mut rows = sink.rows();
        rows[0].ts = "2030-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(verify_chain(&rows).unwrap(), 1);
    }

    #[test]
    fn an_edited_row_is_detected() {
        let (log, sink) = AuditLog::in_memory();
        log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        log.record(
            &Actor::Operator,
            AuditAction::NoteEraseCompleted,
            "b",
            json!({}),
        )
        .unwrap();
        let mut rows = sink.rows();
        rows[0].subject = "tampered".into();
        let err = verify_chain(&rows).unwrap_err().to_string();
        // The first row of the log, counted the way the listing prints it.
        assert!(err.contains("row 1"), "{err}");
        assert!(err.contains("edited"), "{err}");
    }

    #[test]
    fn a_removed_row_is_detected() {
        let (log, sink) = AuditLog::in_memory();
        for s in ["a", "b", "c"] {
            log.record(&Actor::Operator, AuditAction::NoteWrite, s, json!({}))
                .unwrap();
        }
        let mut rows = sink.rows();
        rows.remove(1);
        let err = verify_chain(&rows).unwrap_err().to_string();
        assert!(err.contains("row 2"), "{err}");
        assert!(err.contains("removed"), "{err}");
    }

    #[test]
    fn a_row_with_its_chain_stripped_is_detected() {
        let (log, sink) = AuditLog::in_memory();
        log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        let mut rows = sink.rows();
        rows[0].detail = json!({});
        assert!(verify_chain(&rows).is_err());
    }

    #[test]
    fn a_failing_sink_propagates() {
        let (log, sink) = AuditLog::in_memory();
        sink.fail_next_append();
        assert!(
            log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
                .is_err()
        );
        assert!(sink.is_empty());
    }

    #[test]
    fn export_records_itself_after_rendering() {
        let (log, sink) = AuditLog::in_memory();
        log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        let out = log
            .export(
                &Actor::Operator,
                &AuditFilter::default(),
                ExportFormat::Jsonl,
            )
            .unwrap();
        assert_eq!(
            out.lines().count(),
            1,
            "the export row is not in its own export"
        );
        assert_eq!(sink.actions(), ["note.write", "audit.export"]);
        assert_eq!(log.verify().unwrap(), 2);
    }

    #[test]
    fn filter_selects() {
        let (log, _) = AuditLog::in_memory();
        log.record(
            &Actor::Operator,
            AuditAction::NoteWrite,
            "alpha",
            json!({"who": "Bob"}),
        )
        .unwrap();
        log.record(&Actor::Cli, AuditAction::EgressPermitted, "beta", json!({}))
            .unwrap();
        let f = AuditFilter {
            action: Some("egress.permitted".into()),
            ..Default::default()
        };
        assert_eq!(log.read(&f).unwrap().len(), 1);
        let f = AuditFilter {
            subject: Some("alpha".into()),
            ..Default::default()
        };
        assert_eq!(log.read(&f).unwrap()[0].subject, "alpha");
        let f = AuditFilter {
            contains: Some("bob".into()),
            ..Default::default()
        };
        assert_eq!(
            log.read(&f).unwrap().len(),
            1,
            "contains is case-insensitive and covers detail"
        );
        let f = AuditFilter {
            limit: Some(1),
            ..Default::default()
        };
        assert_eq!(log.read(&f).unwrap().len(), 1);
    }

    #[test]
    fn inference_rows_keep_absent_counts_absent() {
        let (log, sink) = AuditLog::in_memory();
        log.record_inference(
            &Actor::Cli,
            "http://127.0.0.1:11434/v1/chat/completions",
            "qwen3",
            "ok",
            Some(12),
            None,
            None,
            json!({}),
        )
        .unwrap();
        let d = &sink.rows()[0].detail;
        assert_eq!(d["prompt_tokens"], 12);
        assert!(d["completion_tokens"].is_null(), "never estimated: {d}");
        assert!(d["total_tokens"].is_null());
        assert_eq!(sink.actions(), ["inference.call"]);
    }

    #[test]
    fn text_export_hides_the_chain_but_jsonl_keeps_it() {
        let (log, _) = AuditLog::in_memory();
        log.record(
            &Actor::Operator,
            AuditAction::NoteWrite,
            "a",
            json!({"k": "v"}),
        )
        .unwrap();
        let rows = log.read(&AuditFilter::default()).unwrap();
        assert!(!render(&rows, ExportFormat::Text).contains("_chain"));
        assert!(render(&rows, ExportFormat::Jsonl).contains("_chain"));
    }

    /// K10: `policy audit --action X --export f` wrote a file that its own check then
    /// refused, because the rows of one action are not consecutive in the chain and the
    /// check walked them as if they were. Calibrated against the state before the fix:
    /// `verify` failed with "a row was removed, reordered or inserted" on row 2.
    #[test]
    fn a_bundle_filtered_by_action_verifies() {
        let (log, _) = AuditLog::in_memory();
        for (action, subject) in [
            (AuditAction::NoteWrite, "a"),
            (AuditAction::PolicyRefusal, "x"),
            (AuditAction::NoteWrite, "b"),
            (AuditAction::PolicyRefusal, "y"),
            (AuditAction::NoteWrite, "c"),
        ] {
            log.record(&Actor::Operator, action, subject, json!({}))
                .unwrap();
        }
        let filter = AuditFilter {
            action: Some("note.write".into()),
            ..Default::default()
        };
        let text = log
            .export_bundle(&Actor::Operator, &filter, "test")
            .unwrap();
        let report = bundle::verify(&text).expect("a filtered export verifies");
        assert_eq!(report.rows, 3);
    }

    fn selection_bundle() -> (String, Vec<AuditEvent>) {
        let (log, sink) = AuditLog::in_memory();
        for (action, subject) in [
            (AuditAction::NoteWrite, "a"),
            (AuditAction::PolicyRefusal, "x"),
            (AuditAction::NoteWrite, "b"),
        ] {
            log.record(&Actor::Operator, action, subject, json!({}))
                .unwrap();
        }
        let filter = AuditFilter {
            action: Some("note.write".into()),
            ..Default::default()
        };
        let text = log
            .export_bundle(&Actor::Operator, &filter, "test")
            .unwrap();
        (text, sink.rows())
    }

    /// A selection is checked row by row, which still catches an edit.
    #[test]
    fn an_edited_row_in_a_selection_is_refused() {
        let (text, _) = selection_bundle();
        assert!(
            text.contains("\"version\":2"),
            "a selection needs version 2"
        );
        let bad = text.replace("\"subject\":\"b\"", "\"subject\":\"c\"");
        let err = bundle::verify(&bad).unwrap_err().to_string();
        assert!(err.contains("row 2") && err.contains("edited"), "{err}");
    }

    /// And a row that does not belong to the selection cannot ride along in it.
    #[test]
    fn a_row_outside_the_selection_is_refused() {
        let (text, rows) = selection_bundle();
        let foreign = serde_json::to_string(&rows[1]).unwrap();
        let mut lines: Vec<&str> = text.lines().collect();
        lines.insert(2, &foreign);
        let patched = lines.join("\n").replace("\"rows\":2", "\"rows\":3");
        let err = bundle::verify(&patched).unwrap_err().to_string();
        assert!(err.contains("selection"), "{err}");
    }

    /// Calling a selection version 1 does not make a version-1 reader walk it as a chain.
    #[test]
    fn a_selection_cannot_pass_as_version_one() {
        let (text, _) = selection_bundle();
        let err = bundle::verify(&text.replace("\"version\":2", "\"version\":1"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("version 1"), "{err}");
    }

    // ---- signed rows (F2, K1) ----

    fn signed_log() -> (AuditLog, Arc<MemoryAuditSink>, AuditKey) {
        let key = AuditKey::generate().unwrap();
        let sink = Arc::new(MemoryAuditSink::new());
        (
            AuditLog::new(sink.clone()).with_key(Some(key.clone())),
            sink,
            key,
        )
    }

    /// K1: in format 1 a newline inside one field and a moved boundary give the same hash,
    /// so a signed row could be made to say something else and keep its signature.
    /// Calibrated against format 2 still hashing with the newline rule: the shifted row
    /// verified, and this test failed on the `is_err`.
    #[test]
    fn moving_a_field_boundary_breaks_a_signed_row() {
        let (log, sink, _) = signed_log();
        log.record_raw("agent:a\nnote.write", "x", "s", json!({}))
            .unwrap();
        let mut rows = sink.rows();
        rows[0].actor = "agent:a".into();
        rows[0].action = "note.write\nx".into();
        let err = verify_chain(&rows);
        assert!(err.is_err(), "a moved boundary kept its hash and signature");
    }

    #[test]
    fn signed_rows_verify_and_name_their_key() {
        let (log, sink, key) = signed_log();
        for s in ["a", "b"] {
            log.record(&Actor::Operator, AuditAction::NoteWrite, s, json!({}))
                .unwrap();
        }
        let rows = sink.rows();
        assert_eq!(rows[0].chain_version(), CHAIN_V2);
        let r = verify_chain_report(GENESIS, &rows).unwrap();
        assert_eq!((r.rows, r.signed, r.unsigned), (2, 2, 0));
        assert_eq!(r.keys, vec![key.public_hex()]);
        assert_eq!(r.fingerprints(), vec![key.fingerprint()]);
    }

    /// The rows a store wrote before it had a key keep verifying under their own rule, and
    /// are counted as such rather than reported as a fault.
    #[test]
    fn rows_from_before_the_key_verify_as_pre_signing() {
        let sink = Arc::new(MemoryAuditSink::new());
        let old = AuditLog::new(sink.clone());
        old.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        old.record(&Actor::Operator, AuditAction::NoteWrite, "b", json!({}))
            .unwrap();
        let new = AuditLog::new(sink.clone()).with_key(Some(AuditKey::generate().unwrap()));
        new.record(&Actor::Operator, AuditAction::NoteWrite, "c", json!({}))
            .unwrap();
        let r = new.verify_report().unwrap();
        assert_eq!((r.rows, r.unsigned, r.signed), (3, 2, 1));
    }

    /// Somebody without the key edits a signed row and recomputes its hash, and the chain
    /// after it: the hashes hold, the signature does not.
    #[test]
    fn a_rehashed_signed_row_fails_on_its_signature() {
        let (log, sink, _) = signed_log();
        log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        let mut rows = sink.rows();
        rows[0].subject = "b".into();
        let at = rows[0].chain_at().unwrap().to_string();
        let hash = rows[0].compute_hash_v2(GENESIS, &at);
        rows[0].detail["_chain"]["hash"] = json!(hash);
        let err = verify_chain(&rows).unwrap_err().to_string();
        assert!(err.contains("signature does not match"), "{err}");
    }

    /// Stripping the signature and recomputing the old-style hash makes the row look like
    /// one from before the key. After a signed row, that is refused.
    #[test]
    fn an_unsigned_row_after_a_signed_one_is_refused() {
        let (log, sink, _) = signed_log();
        for s in ["a", "b"] {
            log.record(&Actor::Operator, AuditAction::NoteWrite, s, json!({}))
                .unwrap();
        }
        let mut rows = sink.rows();
        let prev = rows[1].chain_prev().unwrap().to_string();
        let at = rows[1].chain_at().unwrap().to_string();
        let hash = rows[1].compute_hash(&prev, &at);
        rows[1].detail["_chain"] = json!({ "prev": prev, "hash": hash, "at": at });
        let err = verify_chain(&rows).unwrap_err().to_string();
        assert!(err.contains("row 2") && err.contains("unsigned"), "{err}");
    }

    #[test]
    fn a_chain_format_from_the_future_is_refused_rather_than_guessed() {
        let (log, sink, _) = signed_log();
        log.record(&Actor::Operator, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        let mut rows = sink.rows();
        rows[0].detail["_chain"]["v"] = json!(9);
        let err = verify_chain(&rows).unwrap_err().to_string();
        assert!(err.contains("format 9"), "{err}");
    }

    // ---- signed bundles ----

    fn signed_bundle() -> (String, AuditKey) {
        let (log, _, key) = signed_log();
        for s in ["a", "b", "c"] {
            log.record(&Actor::Operator, AuditAction::NoteWrite, s, json!({}))
                .unwrap();
        }
        let text = log
            .export_bundle_with(&Actor::Operator, &AuditFilter::default(), "test", true)
            .unwrap();
        (text, key)
    }

    #[test]
    fn a_signed_bundle_verifies_against_the_expected_key() {
        let (text, key) = signed_bundle();
        let r = bundle::verify_expecting(&text, &key.fingerprint()).unwrap();
        assert_eq!(r.version, bundle::VERSION_2);
        assert_eq!(r.fingerprint.as_deref(), Some(key.fingerprint().as_str()));
        assert_eq!((r.chain.signed, r.chain.unsigned), (3, 0));
    }

    /// A forger signs too. Only the expected key tells the two apart.
    #[test]
    fn a_bundle_resigned_by_another_key_fails_the_expectation() {
        let (text, key) = signed_bundle();
        let forger = AuditKey::generate().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut header: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        header["key"] = json!(forger.public_hex());
        let mut body = format!("{header}\n");
        for l in &lines[1..lines.len() - 1] {
            body.push_str(l);
            body.push('\n');
        }
        let mut footer: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
        footer["sig"] = json!(forger.sign(crate::audit_key::BUNDLE_DOMAIN, body.as_bytes()));
        let forged = format!("{body}{footer}\n");
        assert!(
            bundle::verify(&forged).is_ok(),
            "self-consistent, as forgeries are"
        );
        let err = bundle::verify_expecting(&forged, &key.fingerprint())
            .unwrap_err()
            .to_string();
        assert!(err.contains("not with the expected"), "{err}");
    }

    #[test]
    fn a_signed_bundle_with_a_row_removed_from_the_end_fails_its_signature() {
        let (text, _) = signed_bundle();
        let lines: Vec<&str> = text.lines().collect();
        let footer = lines[lines.len() - 1].replace("\"rows\":3", "\"rows\":2");
        let mut cut: Vec<String> = lines[..lines.len() - 2]
            .iter()
            .map(|l| l.to_string())
            .collect();
        cut[0] = cut[0].replace("\"rows\":3", "\"rows\":2");
        cut.push(footer);
        let err = bundle::verify(&(cut.join("\n") + "\n"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("signature"), "{err}");
    }

    #[test]
    fn a_signature_stripped_from_a_bundle_is_noticed() {
        let (text, _) = signed_bundle();
        let lines: Vec<&str> = text.lines().collect();
        let mut footer: serde_json::Value = serde_json::from_str(lines[lines.len() - 1]).unwrap();
        footer.as_object_mut().unwrap().remove("sig");
        let stripped = format!("{}\n{footer}\n", lines[..lines.len() - 1].join("\n"));
        let err = bundle::verify(&stripped).unwrap_err().to_string();
        assert!(err.contains("no signature"), "{err}");
    }

    #[test]
    fn asking_for_a_signature_without_a_key_is_an_error() {
        let (log, _) = AuditLog::in_memory();
        let err = log
            .export_bundle_with(&Actor::Operator, &AuditFilter::default(), "t", true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("audit-key --init"), "{err}");
    }

    /// An unsigned period of an unsigned log is still version 1, so a reader that only knows
    /// version 1 keeps reading what it always read.
    #[test]
    fn a_plain_unsigned_bundle_stays_version_one() {
        let (rows, _) = three_rows();
        assert!(
            bundle_of(&rows).starts_with("{\"kind\":\"cyberbrain.audit.export\",\"version\":1,")
        );
    }
}
