//! Schema and versioned migrations (SPEC §5).
//!
//! Every migration is applied in order inside one transaction and the resulting version is
//! written to `meta.schema_version`. Opening a database that is *newer* than this build is
//! refused, because reading a schema you do not understand is how a cache lies.

use crate::SqlResultExt;
use cyberbrain_core::{Error, Result};
use rusqlite::Connection;

/// Version of the schema this build writes. Bump when appending to [`MIGRATIONS`].
pub const SCHEMA_VERSION: u32 = 3;

/// Migrations, applied in order. Index `i` brings the schema to version `i + 1`.
/// Never edit a published entry; append a new one.
const MIGRATIONS: &[&str] = &[
    // v1: initial schema.
    r#"
    CREATE TABLE meta (
        key   TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL
    );

    CREATE TABLE notes (
        id        TEXT PRIMARY KEY NOT NULL,             -- ULID, immutable
        name      TEXT NOT NULL UNIQUE,                  -- kebab-case slug
        ring      INTEGER NOT NULL CHECK (ring BETWEEN 0 AND 4),
        kind      TEXT NOT NULL,                         -- lowercase NoteKind
        path      TEXT NOT NULL,                         -- file on disk, authoritative
        created   TEXT NOT NULL,                         -- RFC 3339 UTC
        updated   TEXT NOT NULL,                         -- RFC 3339 UTC
        mtime_ns  INTEGER,                               -- file mtime, NULL if not stat-able
        size      INTEGER,                               -- file size, NULL if not stat-able
        hash      TEXT NOT NULL,                         -- blake3 hex, see content_hash()
        tags      TEXT NOT NULL DEFAULT '[]',            -- JSON array of strings
        retention TEXT,                                  -- ISO-8601 duration or NULL
        pii       TEXT NOT NULL DEFAULT 'none'           -- lowercase PiiState
    );

    CREATE TABLE blocks (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,   -- also the rowid of blocks_fts
        citation    TEXT NOT NULL UNIQUE,                -- r{ring}-{10 hex}
        note_id     TEXT NOT NULL REFERENCES notes(id),
        idx         INTEGER NOT NULL,
        text        TEXT NOT NULL,
        token_count INTEGER NOT NULL,
        UNIQUE (note_id, idx)
    );
    CREATE INDEX blocks_note ON blocks(note_id);

    -- Standalone FTS5 table. rowid is blocks.id, so rows can be removed by id without a
    -- scan. citation and ring are carried UNINDEXED so a hit and a ring filter need no join.
    CREATE VIRTUAL TABLE blocks_fts USING fts5(
        text,
        citation UNINDEXED,
        ring UNINDEXED,
        tokenize = 'unicode61 remove_diacritics 2'
    );
    -- v2 replaces this table; a fresh database gets v1 and then the migration, so the two
    -- paths cannot drift.

    CREATE TABLE vectors (
        citation TEXT PRIMARY KEY NOT NULL REFERENCES blocks(citation),
        dim      INTEGER NOT NULL,
        data     BLOB NOT NULL                           -- f32 little-endian, L2-normalised
    );

    CREATE TABLE links (
        from_note        TEXT NOT NULL REFERENCES notes(id),
        pos              INTEGER NOT NULL,               -- order of appearance
        to_name          TEXT NOT NULL,
        resolved_note_id TEXT,                           -- NULL = dangling (valid, SPEC §3.1)
        PRIMARY KEY (from_note, to_name)
    );
    CREATE INDEX links_to_name  ON links(to_name);
    CREATE INDEX links_resolved ON links(resolved_note_id);

    -- No audit table here: the audit record lives in audit.db (see audit.rs), because
    -- this file is a cache that may be deleted and the record may not.

    INSERT INTO meta (key, value) VALUES ('generation', '0');
    "#,
    // v2: the note name becomes searchable.
    //
    // The lexical index held block text only, so a note whose distinguishing word lives in
    // its title was unfindable by that word: `inferenz-setup-05-09-2026` says "Inferenz"
    // nowhere in its body, and asking for it returned five unrelated notes. The name is
    // kebab-case and `unicode61` splits on the hyphens, so the title's words become terms
    // like any other.
    //
    // The name lands on the note's first block only: on every block a title word would
    // match every row of the note and one note would fill the whole top-k.
    //
    // FTS5 cannot add a column to an existing table, so the table is rebuilt and refilled
    // from `blocks` and `notes`. That runs inside the migration transaction on the next
    // open; no rescan of the notes tree is needed, because both sources are already here.
    r#"
    DROP TABLE blocks_fts;

    CREATE VIRTUAL TABLE blocks_fts USING fts5(
        text,
        name,
        citation UNINDEXED,
        ring UNINDEXED,
        tokenize = 'unicode61 remove_diacritics 2'
    );

    INSERT INTO blocks_fts (rowid, text, name, citation, ring)
        SELECT b.id, b.text, CASE WHEN b.idx = 0 THEN n.name END, b.citation, n.ring
        FROM blocks b JOIN notes n ON n.id = b.note_id;
    "#,
    // v3: `bereich` — which part of the organisation a note belongs to. Nullable, because
    // every note written before this migration has none and a single-project store needs none.
    r#"
    ALTER TABLE notes ADD COLUMN bereich TEXT;
    CREATE INDEX notes_bereich ON notes(bereich) WHERE bereich IS NOT NULL;
    "#,
];

/// Bring `conn` to [`SCHEMA_VERSION`]. Idempotent.
pub fn migrate(conn: &mut Connection) -> Result<()> {
    let current = current_version(conn)?;
    if current > SCHEMA_VERSION {
        return Err(Error::Index(format!(
            "database schema is version {current} but this build understands up to \
             {SCHEMA_VERSION}; upgrade cyberbrain or delete the index and rescan"
        )));
    }
    if current == SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.transaction().ix()?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        tx.execute_batch(sql)
            .map_err(|e| Error::Index(format!("migration to schema v{} failed: {e}", i + 1)))?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [(i + 1).to_string()],
        )
        .ix()?;
    }
    tx.commit().ix()
}

/// 0 for an empty database.
pub fn current_version(conn: &Connection) -> Result<u32> {
    let has_meta: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .ix()?
        > 0;
    if !has_meta {
        return Ok(0);
    }
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .ix()?;
    match v {
        None => Ok(0),
        Some(s) => s
            .parse()
            .map_err(|_| Error::Index(format!("meta.schema_version is not a number: {s:?}"))),
    }
}
