//! The audit record (SPEC §12.6), in its own file.
//!
//! `cyberbrain.db` is a cache and disposable; `audit.db` is a record and is not (SPEC §4).
//! That is why this table does not live next to the index: deleting the cache must stay
//! non-destructive, and it was not while the log was inside it.
//!
//! This module is deliberately dumb. It stores rows and gives them back. The hash chain
//! (`_chain` inside `detail`) is computed and verified by `cyberbrain-policy`, which is why
//! `detail` is stored and returned **verbatim**: re-serialising the JSON would reorder keys
//! or change whitespace, the chain hash would no longer match, and `verify` would report
//! tampering that never happened. The index writes nothing here on its own; one writer,
//! one chain.

use crate::SqlResultExt;
use cyberbrain_core::{Error, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Version of the audit schema this build writes. Independent of the cache schema.
pub const AUDIT_SCHEMA_VERSION: u32 = 1;

/// Index `i` brings the schema to version `i + 1`. Never edit a published entry.
const MIGRATIONS: &[&str] = &[
    // v1: initial schema.
    r#"
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL
    );

    -- Append-only. UPDATE and DELETE are refused by trigger.
    CREATE TABLE audit (
        seq     INTEGER PRIMARY KEY AUTOINCREMENT,
        ts      TEXT NOT NULL,      -- 'YYYY-MM-DDTHH:MM:SS.sssZ', UTC, stamped by SQLite
        actor   TEXT NOT NULL,
        action  TEXT NOT NULL,
        subject TEXT,
        detail  TEXT                -- JSON, stored verbatim, or NULL
    );
    CREATE INDEX audit_ts      ON audit(ts);
    CREATE INDEX audit_action  ON audit(action);
    CREATE INDEX audit_subject ON audit(subject);
    CREATE TRIGGER audit_no_update BEFORE UPDATE ON audit
        BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
    CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
        BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;
    "#,
];

/// One row, exactly as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditEntry {
    pub seq: i64,
    /// `YYYY-MM-DDTHH:MM:SS.sssZ`
    pub ts: String,
    pub actor: String,
    pub action: String,
    pub subject: Option<String>,
    /// The JSON text exactly as it was appended. Never parsed or normalised here.
    pub detail: Option<String>,
}

/// A row to append. `seq` is always assigned by the store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewAuditEntry {
    /// When the event happened, in the column's format
    /// (`YYYY-MM-DDTHH:MM:SS.sssZ`, UTC, fixed width). `None` lets the store stamp the
    /// moment of insertion instead.
    ///
    /// A writer that keeps its own timestamp **must** supply it here. Letting the store
    /// stamp while the writer reports its own clock produces two timestamps for one event,
    /// and since the column is truncated to milliseconds while a writer's clock is not, a
    /// `since` filter built from the reported value can miss the very row it came from.
    pub ts: Option<String>,
    pub actor: String,
    pub action: String,
    pub subject: Option<String>,
    /// Must be valid JSON if present. Stored byte for byte.
    pub detail: Option<String>,
}

/// All fields optional and ANDed together.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Inclusive lower bound on `ts`, same format as the column.
    pub since: Option<String>,
    pub action: Option<String>,
    pub subject: Option<String>,
    /// Case-insensitive substring over actor, action, subject and detail. This is what
    /// `policy subject <identifier>` (SPEC §12.3) uses.
    pub contains: Option<String>,
    pub limit: Option<usize>,
}

const COLUMNS: &str = "seq, ts, actor, action, subject, detail";

fn row_to_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEntry> {
    Ok(AuditEntry {
        seq: r.get(0)?,
        ts: r.get(1)?,
        actor: r.get(2)?,
        action: r.get(3)?,
        subject: r.get(4)?,
        detail: r.get(5)?,
    })
}

/// One open `audit.db`. Not `Sync`; wrap in a mutex to share across threads.
pub struct AuditStore {
    pub(crate) conn: Connection,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for AuditStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditStore")
            .field("path", &self.path)
            .finish()
    }
}

impl AuditStore {
    /// Open or create the record at `path` and migrate it to the current schema.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
            .ix()?;
        // The record must survive a crash: FULL, unlike the cache's NORMAL.
        conn.pragma_update(None, "synchronous", "FULL").ix()?;
        Self::finish(conn, Some(path.to_path_buf()))
    }

    /// A private in-memory record. For tests and `--dry-run`.
    pub fn open_in_memory() -> Result<Self> {
        Self::finish(Connection::open_in_memory().ix()?, None)
    }

    fn finish(mut conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5)).ix()?;
        migrate(&mut conn)?;
        Ok(Self { conn, path })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Open or create a record whose oldest rows may be deleted after a retention period:
    /// the search log (SPEC §12.6.3), and nothing else. Marked as such in `meta`, and
    /// [`prune_through`](Self::prune_through) refuses any store without the mark, so the
    /// main `audit.db` cannot be pruned by calling the wrong function on it.
    pub fn open_prunable(path: &Path) -> Result<Self> {
        let s = Self::open(path)?;
        s.conn
            .execute(
                "INSERT OR IGNORE INTO meta (key, value) VALUES ('prunable', '1')",
                [],
            )
            .ix()?;
        Ok(s)
    }

    /// An in-memory prunable record, for tests.
    pub fn open_prunable_in_memory() -> Result<Self> {
        let s = Self::open_in_memory()?;
        s.conn
            .execute(
                "INSERT OR IGNORE INTO meta (key, value) VALUES ('prunable', '1')",
                [],
            )
            .ix()?;
        Ok(s)
    }

    fn meta(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ix()
    }

    /// The chain hash the first remaining row points back at, once older rows were pruned;
    /// `None` while the record still starts at the genesis marker.
    pub fn anchor(&self) -> Result<Option<String>> {
        self.meta("anchor")
    }

    /// The oldest row, if any.
    pub fn first(&self) -> Result<Option<AuditEntry>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM audit ORDER BY seq LIMIT 1"),
                [],
                row_to_entry,
            )
            .optional()
            .ix()
    }

    /// Delete every row up to and including the last one stamped before `cutoff` (same
    /// format as the column), always a prefix of the log. `report` sees the rows about to go
    /// while the write lock is held, records them elsewhere and returns the chain hash of the
    /// last of them, which becomes the new [`anchor`](Self::anchor). If `report` fails, nothing
    /// is deleted. Returns how many rows went.
    ///
    /// Refused on a store not opened with [`open_prunable`](Self::open_prunable).
    pub fn prune_through(
        &mut self,
        cutoff: &str,
        report: impl FnOnce(&[AuditEntry]) -> Result<String>,
    ) -> Result<usize> {
        if self.meta("prunable")?.as_deref() != Some("1") {
            return Err(Error::Index(
                "audit: this record is append-only; only the search log may be pruned".into(),
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| Error::Index(format!("audit: could not take the write lock: {e}")))?;
        let last_seq: Option<i64> = tx
            .query_row("SELECT max(seq) FROM audit WHERE ts < ?1", [cutoff], |r| {
                r.get(0)
            })
            .ix()?;
        let Some(last_seq) = last_seq else {
            return Ok(0);
        };
        let gone: Vec<AuditEntry> = {
            let mut stmt = tx
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM audit WHERE seq <= ?1 ORDER BY seq"
                ))
                .ix()?;
            let rows = stmt.query_map([last_seq], row_to_entry).ix()?;
            rows.collect::<rusqlite::Result<_>>().ix()?
        };
        let anchor = report(&gone)?;
        // The trigger guards against an edit; this is the one deletion the search log
        // allows, done inside the same transaction so nothing else can slip through.
        tx.execute_batch("DROP TRIGGER audit_no_delete;").ix()?;
        tx.execute("DELETE FROM audit WHERE seq <= ?1", [last_seq])
            .ix()?;
        tx.execute_batch(
            "CREATE TRIGGER audit_no_delete BEFORE DELETE ON audit
                BEGIN SELECT RAISE(ABORT, 'audit is append-only'); END;",
        )
        .ix()?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('anchor', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [&anchor],
        )
        .ix()?;
        tx.commit().ix()?;
        Ok(gone.len())
    }

    /// How long an append waits for another writer before failing with an error.
    /// Default five seconds.
    pub fn set_busy_timeout(&self, timeout: Duration) -> Result<()> {
        self.conn.busy_timeout(timeout).ix()
    }

    pub fn schema_version(&self) -> Result<u32> {
        current_version(&self.conn)
    }

    /// Append one row and return it as stored, including the timestamp SQLite assigned.
    pub fn append(&mut self, entry: &NewAuditEntry) -> Result<AuditEntry> {
        self.append_after(|_| Ok(entry.clone()))
    }

    /// Read the last row and append the one `build` derives from it, as a single
    /// `BEGIN IMMEDIATE` transaction. This is the primitive a hash chain needs: two
    /// processes appending at the same instant would otherwise both read the same
    /// predecessor and fork the chain, which `verify` would then report as a break.
    ///
    /// The write lock is held while `build` runs; keep it cheap. If `build` returns an
    /// error nothing is written and the error is returned.
    pub fn append_after(
        &mut self,
        build: impl FnOnce(Option<&AuditEntry>) -> Result<NewAuditEntry>,
    ) -> Result<AuditEntry> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| Error::Index(format!("audit: could not take the write lock: {e}")))?;
        let last = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM audit ORDER BY seq DESC LIMIT 1"),
                [],
                row_to_entry,
            )
            .optional()
            .ix()?;
        let entry = build(last.as_ref())?;
        validate(&entry)?;
        tx.execute(
            "INSERT INTO audit (ts, actor, action, subject, detail)
             VALUES (coalesce(?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')), ?2, ?3, ?4, ?5)",
            params![
                entry.ts,
                entry.actor,
                entry.action,
                entry.subject,
                entry.detail
            ],
        )
        .ix()?;
        let stored = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM audit WHERE seq = last_insert_rowid()"),
                [],
                row_to_entry,
            )
            .ix()?;
        tx.commit().ix()?;
        Ok(stored)
    }

    /// The newest row, if any.
    pub fn last(&self) -> Result<Option<AuditEntry>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM audit ORDER BY seq DESC LIMIT 1"),
                [],
                row_to_entry,
            )
            .optional()
            .ix()
    }

    pub fn get(&self, seq: i64) -> Result<Option<AuditEntry>> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM audit WHERE seq = ?1"),
                [seq],
                row_to_entry,
            )
            .optional()
            .ix()
    }

    pub fn count(&self) -> Result<usize> {
        self.conn
            .query_row("SELECT count(*) FROM audit", [], |r| r.get::<_, i64>(0))
            .ix()
            .map(|n| n as usize)
    }

    /// Rows matching `filter`, oldest first.
    pub fn read(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>> {
        let mut sql = format!("SELECT {COLUMNS} FROM audit WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(s) = &filter.since {
            sql.push_str(" AND ts >= ?");
            args.push(Box::new(s.clone()));
        }
        if let Some(a) = &filter.action {
            sql.push_str(" AND action = ?");
            args.push(Box::new(a.clone()));
        }
        if let Some(s) = &filter.subject {
            sql.push_str(" AND subject = ?");
            args.push(Box::new(s.clone()));
        }
        if let Some(c) = &filter.contains {
            sql.push_str(
                " AND instr(lower(actor || ' ' || action || ' ' || coalesce(subject, '') \
                 || ' ' || coalesce(detail, '')), lower(?)) > 0",
            );
            args.push(Box::new(c.clone()));
        }
        sql.push_str(" ORDER BY seq");
        if let Some(l) = filter.limit {
            sql.push_str(" LIMIT ?");
            args.push(Box::new(l as i64));
        }
        let mut stmt = self.conn.prepare(&sql).ix()?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), row_to_entry)
            .ix()?;
        rows.map(|r| r.ix()).collect()
    }

    /// Every row after `seq`, oldest first. For incremental verification and export.
    pub fn after(&self, seq: i64) -> Result<Vec<AuditEntry>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM audit WHERE seq > ?1 ORDER BY seq"
            ))
            .ix()?;
        let rows = stmt.query_map([seq], row_to_entry).ix()?;
        rows.map(|r| r.ix()).collect()
    }
}

/// Reject what the schema cannot express. `detail` is checked to be JSON without being
/// re-serialised, so what is stored is exactly what was given.
fn validate(entry: &NewAuditEntry) -> Result<()> {
    if entry.actor.trim().is_empty() {
        return Err(Error::Index("audit: actor must not be empty".into()));
    }
    if entry.action.trim().is_empty() {
        return Err(Error::Index("audit: action must not be empty".into()));
    }
    if let Some(d) = &entry.detail
        && let Err(e) = serde_json::from_str::<serde::de::IgnoredAny>(d)
    {
        return Err(Error::Index(format!(
            "audit: detail is not valid JSON: {e}"
        )));
    }
    Ok(())
}

fn migrate(conn: &mut Connection) -> Result<()> {
    let current = current_version(conn)?;
    if current > AUDIT_SCHEMA_VERSION {
        return Err(Error::Index(format!(
            "audit.db schema is version {current} but this build understands up to \
             {AUDIT_SCHEMA_VERSION}; upgrade cyberbrain (never delete the audit record)"
        )));
    }
    if current == AUDIT_SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .ix()?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        tx.execute_batch(sql).map_err(|e| {
            Error::Index(format!("audit migration to schema v{} failed: {e}", i + 1))
        })?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [(i + 1).to_string()],
        )
        .ix()?;
    }
    tx.commit().ix()
}

fn current_version(conn: &Connection) -> Result<u32> {
    let has_meta: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |r| r.get(0),
        )
        .ix()?;
    if has_meta == 0 {
        return Ok(0);
    }
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()
        .ix()?;
    match v {
        None => Ok(0),
        Some(s) => s
            .parse()
            .map_err(|_| Error::Index(format!("audit meta.schema_version is not a number: {s:?}"))),
    }
}

#[cfg(test)]
mod prune_tests {
    use super::*;

    fn row(ts: &str) -> NewAuditEntry {
        NewAuditEntry {
            ts: Some(ts.into()),
            actor: "agent:x".into(),
            action: "search.access".into(),
            subject: None,
            detail: Some("{}".into()),
        }
    }

    /// The main log is append-only for ever; only a record opened as prunable gives rows up.
    #[test]
    fn only_a_prunable_record_gives_up_rows_and_only_a_prefix() {
        let mut main = AuditStore::open_in_memory().unwrap();
        main.append(&row("2026-01-01T00:00:00.000Z")).unwrap();
        assert!(
            main.prune_through("2027-01-01T00:00:00.000Z", |_| Ok("h".into()))
                .is_err()
        );
        assert_eq!(main.count().unwrap(), 1);

        let mut s = AuditStore::open_prunable_in_memory().unwrap();
        for ts in [
            "2026-01-01T00:00:00.000Z",
            "2026-02-01T00:00:00.000Z",
            "2026-03-01T00:00:00.000Z",
        ] {
            s.append(&row(ts)).unwrap();
        }
        // A failing report deletes nothing.
        assert!(
            s.prune_through("2026-02-15T00:00:00.000Z", |_| Err(Error::Index(
                "no".into()
            )))
            .is_err()
        );
        assert_eq!(s.count().unwrap(), 3);
        let n = s
            .prune_through("2026-02-15T00:00:00.000Z", |rows| {
                assert_eq!(rows.len(), 2);
                Ok("anchor-hash".into())
            })
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(s.count().unwrap(), 1);
        assert_eq!(s.anchor().unwrap().as_deref(), Some("anchor-hash"));
        // The trigger is back: an ordinary delete is still refused.
        assert!(s.conn.execute("DELETE FROM audit", []).is_err());
    }
}
