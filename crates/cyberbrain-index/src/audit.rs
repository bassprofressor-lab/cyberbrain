//! The append-only audit log (SPEC §12.6).
//!
//! The table is owned by this crate because it lives in the same database file; the rows
//! are written mostly by `cyberbrain-policy`. The index itself records only what it alone
//! knows about: erasures, a wiped vector set, a full clear. Ordinary upserts are not
//! audited here — a scan over thousands of notes would drown the log — the `write`
//! command records its own row through [`Index::append_audit`].
//!
//! Timestamps are stamped by SQLite in UTC with millisecond precision and a fixed width,
//! so `since` filters can compare strings.

use crate::{Index, SqlResultExt};
use cyberbrain_core::Result;
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditEntry {
    pub seq: i64,
    /// `YYYY-MM-DDTHH:MM:SS.sssZ`
    pub ts: String,
    pub actor: String,
    pub action: String,
    pub subject: Option<String>,
    pub detail: Option<serde_json::Value>,
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

/// Actions the index writes itself. Policy is free to use its own vocabulary.
pub mod actions {
    pub const NOTE_ERASED: &str = "index.note-erased";
    pub const INDEX_CLEARED: &str = "index.cleared";
    pub const EMBEDDING_PROFILE_CHANGED: &str = "index.embedding-profile-changed";
}

/// Actor the index uses for its own rows.
pub const INDEX_ACTOR: &str = "cyberbrain-index";

impl Index {
    /// Append one row. Returns its `seq`.
    pub fn append_audit(
        &mut self,
        actor: &str,
        action: &str,
        subject: Option<&str>,
        detail: Option<&serde_json::Value>,
    ) -> Result<i64> {
        append(&self.conn, actor, action, subject, detail)
    }

    /// Rows matching `filter`, oldest first.
    pub fn audit(&self, filter: &AuditFilter) -> Result<Vec<AuditEntry>> {
        let mut sql =
            String::from("SELECT seq, ts, actor, action, subject, detail FROM audit WHERE 1 = 1");
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
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(AuditEntry {
                    seq: r.get(0)?,
                    ts: r.get(1)?,
                    actor: r.get(2)?,
                    action: r.get(3)?,
                    subject: r.get(4)?,
                    detail: r
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| serde_json::from_str(&s).ok()),
                })
            })
            .ix()?;
        rows.map(|r| r.ix()).collect()
    }

    /// The newest row, if any.
    pub fn last_audit(&self) -> Result<Option<AuditEntry>> {
        self.conn
            .query_row(
                "SELECT seq, ts, actor, action, subject, detail FROM audit \
                 ORDER BY seq DESC LIMIT 1",
                [],
                |r| {
                    Ok(AuditEntry {
                        seq: r.get(0)?,
                        ts: r.get(1)?,
                        actor: r.get(2)?,
                        action: r.get(3)?,
                        subject: r.get(4)?,
                        detail: r
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                    })
                },
            )
            .optional()
            .ix()
    }
}

/// Shared by `append_audit` and the index's own writes inside a transaction.
pub(crate) fn append(
    conn: &rusqlite::Connection,
    actor: &str,
    action: &str,
    subject: Option<&str>,
    detail: Option<&serde_json::Value>,
) -> Result<i64> {
    let detail = detail.map(|v| v.to_string());
    conn.execute(
        "INSERT INTO audit (ts, actor, action, subject, detail)
         VALUES (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?1, ?2, ?3, ?4)",
        params![actor, action, subject, detail],
    )
    .ix()?;
    Ok(conn.last_insert_rowid())
}
