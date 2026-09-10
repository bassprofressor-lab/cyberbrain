//! The index handle: open, upsert, delete, lookups and the embedding-profile guard
//! (SPEC §5, §12.2).

use crate::vectors::{VectorCache, encode, normalize};
use crate::{SqlResultExt, schema};
use cyberbrain_core::{
    Block, Citation, Embedder, Error, Frontmatter, Note, NoteId, NoteKind, PiiState, Result, Ring,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Identity of the model that produced the stored vectors (SPEC §5). All three fields must
/// match before a query vector is compared against them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct EmbeddingProfile {
    /// From `Embedder::profile_id`: model, dimension and pooling together.
    pub id: String,
    pub dim: usize,
    /// Content hash of the model artefact (SPEC §6). Informational for the guard when only
    /// an `Embedder` is at hand, decisive in `check_embedding_profile`.
    pub model_hash: String,
}

impl EmbeddingProfile {
    fn describe(&self) -> String {
        let hash = if self.model_hash.len() > 12 {
            &self.model_hash[..12]
        } else {
            &self.model_hash
        };
        format!("{} (dim {}, model {})", self.id, self.dim, hash)
    }
}

/// File stamp recorded at upsert time. `scan` compares this *and* the content hash:
/// mtime alone is not sufficient (SPEC §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct NoteStamp {
    pub mtime_ns: i64,
    pub size: u64,
}

/// A note as the index knows it. The body is not stored; read it from `path`.
#[derive(Debug, Clone, Serialize)]
pub struct NoteRecord {
    pub front: Frontmatter,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    /// `content_hash()` of the note at upsert time.
    pub hash: String,
    pub stamp: Option<NoteStamp>,
    pub block_count: u32,
    pub vector_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    pub from_note: NoteId,
    pub to_name: String,
    /// `None` means dangling: valid, denotes intent, reported by `doctor` (SPEC §3.1).
    pub resolved: Option<NoteId>,
}

/// What an erasure removed, per table, so `forget` can print it (SPEC §12.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Erased {
    pub notes: usize,
    pub blocks: usize,
    pub fts_rows: usize,
    pub vectors: usize,
    pub links_out: usize,
    /// Inbound link rows that now point nowhere. They stay, because the `[[name]]` is
    /// still in the other note's file and `scan` would recreate them; they become
    /// dangling and `doctor` reports them.
    pub links_in_unresolved: usize,
}

/// What `delete_note` removed and whose it was, so the caller can log the erasure with
/// the same facts the index knew (SPEC §12.2). The index writes no audit row itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Erasure {
    pub id: NoteId,
    pub name: String,
    pub ring: Ring,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub counts: Erased,
}

/// What `set_embedding_profile` did. `changed == false` means the same profile was
/// already recorded and nothing happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProfileChange {
    pub previous: Option<EmbeddingProfile>,
    pub current: EmbeddingProfile,
    pub changed: bool,
    /// Vectors deleted because they came from `previous`. The caller should log this.
    pub vectors_wiped: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct UpsertOutcome {
    /// `true` when a note with this id already existed and was replaced.
    pub replaced: bool,
    pub blocks: usize,
    pub vectors: usize,
    pub links: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexStats {
    pub schema_version: u32,
    pub generation: i64,
    pub notes: usize,
    pub blocks: usize,
    pub fts_rows: usize,
    pub vectors: usize,
    pub links: usize,
    pub dangling_links: usize,
    pub embedding: Option<EmbeddingProfile>,
}

/// blake3 over the canonical frontmatter (JSON) and the body. Both `scan` and the index
/// compute it through this one function so they cannot disagree.
pub fn content_hash(note: &Note) -> String {
    let mut h = blake3::Hasher::new();
    let front = serde_json::to_string(&note.front).expect("frontmatter serialises");
    h.update(front.as_bytes());
    h.update(b"\n");
    h.update(note.body.as_bytes());
    h.finalize().to_hex().to_string()
}

fn enum_str<T: Serialize>(v: &T) -> Result<String> {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => Ok(s),
        other => Err(Error::Index(format!(
            "enum did not serialise to a string: {other:?}"
        ))),
    }
}

fn enum_parse<T: serde::de::DeserializeOwned>(what: &str, s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|e| Error::Index(format!("stored {what} {s:?} is not valid: {e}")))
}

fn parse_id(s: &str) -> Result<NoteId> {
    NoteId::from_string(s).map_err(|e| Error::Index(format!("stored note id {s:?}: {e}")))
}

fn stat(path: &Path) -> Option<NoteStamp> {
    let md = std::fs::metadata(path).ok()?;
    let mtime = md.modified().ok()?;
    let ns = mtime.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(NoteStamp {
        mtime_ns: i64::try_from(ns).ok()?,
        size: md.len(),
    })
}

/// One open database. Not `Sync`; wrap in a mutex to share across threads.
pub struct Index {
    pub(crate) conn: Connection,
    path: Option<PathBuf>,
    pub(crate) cache: RefCell<Option<VectorCache>>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Index").field("path", &self.path).finish()
    }
}

impl Index {
    /// Open or create the database at `path` and migrate it to the current schema.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: std::io::Error::other(e.to_string()),
        })?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))
            .ix()?;
        conn.pragma_update(None, "synchronous", "NORMAL").ix()?;
        Self::finish(conn, Some(path.to_path_buf()))
    }

    /// A private in-memory database. For tests and `--dry-run`.
    pub fn open_in_memory() -> Result<Self> {
        Self::finish(Connection::open_in_memory().ix()?, None)
    }

    fn finish(mut conn: Connection, path: Option<PathBuf>) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON").ix()?;
        conn.busy_timeout(Duration::from_secs(5)).ix()?;
        schema::migrate(&mut conn)?;
        Ok(Self {
            conn,
            path,
            cache: RefCell::new(None),
        })
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn schema_version(&self) -> Result<u32> {
        schema::current_version(&self.conn)
    }

    // ----- meta -------------------------------------------------------------------------

    pub(crate) fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ix()
    }

    fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .ix()?;
        Ok(())
    }

    /// Monotonic counter bumped by every write. The vector cache keys on it.
    pub fn generation(&self) -> Result<i64> {
        Self::read_generation(&self.conn)
    }

    pub(crate) fn read_generation(conn: &Connection) -> Result<i64> {
        let s = Self::meta_get(conn, "generation")?.unwrap_or_else(|| "0".into());
        s.parse()
            .map_err(|_| Error::Index(format!("meta.generation is not a number: {s:?}")))
    }

    fn bump_generation(conn: &Connection) -> Result<()> {
        conn.execute(
            "UPDATE meta SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT) \
             WHERE key = 'generation'",
            [],
        )
        .ix()?;
        Ok(())
    }

    // ----- embedding profile ------------------------------------------------------------

    /// The profile that produced the stored vectors, or `None` if none was ever recorded.
    pub fn embedding_profile(&self) -> Result<Option<EmbeddingProfile>> {
        Self::read_profile(&self.conn)
    }

    pub(crate) fn read_profile(conn: &Connection) -> Result<Option<EmbeddingProfile>> {
        let Some(id) = Self::meta_get(conn, "embedding_profile_id")? else {
            return Ok(None);
        };
        let dim = Self::meta_get(conn, "embedding_dim")?.unwrap_or_default();
        let dim = dim
            .parse()
            .map_err(|_| Error::Index(format!("meta.embedding_dim is not a number: {dim:?}")))?;
        let model_hash = Self::meta_get(conn, "embedding_model_hash")?.unwrap_or_default();
        Ok(Some(EmbeddingProfile {
            id,
            dim,
            model_hash,
        }))
    }

    /// Record the profile that upcoming vectors come from. Must be called before
    /// `upsert_note` is given any vectors.
    ///
    /// If a *different* profile was recorded, every stored vector is deleted in the same
    /// transaction: vectors from two models must never coexist. The returned
    /// [`ProfileChange`] carries the previous profile and the number of vectors wiped so
    /// the caller can put that on the audit record; the index does not write one.
    pub fn set_embedding_profile(&mut self, profile: &EmbeddingProfile) -> Result<ProfileChange> {
        if profile.dim == 0 {
            return Err(Error::Index("embedding profile dim must be > 0".into()));
        }
        let previous = self.embedding_profile()?;
        if previous.as_ref() == Some(profile) {
            return Ok(ProfileChange {
                previous,
                current: profile.clone(),
                changed: false,
                vectors_wiped: 0,
            });
        }
        let tx = self.conn.transaction().ix()?;
        let vectors_wiped = tx.execute("DELETE FROM vectors", []).ix()?;
        Self::meta_set(&tx, "embedding_profile_id", &profile.id)?;
        Self::meta_set(&tx, "embedding_dim", &profile.dim.to_string())?;
        Self::meta_set(&tx, "embedding_model_hash", &profile.model_hash)?;
        Self::bump_generation(&tx)?;
        tx.commit().ix()?;
        *self.cache.get_mut() = None;
        Ok(ProfileChange {
            previous,
            current: profile.clone(),
            changed: true,
            vectors_wiped,
        })
    }

    /// The guard (SPEC §5). `Err(EmbeddingProfileMismatch)` when the stored vectors were
    /// produced by anything other than `configured`. `Ok` when they match or when no
    /// profile has been recorded yet (an empty vector set can be compared with anything).
    pub fn check_embedding_profile(&self, configured: &EmbeddingProfile) -> Result<()> {
        match self.embedding_profile()? {
            Some(stored) if &stored != configured => Err(Error::EmbeddingProfileMismatch {
                stored: stored.describe(),
                configured: configured.describe(),
            }),
            _ => Ok(()),
        }
    }

    /// Same guard from an `Embedder`, which only knows its profile id and dimension.
    pub fn check_embedder(&self, embedder: &dyn Embedder) -> Result<()> {
        match self.embedding_profile()? {
            Some(stored) if stored.id != embedder.profile_id() || stored.dim != embedder.dim() => {
                Err(Error::EmbeddingProfileMismatch {
                    stored: stored.describe(),
                    configured: format!("{} (dim {})", embedder.profile_id(), embedder.dim()),
                })
            }
            _ => Ok(()),
        }
    }

    // ----- writes -----------------------------------------------------------------------

    /// Insert or replace a note with all of its blocks, FTS rows, vectors and links, in
    /// one transaction. `vectors`, when given, must hold exactly one vector per block, in
    /// block order, from the profile recorded by `set_embedding_profile`.
    pub fn upsert_note(
        &mut self,
        note: &Note,
        blocks: &[Block],
        vectors: Option<&[Vec<f32>]>,
    ) -> Result<UpsertOutcome> {
        let id = note.front.id;
        let id_s = id.to_string();
        let ring = note.front.ring;

        // Validate before touching the database.
        for (i, b) in blocks.iter().enumerate() {
            if b.note_id != id {
                return Err(Error::Index(format!(
                    "block {i} belongs to note {} but is being stored under {id_s}",
                    b.note_id
                )));
            }
            if b.citation.ring != ring {
                return Err(Error::Index(format!(
                    "block {} carries ring {} but its note is in ring {ring}",
                    b.citation, b.citation.ring
                )));
            }
            if blocks[..i].iter().any(|o| o.idx == b.idx) {
                return Err(Error::Index(format!(
                    "note {id_s} has two blocks with index {}",
                    b.idx
                )));
            }
        }
        let profile = match vectors {
            None => None,
            Some(vs) => {
                if vs.len() != blocks.len() {
                    return Err(Error::Index(format!(
                        "note {id_s}: {} vectors for {} blocks",
                        vs.len(),
                        blocks.len()
                    )));
                }
                let Some(profile) = self.embedding_profile()? else {
                    return Err(Error::Index(
                        "vectors given but no embedding profile is recorded; call \
                         set_embedding_profile first"
                            .into(),
                    ));
                };
                if let Some(bad) = vs.iter().find(|v| v.len() != profile.dim) {
                    return Err(Error::Index(format!(
                        "note {id_s}: vector has dim {} but profile {} has dim {}",
                        bad.len(),
                        profile.id,
                        profile.dim
                    )));
                }
                Some(profile)
            }
        };

        let tx = self.conn.transaction().ix()?;

        // Name uniqueness with a message better than a constraint error.
        if let Some(other) = tx
            .query_row(
                "SELECT id FROM notes WHERE name = ?1 AND id != ?2",
                params![note.front.name, id_s],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ix()?
        {
            return Err(Error::Index(format!(
                "note name {:?} is already used by note {other}",
                note.front.name
            )));
        }

        let existing_name: Option<String> = tx
            .query_row("SELECT name FROM notes WHERE id = ?1", [&id_s], |r| {
                r.get(0)
            })
            .optional()
            .ix()?;
        let replaced = existing_name.is_some();
        if replaced {
            Self::remove_derived(&tx, &id_s)?;
            if existing_name.as_deref() != Some(note.front.name.as_str()) {
                // Renamed: inbound links to the old name no longer resolve to this note.
                tx.execute(
                    "UPDATE links SET resolved_note_id = NULL WHERE resolved_note_id = ?1",
                    [&id_s],
                )
                .ix()?;
            }
        }

        let stamp = stat(&note.path);
        let tags = serde_json::to_string(&note.front.tags)
            .map_err(|e| Error::Index(format!("tags: {e}")))?;
        tx.execute(
            "INSERT INTO notes (id, name, ring, kind, path, created, updated, mtime_ns, size,
                                hash, tags, bereich, retention, pii)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name, ring = excluded.ring, kind = excluded.kind,
                path = excluded.path, created = excluded.created, updated = excluded.updated,
                mtime_ns = excluded.mtime_ns, size = excluded.size, hash = excluded.hash,
                tags = excluded.tags, bereich = excluded.bereich,
                retention = excluded.retention, pii = excluded.pii",
            params![
                id_s,
                note.front.name,
                ring.as_u8() as i64,
                enum_str(&note.front.kind)?,
                note.path.to_string_lossy().into_owned(),
                note.front.created.to_string(),
                note.front.updated.to_string(),
                stamp.map(|s| s.mtime_ns),
                stamp.map(|s| s.size as i64),
                content_hash(note),
                tags,
                note.front.bereich,
                note.front.retention,
                enum_str(&note.front.pii)?,
            ],
        )
        .ix()?;

        {
            let mut ins_block = tx
                .prepare_cached(
                    "INSERT INTO blocks (citation, note_id, idx, text, token_count)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .ix()?;
            let mut ins_fts = tx
                .prepare_cached(
                    "INSERT INTO blocks_fts (rowid, text, name, citation, ring) VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .ix()?;
            let mut ins_vec = tx
                .prepare_cached("INSERT INTO vectors (citation, dim, data) VALUES (?1, ?2, ?3)")
                .ix()?;
            for (i, b) in blocks.iter().enumerate() {
                let cit = b.citation.to_string();
                ins_block
                    .execute(params![
                        cit,
                        id_s,
                        b.idx as i64,
                        b.text,
                        b.token_count as i64
                    ])
                    .map_err(|e| {
                        Error::Index(format!("storing block {cit} of note {id_s}: {e}"))
                    })?;
                let rowid = tx.last_insert_rowid();
                // The name goes on the first block only. On every row it would be
                // correct and useless: a title word would then match all 37 rows of a
                // 37-block note and the whole note would swamp the top-k it just won.
                // One row per note means a title match brings the note in once, at its
                // opening block, which is where a reader who searched the title lands.
                let name = if b.idx == 0 {
                    Some(note.front.name.as_str())
                } else {
                    None
                };
                ins_fts
                    .execute(params![rowid, b.text, name, cit, ring.as_u8() as i64])
                    .ix()?;
                if let (Some(vs), Some(p)) = (vectors, &profile) {
                    let mut v = vs[i].clone();
                    if !normalize(&mut v) {
                        // A zero vector matches nothing; store it as such rather than NaN.
                        v.iter_mut().for_each(|x| *x = 0.0);
                    }
                    ins_vec
                        .execute(params![cit, p.dim as i64, encode(&v)])
                        .ix()?;
                }
            }
        }

        let mut links = 0usize;
        {
            let mut ins_link = tx
                .prepare_cached(
                    "INSERT INTO links (from_note, pos, to_name, resolved_note_id)
                     VALUES (?1, ?2, ?3, (SELECT id FROM notes WHERE name = ?3))",
                )
                .ix()?;
            let mut seen: Vec<&str> = Vec::new();
            for name in &note.front.links {
                let name = name.trim();
                if name.is_empty() || seen.contains(&name) {
                    continue;
                }
                seen.push(name);
                ins_link.execute(params![id_s, links as i64, name]).ix()?;
                links += 1;
            }
        }
        // Anyone who linked to this name now resolves to this note.
        tx.execute(
            "UPDATE links SET resolved_note_id = ?1 WHERE to_name = ?2",
            params![id_s, note.front.name],
        )
        .ix()?;

        Self::bump_generation(&tx)?;
        tx.commit().ix()?;
        *self.cache.get_mut() = None;
        Ok(UpsertOutcome {
            replaced,
            blocks: blocks.len(),
            vectors: if vectors.is_some() { blocks.len() } else { 0 },
            links,
        })
    }

    /// Delete blocks, FTS rows, vectors and outbound links of a note. The `notes` row stays.
    fn remove_derived(tx: &Transaction<'_>, id_s: &str) -> Result<Erased> {
        let vectors = tx
            .execute(
                "DELETE FROM vectors WHERE citation IN (SELECT citation FROM blocks WHERE note_id = ?1)",
                [id_s],
            )
            .ix()?;
        // FTS5 does not report changes reliably; count before and after.
        let fts_before: i64 = tx
            .query_row(
                "SELECT count(*) FROM blocks_fts WHERE rowid IN (SELECT id FROM blocks WHERE note_id = ?1)",
                [id_s],
                |r| r.get(0),
            )
            .ix()?;
        tx.execute(
            "DELETE FROM blocks_fts WHERE rowid IN (SELECT id FROM blocks WHERE note_id = ?1)",
            [id_s],
        )
        .ix()?;
        let fts_after: i64 = tx
            .query_row(
                "SELECT count(*) FROM blocks_fts WHERE rowid IN (SELECT id FROM blocks WHERE note_id = ?1)",
                [id_s],
                |r| r.get(0),
            )
            .ix()?;
        if fts_after != 0 {
            return Err(Error::Index(format!(
                "{fts_after} FTS rows of note {id_s} survived deletion"
            )));
        }
        let blocks = tx
            .execute("DELETE FROM blocks WHERE note_id = ?1", [id_s])
            .ix()?;
        let links_out = tx
            .execute("DELETE FROM links WHERE from_note = ?1", [id_s])
            .ix()?;
        Ok(Erased {
            vectors,
            fts_rows: (fts_before - fts_after) as usize,
            blocks,
            links_out,
            ..Erased::default()
        })
    }

    /// Remove every trace of a note from the index, in one transaction (SPEC §12.2). The
    /// file on disk is the caller's business, and so is the audit row: the returned
    /// [`Erasure`] names the note and counts what went, the caller logs it.
    pub fn delete_note(&mut self, id: &NoteId) -> Result<Erasure> {
        let id_s = id.to_string();
        let tx = self.conn.transaction().ix()?;
        let (name, ring, path): (String, i64, String) = tx
            .query_row(
                "SELECT name, ring, path FROM notes WHERE id = ?1",
                [&id_s],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .ix()?
            .ok_or_else(|| Error::NoSuchNote(id_s.clone()))?;
        let mut erased = Self::remove_derived(&tx, &id_s)?;
        erased.links_in_unresolved = tx
            .execute(
                "UPDATE links SET resolved_note_id = NULL WHERE resolved_note_id = ?1",
                [&id_s],
            )
            .ix()?;
        erased.notes = tx
            .execute("DELETE FROM notes WHERE id = ?1", [&id_s])
            .ix()?;
        Self::bump_generation(&tx)?;
        tx.commit().ix()?;
        *self.cache.get_mut() = None;
        Ok(Erasure {
            id: *id,
            name,
            ring: Ring::try_from(ring as u8)?,
            path: PathBuf::from(path),
            counts: erased,
        })
    }

    /// Drop every note, block, FTS row, vector and link. Keeps `meta`, including the
    /// embedding profile. Cheaper than deleting the file for `scan --full`; either is
    /// safe now that the audit record lives elsewhere. Not audited here.
    pub fn clear(&mut self) -> Result<Erased> {
        let tx = self.conn.transaction().ix()?;
        let vectors = tx.execute("DELETE FROM vectors", []).ix()?;
        let fts_rows = tx
            .query_row("SELECT count(*) FROM blocks_fts", [], |r| {
                r.get::<_, i64>(0)
            })
            .ix()? as usize;
        tx.execute("DELETE FROM blocks_fts", []).ix()?;
        let links_out = tx.execute("DELETE FROM links", []).ix()?;
        let blocks = tx.execute("DELETE FROM blocks", []).ix()?;
        let notes = tx.execute("DELETE FROM notes", []).ix()?;
        let e = Erased {
            notes,
            blocks,
            fts_rows,
            vectors,
            links_out,
            links_in_unresolved: 0,
        };
        Self::bump_generation(&tx)?;
        tx.commit().ix()?;
        *self.cache.get_mut() = None;
        Ok(e)
    }

    // ----- reads ------------------------------------------------------------------------

    const NOTE_COLUMNS: &'static str = "n.id, n.name, n.ring, n.kind, n.path, n.created, n.updated, n.mtime_ns, n.size, \
         n.hash, n.tags, n.bereich, n.retention, n.pii, \
         (SELECT count(*) FROM blocks b WHERE b.note_id = n.id), \
         (SELECT count(*) FROM vectors v JOIN blocks b ON b.citation = v.citation WHERE b.note_id = n.id), \
         (SELECT json_group_array(to_name) FROM (SELECT to_name FROM links l WHERE l.from_note = n.id ORDER BY pos))";

    fn notes_where(&self, clause: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<NoteRecord>> {
        let sql = format!(
            "SELECT {} FROM notes n WHERE {clause} ORDER BY n.ring, n.name",
            Self::NOTE_COLUMNS
        );
        let mut stmt = self.conn.prepare_cached(&sql).ix()?;
        let rows = stmt
            .query_map(args, |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                    r.get::<_, Option<i64>>(8)?,
                    r.get::<_, String>(9)?,
                    r.get::<_, String>(10)?,
                    r.get::<_, Option<String>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                    r.get::<_, String>(13)?,
                    r.get::<_, i64>(14)?,
                    r.get::<_, i64>(15)?,
                    r.get::<_, String>(16)?,
                ))
            })
            .ix()?;
        let mut out = Vec::new();
        for row in rows {
            let (
                id,
                name,
                ring,
                kind,
                path,
                created,
                updated,
                mtime_ns,
                size,
                hash,
                tags,
                bereich,
                retention,
                pii,
                block_count,
                vector_count,
                links,
            ) = row.ix()?;
            let ts = |what: &str, s: &str| -> Result<jiff::Timestamp> {
                s.parse()
                    .map_err(|e| Error::Index(format!("stored {what} {s:?} of note {id}: {e}")))
            };
            out.push(NoteRecord {
                front: Frontmatter {
                    id: parse_id(&id)?,
                    name,
                    ring: Ring::try_from(ring as u8)?,
                    kind: enum_parse::<NoteKind>("kind", &kind)?,
                    created: ts("created", &created)?,
                    updated: ts("updated", &updated)?,
                    tags: serde_json::from_str(&tags)
                        .map_err(|e| Error::Index(format!("stored tags of note {id}: {e}")))?,
                    links: serde_json::from_str(&links)
                        .map_err(|e| Error::Index(format!("stored links of note {id}: {e}")))?,
                    bereich,
                    retention,
                    pii: enum_parse::<PiiState>("pii", &pii)?,
                },
                path: PathBuf::from(path),
                hash,
                stamp: match (mtime_ns, size) {
                    (Some(m), Some(s)) => Some(NoteStamp {
                        mtime_ns: m,
                        size: s as u64,
                    }),
                    _ => None,
                },
                block_count: block_count as u32,
                vector_count: vector_count as u32,
            });
        }
        Ok(out)
    }

    pub fn note(&self, id: &NoteId) -> Result<Option<NoteRecord>> {
        Ok(self
            .notes_where("n.id = ?1", &[&id.to_string()])?
            .into_iter()
            .next())
    }

    pub fn note_by_name(&self, name: &str) -> Result<Option<NoteRecord>> {
        Ok(self
            .notes_where("n.name = ?1", &[&name])?
            .into_iter()
            .next())
    }

    /// Every note, ordered by ring then name.
    pub fn notes(&self) -> Result<Vec<NoteRecord>> {
        self.notes_where("1 = 1", &[])
    }

    /// Notes in one ring.
    pub fn notes_in_ring(&self, ring: Ring) -> Result<Vec<NoteRecord>> {
        self.notes_where("n.ring = ?1", &[&(ring.as_u8() as i64)])
    }

    /// The cheap half of incremental scan: content hash and file stamp as recorded at the
    /// last upsert, or `None` if the note is unknown.
    pub fn fingerprint(&self, id: &NoteId) -> Result<Option<(String, Option<NoteStamp>)>> {
        self.conn
            .query_row(
                "SELECT hash, mtime_ns, size FROM notes WHERE id = ?1",
                [id.to_string()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .optional()
            .ix()
            .map(|o| {
                o.map(|(hash, m, s)| {
                    let stamp = match (m, s) {
                        (Some(m), Some(s)) => Some(NoteStamp {
                            mtime_ns: m,
                            size: s as u64,
                        }),
                        _ => None,
                    };
                    (hash, stamp)
                })
            })
    }

    /// Blocks of one note in index order.
    pub fn blocks_of(&self, id: &NoteId) -> Result<Vec<Block>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT citation, note_id, idx, text, token_count FROM blocks \
                 WHERE note_id = ?1 ORDER BY idx",
            )
            .ix()?;
        let rows = stmt
            .query_map([id.to_string()], Self::row_to_block_raw)
            .ix()?;
        rows.map(|r| Self::block_from_raw(r.ix()?)).collect()
    }

    fn row_to_block_raw(
        r: &rusqlite::Row<'_>,
    ) -> rusqlite::Result<(String, String, i64, String, i64)> {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    }

    fn block_from_raw(
        (cit, note_id, idx, text, tokens): (String, String, i64, String, i64),
    ) -> Result<Block> {
        Ok(Block {
            citation: cit.parse()?,
            note_id: parse_id(&note_id)?,
            idx: idx as u32,
            text,
            token_count: tokens as u32,
        })
    }

    /// Expand a citation to its block and note (`recall --id`).
    pub fn resolve(&self, citation: &Citation) -> Result<Option<(Block, NoteRecord)>> {
        let raw = self
            .conn
            .query_row(
                "SELECT citation, note_id, idx, text, token_count FROM blocks WHERE citation = ?1",
                [citation.to_string()],
                Self::row_to_block_raw,
            )
            .optional()
            .ix()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let block = Self::block_from_raw(raw)?;
        let note = self.note(&block.note_id)?.ok_or_else(|| {
            Error::Index(format!(
                "block {citation} references note {} which does not exist",
                block.note_id
            ))
        })?;
        Ok(Some((block, note)))
    }

    /// Blocks whose text contains `needle`, case-insensitively, as (citation, note name,
    /// text). For `policy subject` (SPEC §12.3). Not ranked; every match is returned.
    pub fn blocks_containing(&self, needle: &str) -> Result<Vec<(Citation, String, String)>> {
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT b.citation, n.name, b.text FROM blocks b JOIN notes n ON n.id = b.note_id \
                 WHERE instr(lower(b.text), lower(?1)) > 0 ORDER BY n.name, b.idx",
            )
            .ix()?;
        let rows = stmt
            .query_map([needle], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .ix()?;
        let mut out = Vec::new();
        for r in rows {
            let (c, n, t) = r.ix()?;
            out.push((c.parse()?, n, t));
        }
        Ok(out)
    }

    fn links_where(&self, clause: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Link>> {
        let sql = format!(
            "SELECT from_note, to_name, resolved_note_id FROM links WHERE {clause} \
             ORDER BY from_note, pos"
        );
        let mut stmt = self.conn.prepare_cached(&sql).ix()?;
        let rows = stmt
            .query_map(args, |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .ix()?;
        let mut out = Vec::new();
        for r in rows {
            let (from, to, resolved) = r.ix()?;
            out.push(Link {
                from_note: parse_id(&from)?,
                to_name: to,
                resolved: resolved.as_deref().map(parse_id).transpose()?,
            });
        }
        Ok(out)
    }

    /// Outbound links of a note.
    pub fn links_from(&self, id: &NoteId) -> Result<Vec<Link>> {
        self.links_where("from_note = ?1", &[&id.to_string()])
    }

    /// Inbound links: rows that resolve to this note.
    pub fn links_to(&self, id: &NoteId) -> Result<Vec<Link>> {
        self.links_where("resolved_note_id = ?1", &[&id.to_string()])
    }

    /// Links to names that do not exist. Valid (SPEC §3.1); `doctor` reports them.
    pub fn dangling_links(&self) -> Result<Vec<Link>> {
        self.links_where("resolved_note_id IS NULL", &[])
    }

    /// Every link row.
    pub fn links(&self) -> Result<Vec<Link>> {
        self.links_where("1 = 1", &[])
    }

    fn count(&self, sql: &str) -> Result<usize> {
        self.conn
            .query_row(sql, [], |r| r.get::<_, i64>(0))
            .ix()
            .map(|n| n as usize)
    }

    pub fn stats(&self) -> Result<IndexStats> {
        Ok(IndexStats {
            schema_version: self.schema_version()?,
            generation: self.generation()?,
            notes: self.count("SELECT count(*) FROM notes")?,
            blocks: self.count("SELECT count(*) FROM blocks")?,
            fts_rows: self.count("SELECT count(*) FROM blocks_fts")?,
            vectors: self.count("SELECT count(*) FROM vectors")?,
            links: self.count("SELECT count(*) FROM links")?,
            dangling_links: self
                .count("SELECT count(*) FROM links WHERE resolved_note_id IS NULL")?,
            embedding: self.embedding_profile()?,
        })
    }

    /// Cross-table consistency problems, one line each. Empty means clean. For `doctor`.
    pub fn integrity(&self) -> Result<Vec<String>> {
        let mut problems = Vec::new();
        let checks: &[(&str, &str)] = &[
            (
                "vectors without a block",
                "SELECT count(*) FROM vectors WHERE citation NOT IN (SELECT citation FROM blocks)",
            ),
            (
                "FTS rows without a block",
                "SELECT count(*) FROM blocks_fts WHERE rowid NOT IN (SELECT id FROM blocks)",
            ),
            (
                "blocks without an FTS row",
                "SELECT count(*) FROM blocks WHERE id NOT IN (SELECT rowid FROM blocks_fts)",
            ),
            (
                "blocks without a note",
                "SELECT count(*) FROM blocks WHERE note_id NOT IN (SELECT id FROM notes)",
            ),
            (
                "links from a missing note",
                "SELECT count(*) FROM links WHERE from_note NOT IN (SELECT id FROM notes)",
            ),
            (
                "links resolved to a missing note",
                "SELECT count(*) FROM links WHERE resolved_note_id IS NOT NULL \
                 AND resolved_note_id NOT IN (SELECT id FROM notes)",
            ),
            (
                "FTS rows whose citation disagrees with the block",
                "SELECT count(*) FROM blocks_fts f JOIN blocks b ON b.id = f.rowid \
                 WHERE f.citation != b.citation",
            ),
        ];
        for (what, sql) in checks {
            let n = self.count(sql)?;
            if n > 0 {
                problems.push(format!("{n} {what}"));
            }
        }
        // A cache written before the audit record moved to audit.db still carries the
        // old table. Its rows are not part of the record; say so before anyone deletes
        // the file believing it holds nothing of value.
        let legacy = self
            .count("SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'audit'")?;
        if legacy > 0 {
            let rows = self.count("SELECT count(*) FROM audit")?;
            problems.push(format!(
                "cache holds a legacy audit table with {rows} rows; the audit record now \
                 lives in audit.db, export these before discarding the cache"
            ));
        }
        if let Some(p) = self.embedding_profile()? {
            let n = self.count(&format!(
                "SELECT count(*) FROM vectors WHERE dim != {}",
                p.dim
            ))?;
            if n > 0 {
                problems.push(format!(
                    "{n} vectors with a dimension other than the profile's {}",
                    p.dim
                ));
            }
            let missing = self.count(
                "SELECT count(*) FROM blocks WHERE citation NOT IN (SELECT citation FROM vectors)",
            )?;
            if missing > 0 {
                problems.push(format!(
                    "{missing} blocks without a vector (semantic search will not see them)"
                ));
            }
        } else if self.count("SELECT count(*) FROM vectors")? > 0 {
            problems.push("vectors stored but no embedding profile recorded".into());
        }
        Ok(problems)
    }

    /// Reclaim space after large deletions. Not part of any hot path.
    pub fn vacuum(&self) -> Result<()> {
        self.conn.execute_batch("VACUUM").ix()
    }
}
