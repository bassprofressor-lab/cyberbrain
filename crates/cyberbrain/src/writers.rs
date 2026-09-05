//! The writers `--dry-run` swaps (SPEC §8, §8.2). The real scan, the real policy checks and
//! the real erasure logic run either way; these are the only things that differ.
//!
//! Every no-op writer returns the outcome the real one *would* have returned, computed from
//! reads, so a dry-run report carries real numbers. It never touches disk or the database.

use cyberbrain_core::{Block, Error, Note, NoteId, Result, Store};
use cyberbrain_index::{EmbeddingProfile, Erased, Erasure, Index, ProfileChange, UpsertOutcome};
use cyberbrain_policy::{EraseRequest, Eraser, ErasureReport};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Notes on disk.
pub trait NoteWriter: Send + Sync {
    /// Write the file and return its path.
    fn write(&self, note: &Note) -> Result<PathBuf>;
    /// Remove the file. `Ok(true)` when a file was (or would have been) removed, `Ok(false)`
    /// when there was nothing there.
    fn remove(&self, path: &Path) -> Result<bool>;
}

pub struct FsNoteWriter {
    pub store: Store,
}

impl NoteWriter for FsNoteWriter {
    fn write(&self, note: &Note) -> Result<PathBuf> {
        self.store.write(note)
    }

    fn remove(&self, path: &Path) -> Result<bool> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(Error::Io {
                path: path.to_path_buf(),
                source: e,
            }),
        }
    }
}

/// Validates exactly what the real writer validates before it writes, then writes nothing.
///
/// "Exactly" is load-bearing and was not true: this used to compute a path and return it,
/// skipping the cross-ring name check, the resident cap and the frontmatter rendering. A
/// dry run therefore reported success for a write the real run would refuse, which is the
/// worst thing a preview can do — it does not merely fail to warn, it reassures.
/// [`Store::validate_write`] is now the one implementation both callers use.
pub struct NoopNoteWriter {
    pub store: Store,
}

impl NoteWriter for NoopNoteWriter {
    fn write(&self, note: &Note) -> Result<PathBuf> {
        self.store.validate_write(note).map(|(path, _)| path)
    }

    fn remove(&self, path: &Path) -> Result<bool> {
        Ok(path.is_file())
    }
}

/// The index.
pub trait IndexWriter: Send + Sync {
    fn set_embedding_profile(&self, profile: &EmbeddingProfile) -> Result<ProfileChange>;
    fn upsert_note(
        &self,
        note: &Note,
        blocks: &[Block],
        vectors: Option<&[Vec<f32>]>,
    ) -> Result<UpsertOutcome>;
    fn delete_note(&self, id: &NoteId) -> Result<Erasure>;
    fn clear(&self) -> Result<Erased>;
}

pub fn lock_index(index: &Mutex<Index>) -> Result<MutexGuard<'_, Index>> {
    index
        .lock()
        .map_err(|_| Error::Index("index lock poisoned; refusing to continue".into()))
}

pub struct SqliteIndexWriter {
    pub index: Arc<Mutex<Index>>,
}

impl IndexWriter for SqliteIndexWriter {
    fn set_embedding_profile(&self, profile: &EmbeddingProfile) -> Result<ProfileChange> {
        lock_index(&self.index)?.set_embedding_profile(profile)
    }

    fn upsert_note(
        &self,
        note: &Note,
        blocks: &[Block],
        vectors: Option<&[Vec<f32>]>,
    ) -> Result<UpsertOutcome> {
        lock_index(&self.index)?.upsert_note(note, blocks, vectors)
    }

    fn delete_note(&self, id: &NoteId) -> Result<Erasure> {
        lock_index(&self.index)?.delete_note(id)
    }

    fn clear(&self) -> Result<Erased> {
        lock_index(&self.index)?.clear()
    }
}

/// Reads the index to say what the real writer would have done, and does nothing.
pub struct NoopIndexWriter {
    pub index: Arc<Mutex<Index>>,
}

impl IndexWriter for NoopIndexWriter {
    fn set_embedding_profile(&self, profile: &EmbeddingProfile) -> Result<ProfileChange> {
        let ix = lock_index(&self.index)?;
        let previous = ix.embedding_profile()?;
        let changed = previous.as_ref() != Some(profile);
        Ok(ProfileChange {
            vectors_wiped: if changed { ix.stats()?.vectors } else { 0 },
            previous,
            current: profile.clone(),
            changed,
        })
    }

    fn upsert_note(
        &self,
        note: &Note,
        blocks: &[Block],
        vectors: Option<&[Vec<f32>]>,
    ) -> Result<UpsertOutcome> {
        let ix = lock_index(&self.index)?;
        let mut links: Vec<&str> = Vec::new();
        for l in &note.front.links {
            let l = l.trim();
            if !l.is_empty() && !links.contains(&l) {
                links.push(l);
            }
        }
        Ok(UpsertOutcome {
            replaced: ix.note(&note.front.id)?.is_some(),
            blocks: blocks.len(),
            vectors: if vectors.is_some() { blocks.len() } else { 0 },
            links: links.len(),
        })
    }

    fn delete_note(&self, id: &NoteId) -> Result<Erasure> {
        let ix = lock_index(&self.index)?;
        let rec = ix
            .note(id)?
            .ok_or_else(|| Error::NoSuchNote(id.to_string()))?;
        Ok(Erasure {
            id: *id,
            name: rec.front.name,
            ring: rec.front.ring,
            path: rec.path,
            counts: Erased {
                notes: 1,
                blocks: rec.block_count as usize,
                // One FTS row per block; `integrity()` is what checks that holds.
                fts_rows: rec.block_count as usize,
                vectors: rec.vector_count as usize,
                links_out: ix.links_from(id)?.len(),
                links_in_unresolved: ix.links_to(id)?.len(),
            },
        })
    }

    fn clear(&self) -> Result<Erased> {
        let s = lock_index(&self.index)?.stats()?;
        Ok(Erased {
            notes: s.notes,
            blocks: s.blocks,
            fts_rows: s.fts_rows,
            vectors: s.vectors,
            links_out: s.links,
            links_in_unresolved: 0,
        })
    }
}

/// The storage side of erasure (SPEC §12.2): the file, then every index row, and the
/// counts of each. This is the *one* erasure path; `forget`, the retention sweep and the
/// API's `DELETE` all arrive here through `Policy::forget` / `Policy::apply_retention`.
pub struct StoreEraser<'a> {
    pub notes: &'a dyn NoteWriter,
    pub index: &'a dyn IndexWriter,
}

impl Eraser for StoreEraser<'_> {
    fn erase(&mut self, req: &EraseRequest) -> Result<ErasureReport> {
        let mut report = ErasureReport {
            file_removed: self.notes.remove(&req.path)?,
            ..ErasureReport::default()
        };
        match self.index.delete_note(&req.note_id) {
            Ok(er) => {
                report.blocks = er.counts.blocks;
                report.fts_rows = er.counts.fts_rows;
                report.vectors = er.counts.vectors;
                report.links_out = er.counts.links_out;
                report.links_in_unresolved = er.counts.links_in_unresolved;
            }
            Err(Error::NoSuchNote(_)) => report.notes.push(
                "the note was not in the index, so there were no blocks, vectors or links to \
                 remove there; run `cyberbrain doctor` if that is unexpected"
                    .into(),
            ),
            Err(e) => return Err(e),
        }
        Ok(report)
    }
}
