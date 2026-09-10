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
    /// Why this device's last delivery was turned away, if one was.
    pub last_refusal: Option<String>,
    pub last_refusal_at: Option<String>,
}

impl Device {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// One note the hub holds for a bereich.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SyncedNote {
    pub id: String,
    pub bereich: String,
    pub name: String,
    pub ring: u8,
    pub kind: String,
    pub updated: String,
    pub frontmatter: String,
    pub body: String,
    pub from_device: String,
}

pub struct HubStore {
    // Private, except to the tests in this module, which need to simulate the one attack
    // the triggers cannot stop: dropping them and rewriting a row. That is exactly the
    // shape the chain exists to catch, so it has to be reachable to prove it.
    #[cfg(not(test))]
    conn: Connection,
    #[cfg(test)]
    pub(super) conn: Connection,
}

fn ix<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|e| Error::Index(format!("hub store: {}", explain(e))))
}

/// SQLite's own words, plus what to do about them where we know.
///
/// "attempt to write a readonly database" is the message a person gets for running `hub add`
/// in an ordinary prompt: the record belongs to the service account, everybody else may read
/// it, and SQLite therefore opened it read-only. The sentence is accurate and tells the
/// reader nothing they can act on, which for a command they typed on purpose is the same as
/// telling them nothing.
pub(super) fn explain(e: rusqlite::Error) -> String {
    let text = e.to_string();
    let readonly = matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ReadOnly,
                ..
            },
            _
        )
    );
    if !readonly {
        return text;
    }
    let hint = if cfg!(windows) {
        concat!(
            "the record belongs to the account the hub service runs as, and this prompt is ",
            "not elevated. Open one with Run as administrator and try again."
        )
    } else {
        concat!(
            "the record belongs to the account the hub runs as. Try again as that user, or ",
            "with sudo."
        )
    };
    format!("{text} — {hint}")
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
                 version     TEXT,
                 -- The last delivery this device made that was turned away, and why.
                 -- Without it a gap is invisible: a delivery that does not continue the
                 -- chain is refused, so it leaves no rows — and the fleet view would show a
                 -- device that simply went quiet, which is a different problem with a
                 -- different fix.
                 last_refusal    TEXT,
                 last_refusal_at TEXT
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

             CREATE INDEX IF NOT EXISTS entries_by_ts ON entries(ts);

             -- One row, holding the licence text. In the record rather than a file beside
             -- it so that moving the hub moves its licence with it.
             CREATE TABLE IF NOT EXISTS settings (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );

             -- People, as opposed to machines. Same token discipline as devices.
             CREATE TABLE IF NOT EXISTS principals (
                 id         TEXT PRIMARY KEY,
                 name       TEXT NOT NULL,
                 role       TEXT NOT NULL,
                 token_hash TEXT NOT NULL UNIQUE,
                 created_at TEXT NOT NULL,
                 revoked_at TEXT
             );

             -- Requests to read activity, and what became of them.
             CREATE TABLE IF NOT EXISTS access_requests (
                 id           TEXT PRIMARY KEY,
                 requester    TEXT NOT NULL REFERENCES principals(id),
                 device       TEXT,
                 from_ts      TEXT,
                 to_ts        TEXT,
                 reason       TEXT NOT NULL,
                 created_at   TEXT NOT NULL,
                 approved_by  TEXT REFERENCES principals(id),
                 approved_at  TEXT,
                 expires_at   TEXT,
                 disclosures  INTEGER NOT NULL DEFAULT 0
             );

             -- The hub own events: roles granted, requests made, approvals, disclosures.
             -- Its own chain, because these are the hub actions rather than any device
             -- rows, and asking who looked -- and whether anyone removed that afterwards --
             -- needs the same answer as every other row here.
             CREATE TABLE IF NOT EXISTS hub_audit (
                 seq    INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts     TEXT NOT NULL,
                 actor  TEXT NOT NULL,
                 action TEXT NOT NULL,
                 detail TEXT NOT NULL,
                 prev   TEXT NOT NULL,
                 hash   TEXT NOT NULL
             );
             CREATE TRIGGER IF NOT EXISTS hub_audit_no_update
                 BEFORE UPDATE ON hub_audit
                 BEGIN SELECT raise(ABORT, 'the hub audit is append-only'); END;
             CREATE TRIGGER IF NOT EXISTS hub_audit_no_delete
                 BEFORE DELETE ON hub_audit
                 BEGIN SELECT raise(ABORT, 'the hub audit is append-only'); END;

             -- Which bereich a device may send or receive, and why. The reason is not
             -- decoration: a department boundary is a purpose limitation, and a purpose
             -- nobody wrote down cannot be shown to anybody later.
             CREATE TABLE IF NOT EXISTS bereich_grants (
                 id         TEXT PRIMARY KEY,
                 device     TEXT NOT NULL REFERENCES devices(id),
                 bereich    TEXT NOT NULL,
                 direction  TEXT NOT NULL CHECK (direction IN ('send','receive','both')),
                 reason     TEXT NOT NULL,
                 granted_by TEXT NOT NULL,
                 created_at TEXT NOT NULL,
                 revoked_at TEXT
             );
             CREATE INDEX IF NOT EXISTS bereich_grants_device
                 ON bereich_grants(device, bereich);

             -- Notes the hub holds on behalf of a bereich. The hub is a relay, not the
             -- authority: `name` is unique per bereich, and the newest `updated` wins, so a
             -- hub that loses this table costs a re-push and not a decision.
             CREATE TABLE IF NOT EXISTS synced_notes (
                 id          TEXT NOT NULL,
                 bereich     TEXT NOT NULL,
                 name        TEXT NOT NULL,
                 ring        INTEGER NOT NULL CHECK (ring BETWEEN 2 AND 4),
                 kind        TEXT NOT NULL,
                 updated     TEXT NOT NULL,
                 body        TEXT NOT NULL,
                 frontmatter TEXT NOT NULL,
                 from_device TEXT NOT NULL REFERENCES devices(id),
                 received_at TEXT NOT NULL,
                 PRIMARY KEY (bereich, name)
             );
             CREATE INDEX IF NOT EXISTS synced_notes_bereich ON synced_notes(bereich);",
        ))?;
        self.add_missing_columns()
    }

    /// Bring an existing record up to the current shape.
    ///
    /// `CREATE TABLE IF NOT EXISTS` does nothing to a table that already exists, so a hub
    /// upgraded in place would keep the columns it was created with and fail on the first
    /// query that names a new one. This record is meant to hold a decade of evidence;
    /// upgrading the program must not mean starting it over.
    fn add_missing_columns(&self) -> Result<()> {
        let mut have = std::collections::BTreeSet::new();
        {
            let mut stmt = ix(self.conn.prepare("PRAGMA table_info(devices)"))?;
            let names = ix(stmt.query_map([], |r| r.get::<_, String>(1)))?;
            for n in names {
                have.insert(ix(n)?);
            }
        }
        // Only ever additive, and only with a NULL default: a migration that rewrites rows
        // in an append-only record is a contradiction.
        for (name, ddl) in [
            ("version", "ALTER TABLE devices ADD COLUMN version TEXT"),
            (
                "last_refusal",
                "ALTER TABLE devices ADD COLUMN last_refusal TEXT",
            ),
            (
                "last_refusal_at",
                "ALTER TABLE devices ADD COLUMN last_refusal_at TEXT",
            ),
        ] {
            if !have.contains(name) {
                ix(self.conn.execute(ddl, []))?;
            }
        }
        Ok(())
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
            last_refusal: None,
            last_refusal_at: None,
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
                "SELECT id, name, created_at, revoked_at, last_seen, anchor, rows, version,
                        last_refusal, last_refusal_at
                 FROM devices WHERE token_hash = ?",
                params![hash],
                row_to_device,
            )
            .optional())
    }

    pub fn devices(&self) -> Result<Vec<Device>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT id, name, created_at, revoked_at, last_seen, anchor, rows, version,
                    last_refusal, last_refusal_at
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
            // A successful delivery clears the refusal: the device is not in that state any
            // more, and a stale complaint in the fleet view is worse than none.
            "UPDATE devices SET anchor = ?, rows = ?, last_seen = ?,
                 version = coalesce(?, version),
                 last_refusal = NULL, last_refusal_at = NULL
             WHERE id = ?",
            params![new_anchor, seq, now, version, device.id],
        ))?;
        ix(tx.commit())?;
        Ok(seq)
    }

    /// Record that a delivery was turned away. Also counts as contact: the device did
    /// reach us, it just could not be taken.
    pub fn note_refusal(&self, device: &str, reason: &str, now: &str) -> Result<()> {
        ix(self.conn.execute(
            "UPDATE devices SET last_refusal = ?, last_refusal_at = ?, last_seen = ?
             WHERE id = ?",
            params![reason, now, now, device],
        ))
        .map(|_| ())
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

    /// One row of the settings table, for the things that are not the licence.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        ix(self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?",
                params![key],
                |r| r.get(0),
            )
            .optional())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        ix(self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        ))
        .map(|_| ())
    }

    pub fn clear_setting(&self, key: &str) -> Result<()> {
        ix(self
            .conn
            .execute("DELETE FROM settings WHERE key = ?", params![key]))
        .map(|_| ())
    }

    /// The installed licence text, if there is one.
    pub fn licence_text(&self) -> Result<Option<String>> {
        ix(self
            .conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'licence'",
                [],
                |r| r.get(0),
            )
            .optional())
    }

    pub fn set_licence(&self, text: &str) -> Result<()> {
        ix(self.conn.execute(
            "INSERT INTO settings (key, value) VALUES ('licence', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![text],
        ))
        .map(|_| ())
    }

    /// Devices that count against the seat limit: everything not revoked.
    ///
    /// Revoked devices are excluded on purpose — a seat freed by someone leaving should be
    /// usable, and their rows stay either way.
    pub fn active_device_count(&self) -> Result<usize> {
        ix(self.conn.query_row(
            "SELECT count(*) FROM devices WHERE revoked_at IS NULL",
            [],
            |r| r.get::<_, i64>(0),
        ))
        .map(|n| n as usize)
    }

    /// One device's rows, rebuilt as audit events in the order they were accepted.
    ///
    /// The detail column holds the row's JSON exactly as it arrived, `_chain` included, so
    /// what comes back out is what the client signed into its chain — which is the only
    /// reason a report can be re-verified by somebody else.
    pub fn rows_of(&self, device: &str) -> Result<Vec<AuditEvent>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT ts, actor, action, subject, detail FROM entries
             WHERE device = ? ORDER BY seq",
        ))?;
        let rows = ix(stmt.query_map(params![device], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        }))?;
        let mut out = Vec::new();
        for r in rows {
            let (ts, actor, action, subject, detail) = ix(r)?;
            out.push(AuditEvent {
                ts: ts
                    .parse()
                    .map_err(|e| Error::Index(format!("hub store: stored ts {ts:?}: {e}")))?,
                actor,
                action,
                subject,
                detail: serde_json::from_str(&detail)
                    .map_err(|e| Error::Index(format!("hub store: stored detail: {e}")))?,
            });
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
        last_refusal: r.get(8)?,
        last_refusal_at: r.get(9)?,
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

// ---------------------------------------------------------------------------------------
// People, requests, and the hub's own chain (slice 7).

use super::access::{AccessRequest, Denied, Principal, Role};

/// One event in the hub's own audit chain.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct HubEvent {
    pub seq: i64,
    pub ts: String,
    pub actor: String,
    pub action: String,
    pub detail: serde_json::Value,
    pub hash: String,
}

impl HubStore {
    /// Append to the hub's own chain. Every call in this file that changes who may see what
    /// goes through here, so "it happened but was not recorded" is not a reachable state.
    // ----- note sync: grants and the notes themselves ------------------------------------

    /// Every grant recorded for a device, revoked ones included. `sync_access::may_move`
    /// needs the withdrawn ones to tell "never granted" from "taken away", which are
    /// different answers to whoever reads the refusal.
    pub fn grants_for_device(&self, device: &str) -> Result<Vec<super::sync_access::BereichGrant>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT id, device, bereich, direction, reason, granted_by, created_at, revoked_at
             FROM bereich_grants WHERE device = ? ORDER BY created_at",
        ))?;
        let rows = ix(stmt.query_map(params![device], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        }))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, device, bereich, direction, reason, granted_by, created_at, revoked_at) =
                ix(row)?;
            out.push(super::sync_access::BereichGrant {
                id,
                device,
                bereich,
                direction: super::sync_access::Direction::parse(&direction)?,
                reason,
                granted_by,
                created_at,
                revoked_at,
            });
        }
        Ok(out)
    }

    /// Record a grant. The caller checks that the granter is an administrator; this writes.
    pub fn grant_bereich(
        &self,
        id: &str,
        device: &str,
        bereich: &str,
        direction: super::sync_access::Direction,
        reason: &str,
        granted_by: &str,
        now: &str,
    ) -> Result<()> {
        ix(self.conn.execute(
            "INSERT INTO bereich_grants
                (id, device, bereich, direction, reason, granted_by, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![id, device, bereich, direction.as_str(), reason, granted_by, now],
        ))?;
        Ok(())
    }

    pub fn revoke_grant(&self, id: &str, now: &str) -> Result<bool> {
        let n = ix(self.conn.execute(
            "UPDATE bereich_grants SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
            params![now, id],
        ))?;
        Ok(n > 0)
    }

    /// Hold a note on behalf of a bereich. Newest `updated` wins: the hub relays, it does
    /// not arbitrate, and a delivery that is older than what is held is not an error but a
    /// sender that was behind.
    pub fn put_synced_note(
        &self,
        id: &str,
        bereich: &str,
        name: &str,
        ring: u8,
        kind: &str,
        updated: &str,
        frontmatter: &str,
        body: &str,
        from_device: &str,
        now: &str,
    ) -> Result<bool> {
        let n = ix(self.conn.execute(
            "INSERT INTO synced_notes
                (id, bereich, name, ring, kind, updated, frontmatter, body, from_device,
                 received_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(bereich, name) DO UPDATE SET
                id = excluded.id, ring = excluded.ring, kind = excluded.kind,
                updated = excluded.updated, frontmatter = excluded.frontmatter,
                body = excluded.body, from_device = excluded.from_device,
                received_at = excluded.received_at
             WHERE excluded.updated > synced_notes.updated",
            params![
                id, bereich, name, ring, kind, updated, frontmatter, body, from_device, now
            ],
        ))?;
        Ok(n > 0)
    }

    /// What the hub holds for a bereich, newest first.
    pub fn synced_notes(&self, bereich: &str) -> Result<Vec<SyncedNote>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT id, bereich, name, ring, kind, updated, frontmatter, body, from_device
             FROM synced_notes WHERE bereich = ? ORDER BY updated DESC",
        ))?;
        let rows = ix(stmt.query_map(params![bereich], |r| {
            Ok(SyncedNote {
                id: r.get(0)?,
                bereich: r.get(1)?,
                name: r.get(2)?,
                ring: r.get::<_, i64>(3)? as u8,
                kind: r.get(4)?,
                updated: r.get(5)?,
                frontmatter: r.get(6)?,
                body: r.get(7)?,
                from_device: r.get(8)?,
            })
        }))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    pub fn record(
        &self,
        actor: &str,
        action: &str,
        detail: serde_json::Value,
        now: &str,
    ) -> Result<String> {
        let prev = self.last_hub_hash()?;
        let detail_text = detail.to_string();
        // Same rule as the store's audit chain: prev, timestamp, actor, action, detail,
        // each terminated, so a reader can recompute it without knowing this code.
        let mut h = blake3::Hasher::new();
        for part in [prev.as_str(), now, actor, action] {
            h.update(part.as_bytes());
            h.update(b"\n");
        }
        h.update(detail_text.as_bytes());
        let hash = h.finalize().to_hex().to_string();
        ix(self.conn.execute(
            "INSERT INTO hub_audit (ts, actor, action, detail, prev, hash)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![now, actor, action, detail_text, prev, hash],
        ))?;
        Ok(hash)
    }

    fn last_hub_hash(&self) -> Result<String> {
        ix(self
            .conn
            .query_row(
                "SELECT hash FROM hub_audit ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional())
        .map(|h| h.unwrap_or_else(|| GENESIS.to_string()))
    }

    /// The hub's own events, oldest first.
    pub fn hub_events(&self, limit: usize) -> Result<Vec<HubEvent>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT seq, ts, actor, action, detail, hash FROM hub_audit ORDER BY seq LIMIT ?",
        ))?;
        let rows = ix(stmt.query_map(params![limit as i64], |r| {
            Ok(HubEvent {
                seq: r.get(0)?,
                ts: r.get(1)?,
                actor: r.get(2)?,
                action: r.get(3)?,
                detail: serde_json::from_str(&r.get::<_, String>(4)?)
                    .unwrap_or(serde_json::Value::Null),
                hash: r.get(5)?,
            })
        }))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    /// Recompute the hub's own chain. Same question as `hub verify` asks of device rows.
    pub fn verify_hub_chain(&self) -> Result<usize> {
        let mut stmt = ix(self
            .conn
            .prepare("SELECT ts, actor, action, detail, prev, hash FROM hub_audit ORDER BY seq"))?;
        let rows = ix(stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        }))?;
        let mut prev = GENESIS.to_string();
        let mut n = 0usize;
        for row in rows {
            let (ts, actor, action, detail, stored_prev, stored_hash) = ix(row)?;
            n += 1;
            if stored_prev != prev {
                return Err(Error::Index(format!(
                    "hub audit chain broken at row {n} ({action}): a row was removed, \
                     reordered or inserted"
                )));
            }
            let mut h = blake3::Hasher::new();
            for part in [prev.as_str(), &ts, &actor, &action] {
                h.update(part.as_bytes());
                h.update(b"\n");
            }
            h.update(detail.as_bytes());
            let want = h.finalize().to_hex().to_string();
            if want != stored_hash {
                return Err(Error::Index(format!(
                    "hub audit chain broken at row {n} ({action}): the row was edited"
                )));
            }
            prev = stored_hash;
        }
        Ok(n)
    }

    pub fn add_principal(&self, name: &str, role: Role, now: &str) -> Result<(Principal, String)> {
        let id = format!("who_{}", cyberbrain_core::NoteId::generate());
        let token = format!("cbp_{}", cyberbrain_core::NoteId::generate());
        ix(self.conn.execute(
            "INSERT INTO principals (id, name, role, token_hash, created_at)
             VALUES (?, ?, ?, ?, ?)",
            params![id, name, role.as_str(), token_hash(&token), now],
        ))?;
        self.record(
            "hub",
            "role.granted",
            serde_json::json!({ "principal": id, "name": name, "role": role.as_str() }),
            now,
        )?;
        Ok((
            Principal {
                id,
                name: name.to_string(),
                role,
                created_at: now.to_string(),
                revoked_at: None,
            },
            token,
        ))
    }

    pub fn principal_by_token(&self, token: &str) -> Result<Option<Principal>> {
        let hash = token_hash(token);
        ix(self
            .conn
            .query_row(
                "SELECT id, name, role, created_at, revoked_at FROM principals
                 WHERE token_hash = ?",
                params![hash],
                row_to_principal,
            )
            .optional())
    }

    pub fn principals(&self) -> Result<Vec<Principal>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT id, name, role, created_at, revoked_at FROM principals
             ORDER BY created_at, id",
        ))?;
        let rows = ix(stmt.query_map([], row_to_principal))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    pub fn revoke_principal(&self, id: &str, now: &str) -> Result<bool> {
        let n = ix(self.conn.execute(
            "UPDATE principals SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL",
            params![now, id],
        ))?;
        if n > 0 {
            self.record(
                "hub",
                "role.revoked",
                serde_json::json!({ "principal": id }),
                now,
            )?;
        }
        Ok(n > 0)
    }

    /// Authenticate a person and check their role in one step, so no caller can do the
    /// first and forget the second.
    pub fn principal_for(
        &self,
        token: Option<&str>,
        need: Role,
    ) -> std::result::Result<Principal, Denied> {
        let token = token.ok_or_else(|| {
            Denied::NotAuthorised(
                "no credential; pass --as <token> or set CYBERBRAIN_HUB_PRINCIPAL_TOKEN".into(),
            )
        })?;
        let who = self
            .principal_by_token(token)
            .map_err(|e| Denied::NotAuthorised(format!("cannot check the credential: {e}")))?
            .ok_or_else(|| Denied::NotAuthorised("unknown credential".into()))?;
        if !who.is_active() {
            return Err(Denied::NotAuthorised(format!("{} was revoked", who.name)));
        }
        if who.role != need {
            return Err(Denied::WrongRole {
                need,
                has: who.role,
            });
        }
        Ok(who)
    }

    pub fn create_request(
        &self,
        requester: &Principal,
        device: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
        reason: &str,
        now: &str,
    ) -> Result<AccessRequest> {
        let id = format!("req_{}", cyberbrain_core::NoteId::generate());
        ix(self.conn.execute(
            "INSERT INTO access_requests (id, requester, device, from_ts, to_ts, reason, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![id, requester.id, device, from, to, reason, now],
        ))?;
        self.record(
            &requester.id,
            "access.requested",
            serde_json::json!({
                "request": id, "device": device, "from": from, "to": to, "reason": reason,
            }),
            now,
        )?;
        Ok(AccessRequest {
            id,
            requester: requester.id.clone(),
            requester_name: requester.name.clone(),
            device: device.map(str::to_owned),
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
            reason: reason.to_string(),
            created_at: now.to_string(),
            approved_by: None,
            approved_by_name: None,
            approved_at: None,
            expires_at: None,
            disclosures: 0,
        })
    }

    pub fn request(&self, id: &str) -> Result<Option<AccessRequest>> {
        ix(self
            .conn
            .query_row(
                "SELECT r.id, r.requester, p.name, r.device, r.from_ts, r.to_ts, r.reason,
                        r.created_at, r.approved_by, q.name, r.approved_at, r.expires_at,
                        r.disclosures
                 FROM access_requests r
                 JOIN principals p ON p.id = r.requester
                 LEFT JOIN principals q ON q.id = r.approved_by
                 WHERE r.id = ?",
                params![id],
                row_to_request,
            )
            .optional())
    }

    pub fn requests(&self) -> Result<Vec<AccessRequest>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT r.id, r.requester, p.name, r.device, r.from_ts, r.to_ts, r.reason,
                    r.created_at, r.approved_by, q.name, r.approved_at, r.expires_at,
                    r.disclosures
             FROM access_requests r
             JOIN principals p ON p.id = r.requester
             LEFT JOIN principals q ON q.id = r.approved_by
             ORDER BY r.created_at DESC",
        ))?;
        let rows = ix(stmt.query_map([], row_to_request))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    /// Countersign. The caller has already checked the role; this enforces the part that is
    /// about identity rather than permission.
    pub fn approve_request(
        &self,
        id: &str,
        by: &Principal,
        expires_at: &str,
        now: &str,
    ) -> std::result::Result<AccessRequest, Denied> {
        let req = self
            .request(id)
            .map_err(|e| Denied::NotAuthorised(e.to_string()))?
            .ok_or_else(|| Denied::NotApproved(id.to_string()))?;
        if req.requester == by.id {
            return Err(Denied::SamePerson);
        }
        self.conn
            .execute(
                "UPDATE access_requests SET approved_by = ?, approved_at = ?, expires_at = ?
                 WHERE id = ? AND approved_at IS NULL",
                params![by.id, now, expires_at, id],
            )
            .map_err(|e| Denied::NotAuthorised(format!("cannot record the approval: {e}")))?;
        let _ = self.record(
            &by.id,
            "access.approved",
            serde_json::json!({ "request": id, "expires_at": expires_at }),
            now,
        );
        self.request(id)
            .map_err(|e| Denied::NotAuthorised(e.to_string()))?
            .ok_or_else(|| Denied::NotApproved(id.to_string()))
    }

    /// Note that rows were handed out under a request.
    pub fn note_disclosure(&self, id: &str, by: &str, rows: usize, now: &str) -> Result<()> {
        ix(self.conn.execute(
            "UPDATE access_requests SET disclosures = disclosures + 1 WHERE id = ?",
            params![id],
        ))?;
        self.record(
            by,
            "access.disclosed",
            serde_json::json!({ "request": id, "rows": rows }),
            now,
        )?;
        Ok(())
    }
}

fn row_to_principal(r: &rusqlite::Row<'_>) -> rusqlite::Result<Principal> {
    Ok(Principal {
        id: r.get(0)?,
        name: r.get(1)?,
        role: Role::parse(&r.get::<_, String>(2)?).unwrap_or(Role::Admin),
        created_at: r.get(3)?,
        revoked_at: r.get(4)?,
    })
}

fn row_to_request(r: &rusqlite::Row<'_>) -> rusqlite::Result<AccessRequest> {
    Ok(AccessRequest {
        id: r.get(0)?,
        requester: r.get(1)?,
        requester_name: r.get(2)?,
        device: r.get(3)?,
        from: r.get(4)?,
        to: r.get(5)?,
        reason: r.get(6)?,
        created_at: r.get(7)?,
        approved_by: r.get(8)?,
        approved_by_name: r.get(9)?,
        approved_at: r.get(10)?,
        expires_at: r.get(11)?,
        disclosures: r.get(12)?,
    })
}
