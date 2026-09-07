//! What the hub keeps: the devices it knows and the rows they sent.
//!
//! Its own SQLite file, not a store. A store is a project's memory; this is a record of what
//! other machines did, and conflating the two would put someone else's audit trail inside a
//! thing that has `forget` as a first-class operation.
//!
//! The same rule as the store's audit log applies here and for the same reason: `BEFORE
//! UPDATE` and `BEFORE DELETE` triggers abort. A hub whose rows can be edited is a hub whose
//! evidence is worth nothing, and "we would notice" is not a control.

use cyberbrain_core::{Error, Result};
use cyberbrain_policy::AuditEvent;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// The chain state of a device that has never sent anything.
pub const GENESIS: &str = "genesis";

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
    pub last_seen: Option<String>,
    /// Hash of the last row accepted from this device. The next bundle must anchor here.
    pub anchor: String,
    pub rows: i64,
    /// Version this device last reported. `None` until it has sent one.
    pub version: Option<String>,
}

impl Device {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

pub struct HubStore {
    conn: Connection,
}

fn ix<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|e| Error::Index(format!("hub store: {e}")))
}

impl HubStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let conn = ix(Connection::open(path))?;
        let s = Self { conn };
        s.migrate()?;
        Ok(s)
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = ix(Connection::open_in_memory())?;
        let s = Self { conn };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&self) -> Result<()> {
        ix(self.conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;

             CREATE TABLE IF NOT EXISTS devices (
                 id          TEXT PRIMARY KEY,
                 name        TEXT NOT NULL,
                 token_hash  TEXT NOT NULL UNIQUE,
                 created_at  TEXT NOT NULL,
                 revoked_at  TEXT,
                 last_seen   TEXT,
                 anchor      TEXT NOT NULL,
                 rows        INTEGER NOT NULL DEFAULT 0,
                 version     TEXT
             );

             CREATE TABLE IF NOT EXISTS entries (
                 device      TEXT NOT NULL REFERENCES devices(id),
                 seq         INTEGER NOT NULL,
                 ts          TEXT NOT NULL,
                 actor       TEXT NOT NULL,
                 action      TEXT NOT NULL,
                 subject     TEXT NOT NULL,
                 detail      TEXT NOT NULL,
                 hash        TEXT NOT NULL,
                 received_at TEXT NOT NULL,
                 PRIMARY KEY (device, seq)
             );

             -- Append-only, enforced by the database rather than by everyone remembering.
             CREATE TRIGGER IF NOT EXISTS entries_no_update
                 BEFORE UPDATE ON entries
                 BEGIN SELECT raise(ABORT, 'the hub record is append-only'); END;
             CREATE TRIGGER IF NOT EXISTS entries_no_delete
                 BEFORE DELETE ON entries
                 BEGIN SELECT raise(ABORT, 'the hub record is append-only'); END;

             CREATE INDEX IF NOT EXISTS entries_by_ts ON entries(ts);",
        ))
    }

    /// Register a device. Returns it with the plaintext token, which is the only time that
    /// value exists anywhere: the table keeps a hash, so a stolen database is not a set of
    /// working credentials.
    pub fn add_device(&self, name: &str, now: &str) -> Result<(Device, String)> {
        let id = format!("dev_{}", cyberbrain_core::NoteId::generate());
        let token = format!("cbh_{}", cyberbrain_core::NoteId::generate());
        let device = Device {
            id: id.clone(),
            name: name.to_string(),
            created_at: now.to_string(),
            revoked_at: None,
            last_seen: None,
            anchor: GENESIS.to_string(),
            rows: 0,
            version: None,
        };
        ix(self.conn.execute(
            "INSERT INTO devices (id, name, token_hash, created_at, anchor)
             VALUES (?, ?, ?, ?, ?)",
            params![id, name, token_hash(&token), now, GENESIS],
        ))?;
        Ok((device, token))
    }

    pub fn device_by_token(&self, token: &str) -> Result<Option<Device>> {
        let hash = token_hash(token);
        ix(self
            .conn
            .query_row(
                "SELECT id, name, created_at, revoked_at, last_seen, anchor, rows, version
                 FROM devices WHERE token_hash = ?",
                params![hash],
                row_to_device,
            )
            .optional())
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT id, name, created_at, revoked_at, last_seen, anchor, rows, version
             FROM devices ORDER BY created_at, id",
        ))?;
        let rows = ix(stmt.query_map([], row_to_device))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    /// Revoking is a state, not a deletion: the rows a device sent stay, and so does the
    /// record of who sent them.
    pub fn revoke(&self, id: &str, now: &str) -> Result<bool> {
        let n = ix(self.conn.execute(
            "UPDATE devices SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
            params![now, id],
        ))?;
        Ok(n > 0)
    }

    /// Append verified rows for a device and move its anchor on.
    ///
    /// The caller has already checked the bundle and that its anchor matches this device's.
    /// One transaction: a half-accepted bundle would leave an anchor nobody can continue
    /// from, and the next delivery would look like tampering.
    pub fn append(
        &mut self,
        device: &Device,
        rows: &[AuditEvent],
        new_anchor: &str,
        version: Option<&str>,
        now: &str,
    ) -> Result<i64> {
        let tx = ix(self.conn.transaction())?;
        let mut seq = device.rows;
        for e in rows {
            seq += 1;
            let hash = e.chain_hash().unwrap_or_default();
            ix(tx.execute(
                "INSERT INTO entries (device, seq, ts, actor, action, subject, detail, hash, received_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    device.id,
                    seq,
                    e.ts.to_string(),
                    e.actor,
                    e.action,
                    e.subject,
                    e.detail.to_string(),
                    hash,
                    now
                ],
            ))?;
        }
        ix(tx.execute(
            "UPDATE devices SET anchor = ?, rows = ?, last_seen = ?,
                 version = coalesce(?, version)
             WHERE id = ?",
            params![new_anchor, seq, now, version, device.id],
        ))?;
        ix(tx.commit())?;
        Ok(seq)
    }

    /// Rows of one device, oldest first: sequence, timestamp and action, never the detail.
    ///
    /// Deliberately narrow. The hub holds other people's audit trails, and a convenience
    /// method that hands out whole rows is how the "collects but does not read" rule would
    /// quietly stop being true. The report in a later slice builds on this shape.
    #[allow(dead_code)] // the fleet report is the next slice; the shape is fixed here.
    pub fn entries(&self, device: &str, limit: usize) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = ix(self
            .conn
            .prepare("SELECT seq, ts, action FROM entries WHERE device = ? ORDER BY seq LIMIT ?"))?;
        let rows = ix(stmt.query_map(params![device, limit as i64], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        }))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    pub fn total_entries(&self) -> Result<i64> {
        ix(self
            .conn
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0)))
    }
}

fn row_to_device(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: r.get(0)?,
        name: r.get(1)?,
        created_at: r.get(2)?,
        revoked_at: r.get(3)?,
        last_seen: r.get(4)?,
        anchor: r.get(5)?,
        rows: r.get(6)?,
        version: r.get(7)?,
    })
}

/// Tokens are stored as a hash, like passwords, for the same reason.
fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_never_stored_in_the_clear() {
        let s = HubStore::in_memory().unwrap();
        let (_, token) = s.add_device("laptop", "2026-09-07T00:00:00Z").unwrap();
        let stored: String = s
            .conn
            .query_row("SELECT token_hash FROM devices", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored, token);
        assert_eq!(stored, token_hash(&token));
        assert!(s.device_by_token(&token).unwrap().is_some());
        assert!(s.device_by_token("cbh_wrong").unwrap().is_none());
    }

    #[test]
    fn a_new_device_starts_at_genesis() {
        let s = HubStore::in_memory().unwrap();
        let (d, _) = s.add_device("laptop", "2026-09-07T00:00:00Z").unwrap();
        assert_eq!(d.anchor, GENESIS);
        assert_eq!(d.rows, 0);
        assert!(d.is_active());
    }

    #[test]
    fn revoking_keeps_the_device_and_its_rows() {
        let s = HubStore::in_memory().unwrap();
        let (d, token) = s.add_device("laptop", "2026-09-07T00:00:00Z").unwrap();
        assert!(s.revoke(&d.id, "2026-09-08T00:00:00Z").unwrap());
        let back = s.device_by_token(&token).unwrap().unwrap();
        assert!(!back.is_active(), "a revoked device is still findable");
        // Revoking twice is not an error, but it is not a second event either.
        assert!(!s.revoke(&d.id, "2026-09-09T00:00:00Z").unwrap());
    }

    #[test]
    fn the_record_refuses_to_be_edited() {
        let mut s = HubStore::in_memory().unwrap();
        let (d, _) = s.add_device("laptop", "2026-09-07T00:00:00Z").unwrap();
        let event = AuditEvent {
            ts: "2026-09-07T00:00:01Z".parse().unwrap(),
            actor: "operator".into(),
            action: "note.write".into(),
            subject: "note:x".into(),
            detail: serde_json::json!({"_chain": {"prev": "genesis", "hash": "abc", "at": "t"}}),
        };
        s.append(&d, &[event], "abc", Some("0.2.1"), "2026-09-07T00:00:02Z")
            .unwrap();

        let update = s
            .conn
            .execute("UPDATE entries SET action = 'note.forget'", [])
            .unwrap_err()
            .to_string();
        assert!(update.contains("append-only"), "{update}");
        let delete = s
            .conn
            .execute("DELETE FROM entries", [])
            .unwrap_err()
            .to_string();
        assert!(delete.contains("append-only"), "{delete}");
    }
}
