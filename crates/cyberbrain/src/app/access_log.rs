//! Who searched which bereich, and who changed who may see what (SPEC §12.6.3, 2026-11-08).
//!
//! Two rows, chained and signed like the rest of the record:
//!
//! - `search.access` for a search or an expanded citation, by the clients `[audit] searches`
//!   names. It carries the bereiche of what was returned and never the query: a firm's search
//!   for a client's name is that name, and a log meant to show *that* somebody looked must not
//!   become a second copy of *what* they looked for. A keyed hash still says which searches
//!   were the same. They go to `search-audit.db`, which unlike `audit.db` gives up rows after
//!   `[audit] search_retention_days`, each time with a report in the main log.
//! - `clients.changed` when `[clients]` differs from the last state the log recorded. The file
//!   is edited by `cb-rechte` or by hand, and neither goes through a command here, so the
//!   change is found where it takes effect: when a process loads the configuration.

use super::{App, AuditView, lock_index};
use crate::audit_bridge::{SEARCH_LOG_FILE, StoreAuditSink, load_audit_key};
use cyberbrain_core::config::SearchAudit;
use cyberbrain_core::{Error, Result};
use cyberbrain_index::AuditStore;
use cyberbrain_policy::audit::GENESIS;
use cyberbrain_policy::{
    Actor, AuditAction, AuditEvent, AuditFilter, AuditLog, ChainReport, ExportFormat,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The search log as the app holds it: the writer, and the sink for reading and pruning.
pub(super) type SearchLog = (AuditLog, Arc<StoreAuditSink>);

/// Read in debug builds only: the moment the retention cutoff is measured from, so a test can
/// move a day ahead without waiting one. A release binary always uses the clock.
const TEST_NOW_ENV: &str = "CYBERBRAIN_TEST_NOW";

fn retention_now() -> jiff::Timestamp {
    #[cfg(debug_assertions)]
    if let Some(t) = std::env::var(TEST_NOW_ENV)
        .ok()
        .and_then(|v| v.parse::<jiff::Timestamp>().ok())
    {
        return t;
    }
    let _ = TEST_NOW_ENV;
    jiff::Timestamp::now()
}

/// The key the query hash is made with, relative to the store root. Beside the audit key,
/// under the same `.gitignore`, readable by the owner only.
pub const SEARCH_KEY_FILE: &str = "keys/search.key";

/// Who a `clients.changed` row is written as. The process that notices the change did not
/// make it: an MCP server started for anna must not be booked as the one who gave anna a file.
pub const CONFIG_FILE_ACTOR: &str = "config-file";

/// A hint from whoever changed the file, such as `cb-rechte`, recorded as `claimed_by`. It is
/// a claim and named as one: anybody who can set an environment variable can set this one.
pub const CHANGED_BY_ENV: &str = "CYBERBRAIN_CONFIG_CHANGED_BY";

/// Hex digits of the query hash kept in the row. 64 bits: two different searches of one store
/// colliding is not a concern at that length, and a shorter value is less of a fingerprint.
const QUERY_HASH_HEX: usize = 16;

impl App {
    /// Whether `[audit] searches` asks for this actor's searches: everybody's, nobody's, or
    /// those of a client limited to bereiche.
    pub(super) fn searches_recorded(&self) -> bool {
        match self.config.audit.searches {
            SearchAudit::Off => false,
            SearchAudit::All => true,
            SearchAudit::Limited => self.client_scope().is_some(),
        }
    }

    /// One `search.access` row, when this actor's searches are recorded.
    ///
    /// Failing to write it fails the search: a limited client whose access leaves no trace is
    /// the case the row exists for, and a caller that cannot record what it is about to hand
    /// over must not hand it over (§12.6).
    pub(super) fn record_search(
        &self,
        tool: &str,
        asked: &str,
        requested_bereich: Option<&str>,
        bereiche: &BTreeMap<String, usize>,
        hits: usize,
    ) -> Result<()> {
        if !self.searches_recorded() {
            return Ok(());
        }
        let mut detail = json!({
            "tool": tool,
            "limited": self.client_scope().is_some(),
            "bereiche": bereiche.keys().collect::<Vec<_>>(),
            "per_bereich": bereiche,
            "hits": hits,
            "query_hash": self.query_hash(asked),
        });
        if let Some(b) = requested_bereich {
            detail["requested_bereich"] = json!(b);
        }
        self.search_log()?.0.record(
            &self.actor,
            AuditAction::SearchAccess,
            format!("search:{tool}"),
            detail,
        )?;
        Ok(())
    }

    /// The bereich of every hit's note, counted. Hits without a bereich count under no name:
    /// a limited client never gets one, and for anybody else "none" is not a bereich.
    pub(super) fn hit_bereiche(
        &self,
        hits: &[cyberbrain_core::Hit],
    ) -> Result<BTreeMap<String, usize>> {
        let ix = lock_index(&self.index)?;
        let mut out = BTreeMap::new();
        for h in hits {
            if let Some(b) = ix.note(&h.note_id)?.and_then(|r| r.front.bereich) {
                *out.entry(b).or_insert(0) += 1;
            }
        }
        Ok(out)
    }

    /// The query as a keyed hash: equal for the same search (case and spacing aside), useless
    /// to anybody without the store's key. The key never leaves this machine, so an exported
    /// log, a hub or AgentGuard cannot test guesses against it. The operator, who holds the
    /// key, can — and can also read every note, which is why that is no loss.
    ///
    /// `None` when there is no key and none can be made (a read-only store): the row is then
    /// written without the hash rather than not at all.
    fn query_hash(&self, asked: &str) -> Option<String> {
        let key = self
            .search_key
            .get_or_init(|| load_or_create_key(&self.root))
            .as_ref()?;
        let normal = asked
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let h = blake3::keyed_hash(key, normal.as_bytes()).to_hex();
        Some(h[..QUERY_HASH_HEX].to_string())
    }

    /// Compare `[clients]` with the last state the log recorded and write the difference.
    ///
    /// Run at every open: cheap (one indexed read of a handful of rows), and the only place a
    /// hand edit can be noticed, because nothing else here sees the file change. A store that
    /// never limited anybody and has no such row writes nothing, so a single user's store
    /// stays as it was. Two processes opening at the same instant after an edit can both write
    /// the row; the second says the same as the first and does no harm.
    pub(super) fn record_clients_change(&self) -> Result<()> {
        let now = clients_state(&self.config.clients);
        let last = self
            .audit_sink
            .read_exact_action(AuditAction::ClientsChanged.as_str())?
            .pop()
            .and_then(|e| e.detail.get("after").cloned());
        let before = match last {
            Some(b) if b == now => return Ok(()),
            Some(b) => Some(b),
            None if now == clients_state(&Default::default()) => return Ok(()),
            None => None,
        };
        let empty = clients_state(&Default::default());
        let old = before.as_ref().unwrap_or(&empty);
        let lists = |v: &Value| -> BTreeMap<String, BTreeSet<String>> {
            v.get("bereiche")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(c, b)| {
                            let set = b
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_owned)
                                        .collect()
                                })
                                .unwrap_or_default();
                            (c.clone(), set)
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (was, is) = (lists(old), lists(&now));
        let diff = |a: &BTreeMap<String, BTreeSet<String>>,
                    b: &BTreeMap<String, BTreeSet<String>>| {
            let mut out = BTreeMap::new();
            for (c, set) in a {
                let gone: Vec<&String> = match b.get(c) {
                    Some(other) => set.difference(other).collect(),
                    None => set.iter().collect(),
                };
                if !gone.is_empty() {
                    out.insert(c.clone(), json!(gone));
                }
            }
            out
        };
        let mut detail = json!({
            "baseline": before.is_none(),
            "before": before,
            "after": now,
            "added": diff(&is, &was),
            "removed": diff(&was, &is),
            "clients_added": is.keys().filter(|c| !was.contains_key(*c)).collect::<Vec<_>>(),
            "clients_removed": was.keys().filter(|c| !is.contains_key(*c)).collect::<Vec<_>>(),
            "detected_by": self.actor.to_string(),
        });
        if old.get("unlisted") != now.get("unlisted") {
            detail["unlisted"] = json!({ "before": old["unlisted"], "after": now["unlisted"] });
        }
        if let Some(who) = std::env::var(CHANGED_BY_ENV)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            detail["claimed_by"] = json!(who);
        }
        self.policy.audit().record(
            &Actor::System(CONFIG_FILE_ACTOR.into()),
            AuditAction::ClientsChanged,
            "config:clients",
            detail,
        )?;
        Ok(())
    }

    /// The search log, opened (and made, on the first recorded search) beside `audit.db`.
    /// Signed with the store's audit key like the main log.
    pub(super) fn search_log(&self) -> Result<&SearchLog> {
        if let Some(l) = self.search_log.get() {
            return Ok(l);
        }
        let sink = Arc::new(StoreAuditSink::search_log(AuditStore::open_prunable(
            &self.root.join(SEARCH_LOG_FILE),
        )?));
        let log = AuditLog::new(sink.clone()).with_key(load_audit_key(&self.root)?);
        let _ = self.search_log.set((log, sink));
        Ok(self.search_log.get().expect("set just now"))
    }

    /// The search log if it exists; `None` in a store that never recorded a search, so that
    /// reading or pruning does not create the file.
    fn existing_search_log(&self) -> Result<Option<&SearchLog>> {
        if self.search_log.get().is_none() && !self.root.join(SEARCH_LOG_FILE).exists() {
            return Ok(None);
        }
        self.search_log().map(Some)
    }

    /// Delete search rows older than `[audit] search_retention_days` (SPEC §12.6.3). Runs at
    /// every open of a store that has a search log: one indexed read when nothing is due.
    ///
    /// Before anything is deleted, a `search.log.pruned` row goes into the main log, under the
    /// search log's write lock: the period, the count, the first and last chain hash and a
    /// digest of every hash that went. The last hash becomes the anchor the remaining rows
    /// chain to, so the search log still verifies, and a deletion without that row does not.
    pub(super) fn prune_search_log(&self) -> Result<usize> {
        let Some((_, sink)) = self.existing_search_log()? else {
            return Ok(0);
        };
        let days = self.config.audit.search_retention_days;
        let now = retention_now();
        let cutoff = now
            .checked_sub(jiff::SignedDuration::from_hours(24 * i64::from(days)))
            .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
        sink.prune_through(cutoff, |rows| {
            let (Some(first), Some(last)) = (rows.first(), rows.last()) else {
                return Err(Error::Index("nothing to prune".into()));
            };
            let unchained = || Error::Index("search log row without a chain hash".into());
            let last_hash = last.chain_hash().ok_or_else(unchained)?.to_string();
            let mut digest = blake3::Hasher::new();
            for r in rows {
                digest.update(r.chain_hash().ok_or_else(unchained)?.as_bytes());
                digest.update(b"\n");
            }
            self.policy.audit().record(
                &Actor::Retention,
                AuditAction::SearchLogPruned,
                "search-log",
                json!({
                    "log": SEARCH_LOG_FILE,
                    "rows": rows.len(),
                    "from": first.ts.to_string(),
                    "until": last.ts.to_string(),
                    "previous_anchor": first.chain_prev(),
                    "first_hash": first.chain_hash(),
                    "last_hash": last_hash,
                    "digest": digest.finalize().to_hex().to_string(),
                    "retention_days": days,
                    "cutoff": cutoff.to_string(),
                }),
            )?;
            Ok(last_hash)
        })
    }

    /// Check the search log: its chain from where it starts, and that it starts where it may.
    ///
    /// It may start at the genesis marker, or at the `last_hash` of a `search.log.pruned`
    /// row in the main log. Anything else is rows deleted without a report. The newest report
    /// must also be accounted for: either the log starts at it, or the rows it names are still
    /// there (the deletion after the report did not happen). `None` without a search log.
    pub fn verify_search_log(&self) -> Option<std::result::Result<ChainReport, String>> {
        let check = || -> Result<Option<ChainReport>> {
            let Some((_, sink)) = self.existing_search_log()? else {
                return Ok(None);
            };
            let rows = cyberbrain_policy::AuditSink::read(sink.as_ref(), &AuditFilter::default())?;
            let reports = self
                .audit_sink
                .read_exact_action(AuditAction::SearchLogPruned.as_str())?;
            let reported: Vec<String> = reports
                .iter()
                .filter_map(|r| r.detail.get("last_hash")?.as_str().map(str::to_owned))
                .collect();
            let start = match rows.first() {
                Some(r) => r.chain_prev().unwrap_or(GENESIS).to_string(),
                None => sink.anchor()?.unwrap_or_else(|| GENESIS.to_string()),
            };
            if start != GENESIS && !reported.contains(&start) {
                return Err(Error::Index(format!(
                    concat!(
                        "the search log starts at {}, which no search.log.pruned row in the ",
                        "audit log names: rows were deleted without a report"
                    ),
                    &start[..start.len().min(12)]
                )));
            }
            if let Some(newest) = reported.last()
                && *newest != start
                && !rows.iter().any(|r| r.chain_hash() == Some(newest.as_str()))
            {
                return Err(Error::Index(format!(
                    concat!(
                        "the newest search.log.pruned row names {}, which the search log ",
                        "neither starts at nor contains"
                    ),
                    &newest[..newest.len().min(12)]
                )));
            }
            let report = cyberbrain_policy::verify_chain_report(&start, &rows)?;
            self.check_row_keys(&report)?;
            Ok(Some(report))
        };
        match check() {
            Ok(Some(r)) => Some(Ok(r)),
            Ok(None) => None,
            Err(e) => Some(Err(e.to_string())),
        }
    }

    /// `policy audit --searches`: the search log, as `policy audit` shows the main one. The
    /// reading is recorded in the main log, which is where the operator's actions belong.
    pub fn policy_search_audit(
        &self,
        filter: &AuditFilter,
        verify: bool,
        format: ExportFormat,
    ) -> Result<AuditView> {
        self.refuse_limited_audit_reader("policy audit")?;
        let checked = verify.then(|| self.verify_search_log());
        let rows = match self.existing_search_log()? {
            Some((_, sink)) => cyberbrain_policy::AuditSink::read(sink.as_ref(), filter)?,
            None => Vec::new(),
        };
        let rendered = cyberbrain_policy::audit::render(&rows, format);
        self.policy.audit().record(
            &self.actor,
            AuditAction::AuditExport,
            "audit",
            json!({ "rows": rows.len(), "format": format.as_str(), "log": SEARCH_LOG_FILE }),
        )?;
        let (verified, chain) = match checked.flatten() {
            None if verify => (Some(Ok(0)), None),
            None => (None, None),
            Some(Ok(r)) => (Some(Ok(r.rows)), Some(r)),
            Some(Err(e)) => (Some(Err(e)), None),
        };
        Ok(AuditView {
            rows: rows.len(),
            verified,
            chain,
            searches_verified: None,
            rendered,
        })
    }

    /// `policy audit --searches --export`: a bundle of the search log, checkable like one of
    /// the main log. Its anchor is where the selected rows chain from.
    pub fn export_search_bundle(&self, filter: &AuditFilter, signed: bool) -> Result<String> {
        self.refuse_limited_audit_reader("an audit export")?;
        let key = match (signed, self.policy.audit().key()) {
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
        let rows: Vec<AuditEvent> = match self.existing_search_log()? {
            Some((_, sink)) => cyberbrain_policy::AuditSink::read(sink.as_ref(), filter)?,
            None => Vec::new(),
        };
        let out = cyberbrain_policy::audit::bundle::render_with(
            &rows,
            &cyberbrain_policy::audit::bundle::Spec {
                from: filter.since.map(|t| t.to_string()),
                to: filter.until.map(|t| t.to_string()),
                tool: concat!("cyberbrain ", env!("CARGO_PKG_VERSION")).to_string(),
                exported_at: jiff::Timestamp::now().to_string(),
                selection: cyberbrain_policy::audit::bundle::Selection::of(filter),
                key,
            },
        );
        self.policy.audit().record(
            &self.actor,
            AuditAction::AuditExport,
            "audit",
            json!({ "rows": rows.len(), "format": "bundle", "signed": key.is_some(),
                    "log": SEARCH_LOG_FILE }),
        )?;
        Ok(out)
    }

    /// The audit log is the operator's. A limited client is refused it like `status`: it
    /// names other clients, their bereiche and how much they searched.
    pub(super) fn refuse_limited_audit_reader(&self, what: &str) -> Result<()> {
        if self.client_scope().is_some() {
            return Err(Error::PolicyRefusal {
                profile: "clients".to_string(),
                reason: format!(
                    "{} is limited to bereiche in [clients.bereiche]; {what} reads the audit \
                     log of the whole store and is not shown to it",
                    self.actor
                ),
            });
        }
        Ok(())
    }
}

/// `[clients]` in the one form rows are compared in: client names trimmed (as the limit
/// matches them), bereiche sorted and without repeats, so reordering a list is no change.
fn clients_state(c: &cyberbrain_core::config::ClientsConfig) -> Value {
    let mut bereiche: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (client, list) in &c.bereiche {
        bereiche
            .entry(client.trim().to_string())
            .or_default()
            .extend(list.iter().cloned());
    }
    json!({
        "unlisted": serde_json::to_value(c.unlisted).unwrap_or(Value::Null),
        "bereiche": bereiche,
    })
}

/// The store's search key, made on first use. If two processes make one at the same moment,
/// the file that ends up on disk is read back and used by both from then on.
fn load_or_create_key(root: &std::path::Path) -> Option<[u8; 32]> {
    let path = root.join(SEARCH_KEY_FILE);
    let parse = |text: &str| -> Option<[u8; 32]> {
        let t = text.trim();
        if t.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(out)
    };
    if let Ok(text) = std::fs::read_to_string(&path) {
        return parse(&text);
    }
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "# Private keys. Never commit these.\n*\n");
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).ok()?;
    let hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
    crate::hub::client::write_owner_only(&path, &format!("{hex}\n")).ok()?;
    parse(&std::fs::read_to_string(&path).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberbrain_core::config::{ClientsConfig, Unlisted};

    #[test]
    fn clients_state_ignores_order_repeats_and_padding() {
        let mut a = ClientsConfig::default();
        a.bereiche.insert(
            "agent:mcp:anna".into(),
            vec!["b".into(), "a".into(), "a".into()],
        );
        let mut b = ClientsConfig::default();
        b.bereiche
            .insert(" agent:mcp:anna ".into(), vec!["a".into(), "b".into()]);
        assert_eq!(clients_state(&a), clients_state(&b));
        b.unlisted = Unlisted::Nothing;
        assert_ne!(clients_state(&a), clients_state(&b));
        assert_eq!(clients_state(&b)["unlisted"], "nothing");
    }

    #[test]
    fn the_search_key_is_made_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let k1 = load_or_create_key(dir.path()).unwrap();
        let k2 = load_or_create_key(dir.path()).unwrap();
        assert_eq!(k1, k2);
        assert!(dir.path().join("keys/.gitignore").exists());
    }
}
