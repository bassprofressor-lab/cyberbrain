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

use cyberbrain_core::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use std::sync::{Arc, Mutex};

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

/// The writer. Cheap to clone (it is an `Arc` around the sink); hand clones to the egress
/// gate, the write gate and the retention path.
#[derive(Clone)]
pub struct AuditLog {
    sink: Arc<dyn AuditSink>,
    /// Serialises `last` + `append` within this process so the chain cannot fork here.
    write_lock: Arc<Mutex<()>>,
}

impl AuditLog {
    pub fn new(sink: Arc<dyn AuditSink>) -> Self {
        Self {
            sink,
            write_lock: Arc::new(Mutex::new(())),
        }
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
        let mut detail = match detail {
            Value::Object(m) => Value::Object(m),
            Value::Null => json!({}),
            other => json!({ "value": other }),
        };
        let _guard = self.write_lock.lock().map_err(|_| {
            Error::Index("audit write lock poisoned; refusing to append (fail closed)".into())
        })?;
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
            subject: subject.into(),
            detail: detail.clone(),
        };
        let hash = event.compute_hash(&prev, &at);
        if let Value::Object(m) = &mut detail {
            m.insert(
                "_chain".into(),
                json!({ "prev": prev, "hash": hash, "at": at }),
            );
        }
        event.detail = detail;
        self.sink.append(&event)?;
        Ok(event)
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
        let rows = self.sink.read(&AuditFilter::default())?;
        verify_chain(&rows)
    }

    /// A period of the log as a self-checking bundle: header with the anchor, the rows, a
    /// footer. See [`bundle`] for the format and why it exists.
    ///
    /// Recorded like the plain export, and for the same reason — an extract is an access,
    /// and an access that leaves no trace is the one nobody can ask about later.
    pub fn export_bundle(&self, actor: &Actor, filter: &AuditFilter, tool: &str) -> Result<String> {
        let rows = self.sink.read(filter)?;
        let out = bundle::render(
            &rows,
            filter.since.map(|t| t.to_string()),
            filter.until.map(|t| t.to_string()),
            tool,
            &jiff::Timestamp::now().to_string(),
        );
        self.record(
            actor,
            AuditAction::AuditExport,
            "audit",
            json!({ "rows": rows.len(), "format": "bundle" }),
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
    use super::{AuditEvent, GENESIS, verify_chain_from};
    use cyberbrain_core::{Error, Result};
    use serde::{Deserialize, Serialize};

    pub const KIND: &str = "cyberbrain.audit.export";
    pub const KIND_END: &str = "cyberbrain.audit.export.end";
    pub const VERSION: u32 = 1;

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
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct Footer {
        pub kind: String,
        pub rows: usize,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub first_hash: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub last_hash: Option<String>,
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
    }

    /// Render rows as a bundle. `tool` names the writer, `exported_at` is its own timestamp.
    pub fn render(
        rows: &[AuditEvent],
        from: Option<String>,
        to: Option<String>,
        tool: &str,
        exported_at: &str,
    ) -> String {
        // The anchor comes out of the first row rather than a second read of the log: a row
        // already carries the hash it follows. One source, no chance of the two disagreeing.
        let anchor = rows
            .first()
            .and_then(|r| r.chain_prev())
            .unwrap_or(GENESIS)
            .to_string();
        let header = Header {
            kind: KIND.to_string(),
            version: VERSION,
            tool: tool.to_string(),
            exported_at: exported_at.to_string(),
            from,
            to,
            anchor,
            rows: rows.len(),
        };
        let footer = Footer {
            kind: KIND_END.to_string(),
            rows: rows.len(),
            first_hash: rows.first().and_then(|r| r.chain_hash()).map(str::to_owned),
            last_hash: rows.last().and_then(|r| r.chain_hash()).map(str::to_owned),
        };

        let mut s = serde_json::to_string(&header).expect("header serialises");
        s.push('\n');
        for r in rows {
            s.push_str(&serde_json::to_string(r).expect("AuditEvent serialises"));
            s.push('\n');
        }
        s.push_str(&serde_json::to_string(&footer).expect("footer serialises"));
        s.push('\n');
        s
    }

    /// Check a bundle on its own: no store, no configuration, no network.
    ///
    /// Everything it needs is in the file, which is the property that has to survive the
    /// next ten years.
    pub fn verify(text: &str) -> Result<Report> {
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
        if header.version != VERSION {
            return Err(bad(format!(
                "format version {} is not {VERSION}; use a reader that knows it",
                header.version
            )));
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
        let verified = verify_chain_from(&header.anchor, &rows)?;

        let last_hash = rows.last().and_then(|r| r.chain_hash()).map(str::to_owned);
        if footer.last_hash != last_hash {
            return Err(bad(
                "footer's last hash is not the last row's hash; rows were removed from the end"
                    .into(),
            ));
        }

        Ok(Report {
            rows: verified,
            anchor: header.anchor,
            from: header.from,
            to: header.to,
            last_hash,
            tool: header.tool,
            exported_at: header.exported_at,
        })
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
    let mut prev = anchor.to_string();
    for (i, row) in rows.iter().enumerate() {
        // The reader counts from one, and so does every listing this number is compared
        // against; a zero-based index here sent someone looking at the wrong row. Not
        // named `at`: that is already the chain's own timestamp, three lines down.
        let row_no = i + 1;
        let (Some(p), Some(h), Some(at)) = (row.chain_prev(), row.chain_hash(), row.chain_at())
        else {
            return Err(Error::Index(format!(
                "audit chain broken at row {row_no} ({} {} {}): row carries no _chain",
                row.ts, row.action, row.subject
            )));
        };
        if p != prev {
            return Err(Error::Index(format!(
                "audit chain broken at row {row_no} ({} {} {}): prev does not match the row \
                 before it; a row was removed, reordered or inserted",
                row.ts, row.action, row.subject
            )));
        }
        let expect = row.compute_hash(&prev, at);
        if h != expect {
            return Err(Error::Index(format!(
                "audit chain broken at row {row_no} ({} {} {}): content does not match its hash; \
                 the row was edited",
                row.ts, row.action, row.subject
            )));
        }
        prev = h.to_string();
    }
    Ok(rows.len())
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

#[cfg(test)]
mod tests {
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
}
