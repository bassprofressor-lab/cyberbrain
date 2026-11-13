//! Anchors in the index (schema v7): which note anchors which file, and a cache of the hash
//! each anchored file has now.
//!
//! The note head is the record; the `anchors` table is derived from it on every upsert, like
//! blocks and links, so a recall can ask for the anchors of its hits, and `find` for the notes
//! anchored to a file, without reading note files. The `anchor_files` cache keeps the last
//! hash of a file next to the mtime and size it was taken at: a recall hashes a file again
//! only when one of the two moved. Hashing itself is the caller's, which knows the project.

use crate::SqlResultExt;
use crate::index::Index;
use cyberbrain_core::{Anchor, NoteId, Result, Ring};
use rusqlite::{OptionalExtension, Transaction, params};
use std::path::Path;

/// One anchor with the note it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRow {
    pub note_id: NoteId,
    pub note_name: String,
    pub ring: Ring,
    pub anchor: Anchor,
}

/// A file's identity for the cache: modification time in nanoseconds and size in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHash {
    pub mtime_ns: i64,
    pub size: u64,
}

impl FileHash {
    /// `None` when the file cannot be stat'ed (missing, no permission).
    pub fn of(path: &Path) -> Option<FileHash> {
        let md = std::fs::metadata(path).ok()?;
        if !md.is_file() {
            return None;
        }
        let ns = md
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(FileHash {
            mtime_ns: i64::try_from(ns).ok()?,
            size: md.len(),
        })
    }
}

impl Index {
    pub(crate) fn insert_anchors(
        tx: &Transaction<'_>,
        id_s: &str,
        anchors: &[Anchor],
    ) -> Result<()> {
        if anchors.is_empty() {
            return Ok(());
        }
        let mut ins = tx
            .prepare_cached(
                "INSERT INTO anchors (note_id, path, lines, sha256, at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .ix()?;
        for a in anchors {
            ins.execute(params![id_s, a.path, a.lines, a.sha256, a.at.to_string()])
                .ix()?;
        }
        Ok(())
    }

    fn anchor_rows(&self, clause: &str, arg: &str) -> Result<Vec<AnchorRow>> {
        let sql = format!(
            "SELECT a.note_id, n.name, n.ring, a.path, a.lines, a.sha256, a.at \
             FROM anchors a JOIN notes n ON n.id = a.note_id WHERE {clause} \
             ORDER BY n.ring, n.name, a.rowid"
        );
        let mut stmt = self.conn.prepare_cached(&sql).ix()?;
        let rows = stmt
            .query_map([arg], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .ix()?;
        let mut out = Vec::new();
        for row in rows {
            let (id, name, ring, path, lines, sha256, at) = row.ix()?;
            // A row this build cannot read back is skipped, not fatal: an anchor is a hint
            // on a hit, and a recall must not fail over one.
            let (Ok(note_id), Ok(ring), Ok(at)) = (
                id.parse::<NoteId>(),
                Ring::try_from(ring as u8),
                at.parse::<jiff::Timestamp>(),
            ) else {
                continue;
            };
            out.push(AnchorRow {
                note_id,
                note_name: name,
                ring,
                anchor: Anchor {
                    path,
                    lines,
                    sha256,
                    at,
                },
            });
        }
        Ok(out)
    }

    /// The anchors of one note, in the order its head lists them.
    pub fn anchors_of(&self, id: &NoteId) -> Result<Vec<AnchorRow>> {
        self.anchor_rows("a.note_id = ?1", &id.to_string())
    }

    /// Every note anchored to this project-relative path.
    pub fn anchored_to(&self, path: &str) -> Result<Vec<AnchorRow>> {
        self.anchor_rows("a.path = ?1", path)
    }

    /// The hash last taken of `path`, if it was taken at exactly this mtime and size.
    pub fn cached_file_hash(&self, path: &Path, at: FileHash) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT sha256 FROM anchor_files WHERE path = ?1 AND mtime_ns = ?2 AND size = ?3",
                params![path.to_string_lossy(), at.mtime_ns, at.size as i64],
                |r| r.get(0),
            )
            .optional()
            .ix()
    }

    /// Remember a hash for the next recall. A cache: the caller may ignore a failure.
    pub fn remember_file_hash(&self, path: &Path, at: FileHash, sha256: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO anchor_files (path, mtime_ns, size, sha256) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET mtime_ns = excluded.mtime_ns,
                    size = excluded.size, sha256 = excluded.sha256",
                params![path.to_string_lossy(), at.mtime_ns, at.size as i64, sha256],
            )
            .ix()?;
        Ok(())
    }
}
