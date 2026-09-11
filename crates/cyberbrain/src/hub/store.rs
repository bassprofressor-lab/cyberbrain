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

/// What became of an offered note.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum NoteOutcome {
    /// Taken: either the hub held nothing, or the sender built on what it holds.
    Stored,
    /// The sender re-sent what is already held. Not an error and not a change.
    Unchanged,
    /// Two machines changed it without seeing each other. Both versions are kept and
    /// somebody has to say which one stands.
    Conflict { id: String, held_updated: String },
}

/// What an erasure actually removed. Counted rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct ErasureCount {
    pub notes: usize,
    pub conflicts: usize,
}

/// Two versions of one note that nobody has reconciled yet.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct NoteConflict {
    pub id: String,
    pub bereich: String,
    pub name: String,
    pub held_updated: String,
    pub held_from_device: String,
    pub offered_updated: String,
    pub offered_from_device: String,
    pub offered_frontmatter: String,
    pub offered_body: String,
    pub based_on: Option<String>,
    pub detected_at: String,
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

/// What came of a countersignature attempt. Each one is a different sentence to the person
/// holding the credential, so they are not collapsed into a bool.
#[derive(Debug, Clone, PartialEq)]
pub enum CountersignOutcome {
    Signed,
    Unknown,
    Withdrawn,
    AlreadySigned {
        by: String,
    },
    /// The person who wrote the grant is the person trying to sign it.
    SamePerson,
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

    /// Write a consistent copy of the whole record to `to`, for `hub backup`.
    ///
    /// `VACUUM INTO`, not a file copy. The record runs in WAL mode, so the newest rows may
    /// still live in `hub.db-wal`, and a copy of `hub.db` alone opens, verifies and misses
    /// them; the first version did exactly that. This reads one snapshot through SQLite, so
    /// deliveries keep arriving while it runs, and the copy is a single file with no WAL of
    /// its own to lose. Never over an existing file: replacing last night's backup with a
    /// broken one is how a backup disappears.
    pub fn backup_to(&self, to: &Path) -> Result<()> {
        if to.exists() {
            // A user error, not an I/O failure: the command was asked for something it will
            // not do, and exit 1 says so where exit 2 would read as a crash.
            return Err(Error::Config(format!(
                "{}: a backup is never written over an existing file; pick a new name",
                to.display()
            )));
        }
        if let Some(parent) = to.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|e| Error::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let target = to.to_str().ok_or_else(|| {
            Error::Index(format!(
                "{} is not valid UTF-8, which SQLite needs for a file name",
                to.display()
            ))
        })?;
        ix(self.conn.execute("VACUUM INTO ?1", params![target]))?;
        Ok(())
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
                 -- Both NULL until a second person signs. A grant in that state is written
                 -- down and moves nothing; see `sync_access::BereichGrant::is_effective`.
                 approved_by TEXT,
                 approved_at TEXT,
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
             CREATE INDEX IF NOT EXISTS synced_notes_bereich ON synced_notes(bereich);

             -- Two machines changed the same note without seeing each other's change. The
             -- offered version is kept beside the held one rather than dropped: last-write-
             -- wins is not a resolution, it is a loss that nobody was told about.
             CREATE TABLE IF NOT EXISTS note_conflicts (
                 id                  TEXT PRIMARY KEY,
                 bereich             TEXT NOT NULL,
                 name                TEXT NOT NULL,
                 held_updated        TEXT NOT NULL,
                 held_from_device    TEXT NOT NULL,
                 offered_updated     TEXT NOT NULL,
                 offered_from_device TEXT NOT NULL,
                 offered_frontmatter TEXT NOT NULL,
                 offered_body        TEXT NOT NULL,
                 based_on            TEXT,
                 detected_at         TEXT NOT NULL,
                 resolved_at         TEXT,
                 resolution          TEXT
             );
             -- Which bereiche a person is responsible for. Only `editor` principals have
             -- these: an admin has none and gets none, because seeing note text is not part
             -- of running the machine.
             CREATE TABLE IF NOT EXISTS principal_bereiche (
                 principal  TEXT NOT NULL REFERENCES principals(id),
                 bereich    TEXT NOT NULL,
                 added_at   TEXT NOT NULL,
                 PRIMARY KEY (principal, bereich)
             );

             CREATE INDEX IF NOT EXISTS note_conflicts_open
                 ON note_conflicts(bereich, name) WHERE resolved_at IS NULL;

             -- A note that was erased. Deliberately carries no text: a record that an
             -- erasure happened must not be a copy of what was erased. It exists so a
             -- machine that delivers the note again learns it was withdrawn, rather than
             -- quietly recreating it (GDPR Art. 17).
             CREATE TABLE IF NOT EXISTS erasures (
                 bereich     TEXT NOT NULL,
                 name        TEXT NOT NULL,
                 erased_at   TEXT NOT NULL,
                 by_device   TEXT NOT NULL,
                 PRIMARY KEY (bereich, name)
             );",
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
        // Only ever additive, and only with a NULL default: a migration that rewrites rows
        // in an append-only record is a contradiction.
        //
        // `approved_by`/`approved_at` arriving NULL is the point rather than a side effect.
        // Grants written before a countersignature was required stop moving notes when the
        // hub is upgraded, and the refusal names the one step that revives them. The other
        // reading — treat what is already there as signed — would carry the hole over the
        // upgrade and call it compatibility.
        for (table, column, ddl) in [
            (
                "devices",
                "version",
                "ALTER TABLE devices ADD COLUMN version TEXT",
            ),
            (
                "devices",
                "last_refusal",
                "ALTER TABLE devices ADD COLUMN last_refusal TEXT",
            ),
            (
                "devices",
                "last_refusal_at",
                "ALTER TABLE devices ADD COLUMN last_refusal_at TEXT",
            ),
            (
                "bereich_grants",
                "approved_by",
                "ALTER TABLE bereich_grants ADD COLUMN approved_by TEXT",
            ),
            (
                "bereich_grants",
                "approved_at",
                "ALTER TABLE bereich_grants ADD COLUMN approved_at TEXT",
            ),
        ] {
            if !self.has_column(table, column)? {
                ix(self.conn.execute(ddl, []))?;
            }
        }
        Ok(())
    }

    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        let mut stmt = ix(self.conn.prepare(&format!("PRAGMA table_info({table})")))?;
        let names = ix(stmt.query_map([], |r| r.get::<_, String>(1)))?;
        for n in names {
            if ix(n)? == column {
                return Ok(true);
            }
        }
        Ok(false)
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
        // In the chain, like a role grant, and for the same reason: a new device is a new
        // pair of eyes on whatever it is later granted, and it used to appear out of
        // nothing. Recorded inside the store rather than at the two call sites, so neither
        // the web form nor `hub add` can be the one that forgets.
        self.record(
            "hub",
            "device.registered",
            serde_json::json!({ "device": device.id, "name": name }),
            now,
        )?;
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
        // Only when something changed: revoking twice is not two events, and a log that
        // says otherwise is a log somebody has to explain.
        if n > 0 {
            self.record(
                "hub",
                "device.revoked",
                serde_json::json!({ "device": id }),
                now,
            )?;
        }
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
    // ----- note sync: grants and the notes themselves ------------------------------------

    /// Every grant recorded for a device, revoked ones included. `sync_access::may_move`
    /// needs the withdrawn ones to tell "never granted" from "taken away", which are
    /// different answers to whoever reads the refusal.
    pub fn grants_for_device(&self, device: &str) -> Result<Vec<super::sync_access::BereichGrant>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT id, device, bereich, direction, reason, granted_by, created_at,
                    approved_by, approved_at, revoked_at
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
                r.get::<_, Option<String>>(8)?,
                r.get::<_, Option<String>>(9)?,
            ))
        }))?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                device,
                bereich,
                direction,
                reason,
                granted_by,
                created_at,
                approved_by,
                approved_at,
                revoked_at,
            ) = ix(row)?;
            out.push(super::sync_access::BereichGrant {
                id,
                device,
                bereich,
                direction: super::sync_access::Direction::parse(&direction)?,
                reason,
                granted_by,
                created_at,
                approved_by,
                approved_at,
                revoked_at,
            });
        }
        Ok(out)
    }

    /// One grant, by id.
    pub fn grant(&self, id: &str) -> Result<Option<super::sync_access::BereichGrant>> {
        let device: Option<String> = ix(self
            .conn
            .query_row(
                "SELECT device FROM bereich_grants WHERE id = ?",
                params![id],
                |r| r.get(0),
            )
            .optional())?;
        let Some(device) = device else {
            return Ok(None);
        };
        Ok(self
            .grants_for_device(&device)?
            .into_iter()
            .find(|g| g.id == id))
    }

    /// Let a grant take effect. The second of the two people a bereich takes.
    ///
    /// Refuses the person who wrote it, whatever role they hold: two signatures from one
    /// hand are one signature. Refuses a grant that is already signed, so "countersigned by"
    /// names the person who actually decided rather than the last one to run the command,
    /// and refuses a withdrawn one, because reviving it is a new decision and should look
    /// like one.
    pub fn countersign_grant(
        &self,
        id: &str,
        who: &super::access::Principal,
        now: &str,
    ) -> Result<CountersignOutcome> {
        let Some(g) = self.grant(id)? else {
            return Ok(CountersignOutcome::Unknown);
        };
        if g.revoked_at.is_some() {
            return Ok(CountersignOutcome::Withdrawn);
        }
        if let Some(by) = &g.approved_by {
            return Ok(CountersignOutcome::AlreadySigned { by: by.clone() });
        }
        if g.granted_by == who.id {
            return Ok(CountersignOutcome::SamePerson);
        }
        ix(self.conn.execute(
            "UPDATE bereich_grants SET approved_by = ?, approved_at = ?
             WHERE id = ? AND approved_at IS NULL",
            params![who.id, now, id],
        ))?;
        self.record(
            &who.id,
            "grant.countersigned",
            serde_json::json!({
                "id": g.id,
                "device": g.device,
                "bereich": g.bereich,
                "direction": g.direction.as_str(),
                "granted_by": g.granted_by,
                "by": who.name,
            }),
            now,
        )?;
        Ok(CountersignOutcome::Signed)
    }

    /// Record a grant. The caller checks that the granter is an administrator; this writes.
    ///
    /// One argument per column, and a struct to carry them would be a second name for the
    /// row that already has one.
    #[allow(clippy::too_many_arguments)]
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
            params![
                id,
                device,
                bereich,
                direction.as_str(),
                reason,
                granted_by,
                now
            ],
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

    /// What became of one offered note.
    #[allow(clippy::too_many_arguments)]
    pub fn offer_synced_note(
        &self,
        id: &str,
        bereich: &str,
        name: &str,
        ring: u8,
        kind: &str,
        updated: &str,
        frontmatter: &str,
        body: &str,
        based_on: Option<&str>,
        from_device: &str,
        now: &str,
    ) -> Result<NoteOutcome> {
        let held: Option<(String, String)> = ix(self
            .conn
            .query_row(
                "SELECT updated, from_device FROM synced_notes WHERE bereich = ? AND name = ?",
                params![bereich, name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional())?;

        match held {
            // Nothing held: nothing to conflict with.
            None => {
                self.write_synced_note(
                    id,
                    bereich,
                    name,
                    ring,
                    kind,
                    updated,
                    frontmatter,
                    body,
                    from_device,
                    now,
                )?;
                Ok(NoteOutcome::Stored)
            }
            Some((held_updated, held_device)) => {
                // The sender says which version it started from. Equal means it saw what the
                // hub holds and moved on from there: a continuation, and safe to take.
                if based_on == Some(held_updated.as_str()) {
                    self.write_synced_note(
                        id,
                        bereich,
                        name,
                        ring,
                        kind,
                        updated,
                        frontmatter,
                        body,
                        from_device,
                        now,
                    )?;
                    return Ok(NoteOutcome::Stored);
                }
                // Byte-identical to what is held is not a conflict, it is a re-send.
                if updated == held_updated {
                    return Ok(NoteOutcome::Unchanged);
                }
                // Anything else is two machines that did not see each other. Keep both.
                let cid = format!("nc_{}", cyberbrain_core::NoteId::generate());
                ix(self.conn.execute(
                    "INSERT INTO note_conflicts
                        (id, bereich, name, held_updated, held_from_device, offered_updated,
                         offered_from_device, offered_frontmatter, offered_body, based_on,
                         detected_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        cid,
                        bereich,
                        name,
                        held_updated,
                        held_device,
                        updated,
                        from_device,
                        frontmatter,
                        body,
                        based_on,
                        now
                    ],
                ))?;
                Ok(NoteOutcome::Conflict {
                    id: cid,
                    held_updated,
                })
            }
        }
    }

    /// Hold a note on behalf of a bereich, unconditionally. Callers reach this through
    /// `offer_synced_note`, which is where the decision lives.
    #[allow(clippy::too_many_arguments)]
    fn write_synced_note(
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
    ) -> Result<()> {
        ix(self.conn.execute(
            "INSERT INTO synced_notes
                (id, bereich, name, ring, kind, updated, frontmatter, body, from_device,
                 received_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(bereich, name) DO UPDATE SET
                id = excluded.id, ring = excluded.ring, kind = excluded.kind,
                updated = excluded.updated, frontmatter = excluded.frontmatter,
                body = excluded.body, from_device = excluded.from_device,
                received_at = excluded.received_at",
            params![
                id,
                bereich,
                name,
                ring,
                kind,
                updated,
                frontmatter,
                body,
                from_device,
                now
            ],
        ))?;
        Ok(())
    }

    /// Erase a note the hub holds, everywhere it holds it.
    ///
    /// Four places carry the text, not one: the held note's body and frontmatter, and the
    /// offered body and frontmatter of every conflict about it. An erasure that only clears
    /// `synced_notes` leaves the text sitting in a conflict row, which is the same data with
    /// a different column name.
    ///
    /// Returns how many rows in each, so the caller can say what was actually removed rather
    /// than assert that something was.
    pub fn erase_note(
        &self,
        bereich: &str,
        name: &str,
        by_device: &str,
        now: &str,
    ) -> Result<ErasureCount> {
        let notes = ix(self.conn.execute(
            "DELETE FROM synced_notes WHERE bereich = ? AND name = ?",
            params![bereich, name],
        ))?;
        let conflicts = ix(self.conn.execute(
            "DELETE FROM note_conflicts WHERE bereich = ? AND name = ?",
            params![bereich, name],
        ))?;
        // The tombstone carries no text. It is what stops the next delivery from recreating
        // what somebody asked to have removed.
        ix(self.conn.execute(
            "INSERT INTO erasures (bereich, name, erased_at, by_device)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(bereich, name) DO UPDATE SET
                erased_at = excluded.erased_at, by_device = excluded.by_device",
            params![bereich, name, now, by_device],
        ))?;
        Ok(ErasureCount { notes, conflicts })
    }

    /// Was this note erased, and when? Checked before taking a delivery, so a machine that
    /// still has its own copy cannot put it back.
    pub fn erased_at(&self, bereich: &str, name: &str) -> Result<Option<String>> {
        ix(self
            .conn
            .query_row(
                "SELECT erased_at FROM erasures WHERE bereich = ? AND name = ?",
                params![bereich, name],
                |r| r.get::<_, String>(0),
            )
            .optional())
    }

    /// Every text column of every table, concatenated. Only for the erasure test, which
    /// checks the whole record rather than a list of columns: a table added later must not
    /// be able to reintroduce erased text without that test noticing.
    #[cfg(test)]
    pub fn dump_all_text(&self) -> Result<String> {
        let mut out = String::new();
        let mut tables = Vec::new();
        {
            let mut stmt = ix(self
                .conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'"))?;
            let rows = ix(stmt.query_map([], |r| r.get::<_, String>(0)))?;
            for r in rows {
                tables.push(ix(r)?);
            }
        }
        for t in tables {
            let mut stmt = ix(self.conn.prepare(&format!("SELECT * FROM \"{t}\"")))?;
            let cols = stmt.column_count();
            let rows = ix(stmt.query_map([], move |r| {
                let mut line = String::new();
                for i in 0..cols {
                    if let Ok(v) = r.get::<_, String>(i) {
                        line.push_str(&v);
                        line.push('\n');
                    }
                }
                Ok(line)
            }))?;
            for r in rows {
                out.push_str(&ix(r)?);
            }
        }
        Ok(out)
    }

    /// Notes a device may fetch: everything held in the bereiche it holds a receive grant
    /// in, changed since `since`. The filter is by grant and not by request, so asking for a
    /// bereich you were not granted returns nothing rather than an error — a fetch is not a
    /// place to learn which departments exist.
    pub fn notes_for_device(&self, device: &str, since: Option<&str>) -> Result<Vec<SyncedNote>> {
        let grants = self.grants_for_device(device)?;
        let mut out = Vec::new();
        // `is_effective`, not `is_active`: a grant nobody countersigned is written down and
        // inert. These two loops are the reading side of the same rule `may_move` states,
        // and they answer without asking it — so the rule has to hold here in its own
        // right, or the operator writes themselves a grant and reads the department.
        for g in grants.iter().filter(|g| {
            g.is_effective()
                && matches!(
                    g.direction,
                    super::sync_access::Direction::Receive | super::sync_access::Direction::Both
                )
        }) {
            for n in self.synced_notes(&g.bereich)? {
                if let Some(s) = since
                    && n.updated.as_str() <= s
                {
                    continue;
                }
                out.push(n);
            }
        }
        Ok(out)
    }

    /// Erasures in the bereiche a device may receive. Sent alongside the notes so a puller
    /// learns that something was withdrawn, not merely that it stopped being offered —
    /// which are indistinguishable if only present notes travel.
    pub fn erasures_for_device(
        &self,
        device: &str,
        since: Option<&str>,
    ) -> Result<Vec<(String, String, String)>> {
        let grants = self.grants_for_device(device)?;
        let mut out = Vec::new();
        // `is_effective`, not `is_active`: a grant nobody countersigned is written down and
        // inert. These two loops are the reading side of the same rule `may_move` states,
        // and they answer without asking it — so the rule has to hold here in its own
        // right, or the operator writes themselves a grant and reads the department.
        for g in grants.iter().filter(|g| {
            g.is_effective()
                && matches!(
                    g.direction,
                    super::sync_access::Direction::Receive | super::sync_access::Direction::Both
                )
        }) {
            let mut stmt = ix(self.conn.prepare(
                "SELECT bereich, name, erased_at FROM erasures
                 WHERE bereich = ? AND (?2 IS NULL OR erased_at > ?2)
                 ORDER BY erased_at",
            ))?;
            let rows = ix(stmt.query_map(params![g.bereich, since], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            }))?;
            for r in rows {
                out.push(ix(r)?);
            }
        }
        Ok(out)
    }

    /// Put a person in charge of a bereich. Only meaningful for an `editor`; the caller
    /// checks the role, this writes.
    pub fn assign_bereich(&self, principal: &str, bereich: &str, now: &str) -> Result<()> {
        ix(self.conn.execute(
            "INSERT INTO principal_bereiche (principal, bereich, added_at) VALUES (?, ?, ?)
             ON CONFLICT(principal, bereich) DO NOTHING",
            params![principal, bereich, now],
        ))?;
        Ok(())
    }

    /// The bereiche a person is responsible for.
    pub fn bereiche_of(&self, principal: &str) -> Result<Vec<String>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT bereich FROM principal_bereiche WHERE principal = ? ORDER BY bereich",
        ))?;
        let rows = ix(stmt.query_map(params![principal], |r| r.get::<_, String>(0)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    /// Open conflicts this person may see: the ones in their bereiche, and no others.
    /// Filtering here rather than at the page means a mistake in a template cannot widen it.
    ///
    /// The held text is fetched alongside. A conflict row holds only the version that was
    /// turned away; showing that on its own asks somebody to choose between a text and a
    /// blank, which is not a choice.
    pub fn conflicts_for_principal(&self, principal: &str) -> Result<Vec<(NoteConflict, String)>> {
        let mut out = Vec::new();
        for b in self.bereiche_of(principal)? {
            for c in self.open_conflicts(&b)? {
                let held: Option<String> = ix(self
                    .conn
                    .query_row(
                        "SELECT body FROM synced_notes WHERE bereich = ? AND name = ?",
                        params![c.bereich, c.name],
                        |r| r.get(0),
                    )
                    .optional())?;
                let held = held.unwrap_or_else(|| {
                    "(the held version is no longer here — it was erased or replaced)".into()
                });
                out.push((c, held));
            }
        }
        Ok(out)
    }

    /// One conflict, but only if this person is responsible for its bereich.
    pub fn conflict_for_principal(
        &self,
        principal: &str,
        id: &str,
    ) -> Result<Option<NoteConflict>> {
        Ok(self
            .conflicts_for_principal(principal)?
            .into_iter()
            .map(|(c, _)| c)
            .find(|c| c.id == id))
    }

    /// Conflicts nobody has decided yet. Open ones only: a resolved conflict is history and
    /// belongs in the log, not in a list of things waiting for a person.
    pub fn open_conflicts(&self, bereich: &str) -> Result<Vec<NoteConflict>> {
        let mut stmt = ix(self.conn.prepare(
            "SELECT id, bereich, name, held_updated, held_from_device, offered_updated,
                    offered_from_device, offered_frontmatter, offered_body, based_on, detected_at
             FROM note_conflicts
             WHERE bereich = ? AND resolved_at IS NULL
             ORDER BY detected_at",
        ))?;
        let rows = ix(stmt.query_map(params![bereich], |r| {
            Ok(NoteConflict {
                id: r.get(0)?,
                bereich: r.get(1)?,
                name: r.get(2)?,
                held_updated: r.get(3)?,
                held_from_device: r.get(4)?,
                offered_updated: r.get(5)?,
                offered_from_device: r.get(6)?,
                offered_frontmatter: r.get(7)?,
                offered_body: r.get(8)?,
                based_on: r.get(9)?,
                detected_at: r.get(10)?,
            })
        }))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(ix(r)?);
        }
        Ok(out)
    }

    /// Settle one conflict. `take_offered` replaces what is held with the version that was
    /// turned away; otherwise the held version stands. Either way the conflict is closed
    /// with a note of which way it went, so the decision is not folded into the data.
    pub fn resolve_conflict(&self, id: &str, take_offered: bool, now: &str) -> Result<bool> {
        let c: Option<NoteConflict> = ix(self
            .conn
            .query_row(
                "SELECT id, bereich, name, held_updated, held_from_device, offered_updated,
                        offered_from_device, offered_frontmatter, offered_body, based_on,
                        detected_at
                 FROM note_conflicts WHERE id = ? AND resolved_at IS NULL",
                params![id],
                |r| {
                    Ok(NoteConflict {
                        id: r.get(0)?,
                        bereich: r.get(1)?,
                        name: r.get(2)?,
                        held_updated: r.get(3)?,
                        held_from_device: r.get(4)?,
                        offered_updated: r.get(5)?,
                        offered_from_device: r.get(6)?,
                        offered_frontmatter: r.get(7)?,
                        offered_body: r.get(8)?,
                        based_on: r.get(9)?,
                        detected_at: r.get(10)?,
                    })
                },
            )
            .optional())?;
        let Some(c) = c else { return Ok(false) };
        if take_offered {
            ix(self.conn.execute(
                "UPDATE synced_notes
                 SET updated = ?, frontmatter = ?, body = ?, from_device = ?, received_at = ?
                 WHERE bereich = ? AND name = ?",
                params![
                    c.offered_updated,
                    c.offered_frontmatter,
                    c.offered_body,
                    c.offered_from_device,
                    now,
                    c.bereich,
                    c.name
                ],
            ))?;
        }
        ix(self.conn.execute(
            "UPDATE note_conflicts SET resolved_at = ?, resolution = ? WHERE id = ?",
            params![now, if take_offered { "offered" } else { "held" }, id],
        ))?;
        Ok(true)
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

    /// Append to the hub's own chain. Every call in this file that changes who may see what
    /// goes through here, so "it happened but was not recorded" is not a reachable state.
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
