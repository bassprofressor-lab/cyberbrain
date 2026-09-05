//! The notes tree on disk (SPEC §4): `<store>/notes/r{0..4}/<name>.md`.
//!
//! The file is authoritative; everything else is a cache. This module is the only place
//! that touches the tree, and it holds three promises:
//!
//! - **Writes are atomic.** A note is written to a temporary file in the same directory,
//!   flushed, then renamed over the target. A reader sees the old file or the new file,
//!   never a partial one. On failure the target is untouched and the temp file is gone.
//! - **The resident cap is enforced here.** Rings 0 and 1 together may not exceed the
//!   configured token budget; a write that would cross it fails with
//!   `Error::RingCapExceeded` (SPEC §3.2). Tokens are [`approx_tokens`] of the body.
//! - **Change detection uses mtime *and* content hash.** Bind mounts and archive
//!   extraction produce identical mtimes for changed content; an index that trusts mtime
//!   alone serves stale blocks with no error. [`Fingerprint::compare`] says which side
//!   changed so the caller can count what it actually did.
//!
//! Listing honours `.cyberbrainignore` (gitignore syntax) at the store root and in any
//! directory under `notes/`. Git's own ignore files are deliberately *not* honoured: the
//! store is normally listed in the project's `.gitignore`, and honouring that would hide
//! every note.

use crate::blocks::approx_tokens;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::frontmatter;
use crate::types::{Note, NoteId, Ring};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const NOTES_DIR: &str = "notes";
pub const DB_FILE: &str = "cyberbrain.db";
pub const MODELS_DIR: &str = "models";
pub const IGNORE_FILE: &str = ".cyberbrainignore";
const NOTE_EXT: &str = "md";

/// Handle on a store directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
    resident_cap: usize,
}

/// A note file found by [`Store::list`], identified by its place in the tree. The
/// frontmatter has not been read yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub ring: Ring,
    /// File stem. [`Store::read`] checks it against the frontmatter `name`.
    pub name: String,
}

/// Something under `notes/` that was not listed, and why. Never silent (SPEC §14.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    /// Sorted by path, so two listings of the same tree are identical.
    pub entries: Vec<Entry>,
    pub skipped: Vec<Skipped>,
}

impl Store {
    /// Open an existing store. Loads `cyberbrain.toml` for the resident cap.
    pub fn open(root: impl Into<PathBuf>) -> Result<Store> {
        let root = root.into();
        let cfg = Config::load(&root)?;
        Self::with_config(root, &cfg)
    }

    /// Open an existing store with an already-loaded config.
    pub fn with_config(root: impl Into<PathBuf>, cfg: &Config) -> Result<Store> {
        Self::with_cap(root, cfg.rings.resident_cap_tokens)
    }

    /// Open an existing store with an explicit resident cap.
    pub fn with_cap(root: impl Into<PathBuf>, resident_cap: usize) -> Result<Store> {
        let root = root.into();
        let notes = root.join(NOTES_DIR);
        if !notes.is_dir() {
            return Err(Error::Io {
                path: root,
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "no notes/ directory here; not a cyberbrain store (run `cyberbrain init`)",
                ),
            });
        }
        Ok(Store { root, resident_cap })
    }

    /// Create the directory layout of SPEC §4 (`notes/r0..r4`, `models/`) and open it.
    /// Existing directories are kept. Does not write a config; that is `init`'s job via
    /// [`Config::write_default`].
    pub fn create(root: impl Into<PathBuf>, resident_cap: usize) -> Result<Store> {
        let root = root.into();
        for dir in Ring::ALL
            .iter()
            .map(|r| root.join(NOTES_DIR).join(r.dir()))
            .chain([root.join(MODELS_DIR)])
        {
            fs::create_dir_all(&dir).map_err(|e| Error::Io {
                path: dir.clone(),
                source: e,
            })?;
        }
        Ok(Store { root, resident_cap })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn resident_cap(&self) -> usize {
        self.resident_cap
    }
    pub fn notes_dir(&self) -> PathBuf {
        self.root.join(NOTES_DIR)
    }
    pub fn ring_dir(&self, ring: Ring) -> PathBuf {
        self.notes_dir().join(ring.dir())
    }
    pub fn db_path(&self) -> PathBuf {
        self.root.join(DB_FILE)
    }
    pub fn config_path(&self) -> PathBuf {
        Config::path_in(&self.root)
    }
    pub fn models_dir(&self) -> PathBuf {
        self.root.join(MODELS_DIR)
    }

    /// Where a note with this ring and name lives. Validates the name so that a name can
    /// never become a path outside its ring directory.
    pub fn note_path(&self, ring: Ring, name: &str) -> Result<PathBuf> {
        frontmatter::validate_name(name).map_err(|why| Error::Frontmatter {
            path: self.ring_dir(ring).join(format!("{name}.{NOTE_EXT}")),
            reason: format!("name `{name}`: {why}"),
        })?;
        Ok(self.ring_dir(ring).join(format!("{name}.{NOTE_EXT}")))
    }

    /// Resolve a name to its file by looking in every ring directory. A name present in
    /// two rings is an integrity error and is reported, not resolved by luck.
    pub fn resolve(&self, name: &str) -> Result<PathBuf> {
        let found: Vec<PathBuf> = Ring::ALL
            .iter()
            .filter_map(|r| self.note_path(*r, name).ok())
            .filter(|p| p.is_file())
            .collect();
        match found.as_slice() {
            [] => Err(Error::NoSuchNote(name.to_string())),
            [one] => Ok(one.clone()),
            [first, rest @ ..] => Err(Error::Frontmatter {
                path: first.clone(),
                reason: format!(
                    "name `{name}` also exists at {}; a name is unique within the store",
                    rest.iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            }),
        }
    }

    /// Walk the notes tree. Honours `.cyberbrainignore`; skips hidden files (which is
    /// also where in-flight temp files live); reports everything it declined to list.
    pub fn list(&self) -> Result<Listing> {
        let notes = self.notes_dir();
        let mut builder = ignore::WalkBuilder::new(&notes);
        builder
            .standard_filters(false)
            .hidden(true)
            .parents(false)
            .follow_links(false)
            .max_depth(Some(2))
            .sort_by_file_path(|a, b| a.cmp(b));
        builder.add_custom_ignore_filename(IGNORE_FILE);
        let root_ignore = self.root.join(IGNORE_FILE);
        if root_ignore.is_file()
            && let Some(err) = builder.add_ignore(&root_ignore)
        {
            return Err(Error::Config(format!("{}: {err}", root_ignore.display())));
        }

        let mut listing = Listing::default();
        for entry in builder.build() {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    return Err(Error::Io {
                        path: notes.clone(),
                        source: io::Error::other(e.to_string()),
                    });
                }
            };
            let path = entry.path().to_path_buf();
            let depth = entry.depth();
            if depth == 0 {
                continue;
            }
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            if depth == 1 {
                let dir_name = entry.file_name().to_string_lossy().into_owned();
                if !is_dir {
                    listing.skipped.push(Skipped {
                        path,
                        reason: "a file directly under notes/; notes live in r0..r4".into(),
                    });
                } else if !Ring::ALL.iter().any(|r| r.dir() == dir_name) {
                    listing.skipped.push(Skipped {
                        path,
                        reason: format!("directory `{dir_name}` is not a ring (r0..r4)"),
                    });
                }
                continue;
            }
            // depth == 2: inside a ring directory.
            let ring_name = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Some(ring) = Ring::ALL.iter().copied().find(|r| r.dir() == ring_name) else {
                // Under a non-ring directory; already reported at depth 1.
                continue;
            };
            if is_dir {
                listing.skipped.push(Skipped {
                    path,
                    reason: "a directory inside a ring; rings are flat".into(),
                });
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some(NOTE_EXT) {
                listing.skipped.push(Skipped {
                    path,
                    reason: "not a .md file".into(),
                });
                continue;
            }
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            if let Err(why) = frontmatter::validate_name(&name) {
                listing.skipped.push(Skipped {
                    path,
                    reason: format!("file name `{name}` is not a note name: {why}"),
                });
                continue;
            }
            listing.entries.push(Entry { path, ring, name });
        }
        Ok(listing)
    }

    /// Read and parse the note at `path`. The path must be inside a ring directory; the
    /// frontmatter's `name` and `ring` must agree with where the file is.
    pub fn read_path(&self, path: &Path) -> Result<Note> {
        let text = fs::read_to_string(path).map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: e,
        })?;
        let parsed = frontmatter::parse(path, &text)?;
        let expected = self.note_path(parsed.front.ring, &parsed.front.name)?;
        if !same_file_name(&expected, path) {
            return Err(Error::Frontmatter {
                path: path.to_path_buf(),
                reason: format!(
                    "frontmatter says name `{}` in ring {}, which belongs at {}",
                    parsed.front.name,
                    parsed.front.ring,
                    expected.display()
                ),
            });
        }
        Ok(Note {
            front: parsed.front,
            body: parsed.body.to_string(),
            path: path.to_path_buf(),
        })
    }

    /// Read a note by name.
    pub fn read(&self, name: &str) -> Result<Note> {
        let path = self.resolve(name)?;
        self.read_path(&path)
    }

    /// Read a note by id. This walks and parses the whole tree: it is the slow path the
    /// index caches. Two notes sharing an id is an integrity error and is reported.
    pub fn read_by_id(&self, id: NoteId) -> Result<Note> {
        let mut found: Option<Note> = None;
        for entry in self.list()?.entries {
            let note = self.read_path(&entry.path)?;
            if note.front.id != id {
                continue;
            }
            if let Some(first) = &found {
                return Err(Error::Frontmatter {
                    path: note.path.clone(),
                    reason: format!(
                        "id {id} is also used by {}; ids are unique",
                        first.path.display()
                    ),
                });
            }
            found = Some(note);
        }
        found.ok_or_else(|| Error::NoSuchNote(id.to_string()))
    }

    /// Write a note atomically to `notes/r{ring}/{name}.md` and return that path.
    ///
    /// Refuses when the same name exists in a different ring (that is a move, and a move
    /// must be explicit: use [`Store::remove`] first). Enforces the resident cap for
    /// rings 0 and 1 before any byte is written.
    /// Everything [`Store::write`] checks before it touches the disk, and the rendered
    /// text it would have written.
    ///
    /// It is a separate function so that a dry run can call exactly this and nothing else.
    /// The alternative — a no-op writer that skips the checks — passes a dry run and then
    /// fails the real one halfway through, which is the worst possible outcome for a
    /// preview: it told you it was fine.
    pub fn validate_write(&self, note: &Note) -> Result<(PathBuf, String)> {
        let ring = note.front.ring;
        let name = &note.front.name;
        let target = self.note_path(ring, name)?;

        for other in Ring::ALL.iter().filter(|r| **r != ring) {
            let p = self.note_path(*other, name)?;
            if p.is_file() {
                return Err(Error::Frontmatter {
                    path: target,
                    reason: format!(
                        "name `{name}` already exists in ring {other} at {}; remove it first \
                         to move the note",
                        p.display()
                    ),
                });
            }
        }

        if ring.is_resident() {
            let others = self.resident_tokens_excluding(Some(&target))?;
            let actual = others + approx_tokens(&note.body) as usize;
            if actual > self.resident_cap {
                return Err(Error::RingCapExceeded {
                    ring: ring.as_u8(),
                    actual,
                    cap: self.resident_cap,
                });
            }
        }

        let text = frontmatter::render(&note.front, &note.body)?;
        Ok((target, text))
    }

    pub fn write(&self, note: &Note) -> Result<PathBuf> {
        let (target, text) = self.validate_write(note)?;
        write_atomic(&target, text.as_bytes())?;
        Ok(target)
    }

    /// Remove a note file by name and return the path that was removed. Only the file:
    /// index rows, vectors and links are the index's to erase (SPEC §12.2).
    pub fn remove(&self, name: &str) -> Result<PathBuf> {
        let path = self.resolve(name)?;
        fs::remove_file(&path).map_err(|e| Error::Io {
            path: path.clone(),
            source: e,
        })?;
        Ok(path)
    }

    /// Approximate tokens held by rings 0 and 1 together — the quantity the cap limits.
    pub fn resident_tokens(&self) -> Result<usize> {
        self.resident_tokens_excluding(None)
    }

    fn resident_tokens_excluding(&self, except: Option<&Path>) -> Result<usize> {
        let mut total = 0usize;
        for entry in self.list()?.entries {
            if !entry.ring.is_resident() {
                continue;
            }
            if except.is_some_and(|p| same_file_name(p, &entry.path)) {
                continue;
            }
            let note = self.read_path(&entry.path)?;
            total += approx_tokens(&note.body) as usize;
        }
        Ok(total)
    }
}

/// Same ring directory and file name. Paths from the walker and from `note_path` may
/// differ in prefix form (`./`, symlinked roots), so compare the tail.
fn same_file_name(a: &Path, b: &Path) -> bool {
    let tail = |p: &Path| {
        p.components()
            .rev()
            .take(2)
            .map(|c| c.as_os_str().to_os_string())
            .collect::<Vec<_>>()
    };
    tail(a) == tail(b)
}

// ---------------------------------------------------------------------------------------
// Atomic writes.

/// Write `bytes` to `path` atomically: temp file in the same directory, fsync, rename.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    write_atomic_with(path, |f| f.write_all(bytes))
}

/// Atomic write with a caller-supplied filler. If `fill` fails, `path` is untouched and
/// no temp file remains. The temp file is hidden (dot-prefixed) so a concurrent
/// [`Store::list`] never sees it.
pub fn write_atomic_with<F>(path: &Path, fill: F) -> Result<()>
where
    F: FnOnce(&mut File) -> io::Result<()>,
{
    let io_err = |source: io::Error| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| {
            io_err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path has no file name",
            ))
        })?
        .to_string_lossy();
    let tmp = dir.join(format!(".{file_name}.tmp-{}", ulid::Ulid::generate()));

    let attempt = (|| -> io::Result<()> {
        let mut f = File::create_new(&tmp)?;
        fill(&mut f)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        // Make the rename itself durable. Directory fsync is a Unix notion; elsewhere
        // opening a directory fails and there is nothing to do.
        if let Ok(d) = File::open(&dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();

    if attempt.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    attempt.map_err(io_err)
}

// ---------------------------------------------------------------------------------------
// Change detection.

/// What a file looked like: modification time, size and blake3 of its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    /// `None` where the filesystem reports no mtime.
    pub mtime: Option<SystemTime>,
    pub size: u64,
    pub hash: [u8; 32],
}

/// The outcome of comparing two fingerprints. Named so the caller can count what it
/// did: only `ContentChanged` needs blocks and vectors rebuilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Same mtime, same content.
    Unchanged,
    /// The content differs — whether or not the mtime does. This is the case an
    /// mtime-only comparison misses.
    ContentChanged,
    /// mtime moved but the bytes are identical (a `touch`, a copy, a checkout).
    TouchedOnly,
}

impl Fingerprint {
    /// Read the file and fingerprint it. Always hashes: an mtime shortcut is exactly the
    /// bug this type exists to prevent.
    pub fn of(path: &Path) -> Result<Fingerprint> {
        let io_err = |source: io::Error| Error::Io {
            path: path.to_path_buf(),
            source,
        };
        let bytes = fs::read(path).map_err(io_err)?;
        let meta = fs::metadata(path).map_err(io_err)?;
        Ok(Self::of_bytes(meta.modified().ok(), &bytes))
    }

    pub fn of_bytes(mtime: Option<SystemTime>, bytes: &[u8]) -> Fingerprint {
        Fingerprint {
            mtime,
            size: bytes.len() as u64,
            hash: *blake3::hash(bytes).as_bytes(),
        }
    }

    pub fn hash_hex(&self) -> String {
        self.hash.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Compare a stored fingerprint (`self`) with the current one.
    pub fn compare(&self, now: &Fingerprint) -> Change {
        if self.hash != now.hash || self.size != now.size {
            Change::ContentChanged
        } else if self.mtime != now.mtime {
            Change::TouchedOnly
        } else {
            Change::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Frontmatter, NoteKind, PiiState};

    fn ts() -> jiff::Timestamp {
        "2026-09-05T09:12:03Z".parse().unwrap()
    }

    fn note(ring: Ring, name: &str, body: &str) -> Note {
        Note {
            front: Frontmatter {
                id: NoteId::generate(),
                name: name.into(),
                ring,
                kind: NoteKind::Knowledge,
                created: ts(),
                updated: ts(),
                tags: vec![],
                links: vec![],
                retention: None,
                pii: PiiState::None,
            },
            body: body.into(),
            path: PathBuf::new(),
        }
    }

    fn fresh() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::create(dir.path(), 100).unwrap();
        (dir, store)
    }

    #[test]
    fn create_lays_out_the_tree_and_open_finds_it() {
        let (dir, store) = fresh();
        for r in Ring::ALL {
            assert!(store.ring_dir(r).is_dir(), "{}", r.dir());
        }
        assert!(store.models_dir().is_dir());
        assert_eq!(store.db_path(), dir.path().join("cyberbrain.db"));
        assert_eq!(store.config_path(), dir.path().join("cyberbrain.toml"));
        let reopened = Store::open(dir.path()).unwrap();
        assert_eq!(
            reopened.resident_cap(),
            Config::default().rings.resident_cap_tokens
        );
    }

    #[test]
    fn open_refuses_a_directory_that_is_not_a_store() {
        let dir = tempfile::tempdir().unwrap();
        match Store::open(dir.path()) {
            Err(Error::Io { path, source }) => {
                assert_eq!(path, dir.path());
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
                assert!(source.to_string().contains("not a cyberbrain store"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn write_then_read_by_name_and_by_id() {
        let (_dir, store) = fresh();
        let n = note(
            Ring::Knowledge,
            "pg18-moves-pgdata",
            "# Title\n\nBody with [[link]].\n",
        );
        let path = store.write(&n).unwrap();
        assert_eq!(
            path,
            store.ring_dir(Ring::Knowledge).join("pg18-moves-pgdata.md")
        );
        assert!(path.is_file());

        let back = store.read("pg18-moves-pgdata").unwrap();
        assert_eq!(back.front.id, n.front.id);
        assert_eq!(back.body, n.body);
        assert_eq!(back.path, path);

        let by_id = store.read_by_id(n.front.id).unwrap();
        assert_eq!(by_id.front.name, "pg18-moves-pgdata");

        assert_eq!(store.resolve("pg18-moves-pgdata").unwrap(), path);
        assert!(matches!(store.resolve("nope"), Err(Error::NoSuchNote(n)) if n == "nope"));
        assert!(matches!(
            store.read_by_id(NoteId::generate()),
            Err(Error::NoSuchNote(_))
        ));
    }

    #[test]
    fn write_overwrites_in_place_and_leaves_no_temp_files() {
        let (_dir, store) = fresh();
        let mut n = note(Ring::Session, "s", "v1");
        store.write(&n).unwrap();
        n.body = "v2".into();
        store.write(&n).unwrap();
        assert_eq!(store.read("s").unwrap().body, "v2");
        let names: Vec<String> = fs::read_dir(store.ring_dir(Ring::Session))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["s.md"]);
    }

    #[test]
    fn write_refuses_a_name_that_lives_in_another_ring() {
        let (_dir, store) = fresh();
        store.write(&note(Ring::Knowledge, "dup", "a")).unwrap();
        let err = store.write(&note(Ring::Session, "dup", "b")).unwrap_err();
        assert!(
            matches!(&err, Error::Frontmatter { reason, .. } if reason.contains("already exists in ring r2")),
            "{err}"
        );
        // After removing, the move is allowed.
        let removed = store.remove("dup").unwrap();
        assert!(!removed.exists());
        store.write(&note(Ring::Session, "dup", "b")).unwrap();
        assert_eq!(store.read("dup").unwrap().front.ring, Ring::Session);
    }

    #[test]
    fn write_refuses_a_bad_name_before_touching_disk() {
        let (_dir, store) = fresh();
        for bad in ["../escape", "Upper", "a/b"] {
            let err = store.write(&note(Ring::Knowledge, bad, "x")).unwrap_err();
            assert!(matches!(err, Error::Frontmatter { .. }), "{bad}: {err}");
        }
        assert!(store.list().unwrap().entries.is_empty());
        assert!(!store.root().join("escape.md").exists());
    }

    #[test]
    fn resident_cap_is_enforced_across_rings_0_and_1_at_write_time() {
        let (_dir, store) = fresh(); // cap 100
        let sixty = "word ".repeat(60); // 60 tokens
        store.write(&note(Ring::Invariant, "inv", &sixty)).unwrap();
        assert_eq!(store.resident_tokens().unwrap(), 60);

        // 60 + 50 > 100: refused, and the file is not written.
        let fifty = "word ".repeat(50);
        match store.write(&note(Ring::Protocol, "proto", &fifty)) {
            Err(Error::RingCapExceeded { ring, actual, cap }) => {
                assert_eq!((ring, actual, cap), (1, 110, 100));
            }
            other => panic!("{other:?}"),
        }
        assert!(!store.ring_dir(Ring::Protocol).join("proto.md").exists());

        // 60 + 40 = 100: exactly at the cap is fine.
        store
            .write(&note(Ring::Protocol, "proto", &"word ".repeat(40)))
            .unwrap();
        assert_eq!(store.resident_tokens().unwrap(), 100);

        // Rewriting an existing resident note counts its new size, not old + new.
        store
            .write(&note(Ring::Invariant, "inv", &"word ".repeat(55)))
            .unwrap();
        assert_eq!(store.resident_tokens().unwrap(), 95);
        assert!(matches!(
            store.write(&note(Ring::Invariant, "inv", &"word ".repeat(61))),
            Err(Error::RingCapExceeded { actual: 101, .. })
        ));

        // Ring 2 is not capped.
        store
            .write(&note(Ring::Knowledge, "big", &"word ".repeat(5000)))
            .unwrap();
    }

    #[test]
    fn list_walks_rings_sorted_and_reports_what_it_skips() {
        let (_dir, store) = fresh();
        store.write(&note(Ring::Knowledge, "b", "x")).unwrap();
        store.write(&note(Ring::Knowledge, "a", "x")).unwrap();
        store.write(&note(Ring::External, "c", "x")).unwrap();
        fs::write(store.notes_dir().join("stray.md"), "x").unwrap();
        fs::create_dir(store.notes_dir().join("r9")).unwrap();
        fs::write(store.notes_dir().join("r9").join("n.md"), "x").unwrap();
        fs::create_dir(store.ring_dir(Ring::Knowledge).join("nested")).unwrap();
        fs::write(
            store
                .ring_dir(Ring::Knowledge)
                .join("nested")
                .join("deep.md"),
            "x",
        )
        .unwrap();
        fs::write(store.ring_dir(Ring::Knowledge).join("notes.txt"), "x").unwrap();
        fs::write(store.ring_dir(Ring::Knowledge).join("Bad_Name.md"), "x").unwrap();
        fs::write(store.ring_dir(Ring::Knowledge).join(".hidden.md"), "x").unwrap();

        let l = store.list().unwrap();
        let names: Vec<(Ring, &str)> = l
            .entries
            .iter()
            .map(|e| (e.ring, e.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                (Ring::Knowledge, "a"),
                (Ring::Knowledge, "b"),
                (Ring::External, "c")
            ]
        );

        let skipped: Vec<String> = l
            .skipped
            .iter()
            .map(|s| {
                // Separators normalised for the comparison only. `Skipped.path` is a
                // `PathBuf` and stays native, which is right for a path a caller may want
                // to open; it is this assertion's hardcoded forward slashes that are
                // platform-specific, not the value.
                s.path
                    .strip_prefix(store.notes_dir())
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(
            skipped,
            [
                "r2/Bad_Name.md",
                "r2/nested",
                "r2/notes.txt",
                "r9",
                "stray.md"
            ]
        );
        assert!(l.skipped.iter().all(|s| !s.reason.is_empty()));
        assert_eq!(l, store.list().unwrap(), "listing is deterministic");
    }

    #[test]
    fn list_honours_cyberbrainignore_at_store_root_and_inside_notes() {
        let (_dir, store) = fresh();
        store.write(&note(Ring::Knowledge, "keep", "x")).unwrap();
        store
            .write(&note(Ring::Knowledge, "old-draft", "x"))
            .unwrap();
        store.write(&note(Ring::External, "imported", "x")).unwrap();
        store.write(&note(Ring::Session, "sess", "x")).unwrap();
        fs::write(store.root().join(IGNORE_FILE), "*-draft.md\n").unwrap();
        fs::write(store.notes_dir().join(IGNORE_FILE), "r4/\n").unwrap();
        fs::write(store.ring_dir(Ring::Session).join(IGNORE_FILE), "sess.md\n").unwrap();

        let names: Vec<String> = store
            .list()
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["keep"]);
    }

    #[test]
    fn list_does_not_honour_gitignore() {
        // The store is usually inside a project whose .gitignore lists it. Honouring
        // that would hide every note.
        let (dir, store) = fresh();
        store.write(&note(Ring::Knowledge, "n", "x")).unwrap();
        fs::write(dir.path().join(".gitignore"), "*\n").unwrap();
        fs::write(store.notes_dir().join(".gitignore"), "*.md\n").unwrap();
        assert_eq!(store.list().unwrap().entries.len(), 1);
    }

    #[test]
    fn read_rejects_a_file_whose_frontmatter_disagrees_with_its_place() {
        let (_dir, store) = fresh();
        let n = note(Ring::Knowledge, "real-name", "x");
        let text = frontmatter::render(&n.front, &n.body).unwrap();
        let wrong = store.ring_dir(Ring::Knowledge).join("other-name.md");
        fs::write(&wrong, &text).unwrap();
        let err = store.read("other-name").unwrap_err();
        assert!(
            matches!(&err, Error::Frontmatter { path, reason } if path == &wrong && reason.contains("real-name")),
            "{err}"
        );

        let wrong_ring = store.ring_dir(Ring::Session).join("real-name.md");
        fs::write(&wrong_ring, &text).unwrap();
        let err = store.read_path(&wrong_ring).unwrap_err();
        assert!(
            matches!(&err, Error::Frontmatter { reason, .. } if reason.contains("ring r2")),
            "{err}"
        );
    }

    #[test]
    fn duplicate_names_and_ids_are_reported_not_resolved_by_luck() {
        let (_dir, store) = fresh();
        let n = note(Ring::Knowledge, "twice", "x");
        let text = frontmatter::render(&n.front, &n.body).unwrap();
        fs::write(store.ring_dir(Ring::Knowledge).join("twice.md"), &text).unwrap();
        let mut n3 = n.clone();
        n3.front.ring = Ring::Session;
        fs::write(
            store.ring_dir(Ring::Session).join("twice.md"),
            frontmatter::render(&n3.front, &n3.body).unwrap(),
        )
        .unwrap();
        assert!(
            matches!(store.resolve("twice"), Err(Error::Frontmatter { reason, .. }) if reason.contains("also exists"))
        );
        assert!(
            matches!(store.read_by_id(n.front.id), Err(Error::Frontmatter { reason, .. }) if reason.contains("also used by"))
        );
    }

    #[test]
    fn malformed_note_errors_name_the_file() {
        let (_dir, store) = fresh();
        let p = store.ring_dir(Ring::Knowledge).join("broken.md");
        fs::write(&p, "---\nname: broken\n---\n").unwrap();
        match store.read("broken") {
            Err(Error::Frontmatter { path, reason }) => {
                assert_eq!(path, p);
                assert!(reason.contains("missing field"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    // --- atomic writes -------------------------------------------------------------

    /// Regression: under a naive implementation (open target, truncate, write) a filler
    /// that fails half-way leaves a corrupt target. Verified to fail against that
    /// implementation before the atomic one was written.
    #[test]
    fn failed_write_leaves_the_old_file_intact_and_no_temp_behind() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("note.md");
        fs::write(&target, "old complete content").unwrap();

        let err = write_atomic_with(&target, |f| {
            f.write_all(b"new partial")?;
            Err(io::Error::other("disk on fire"))
        })
        .unwrap_err();
        assert!(
            matches!(&err, Error::Io { path, source } if path == &target && source.to_string().contains("disk on fire"))
        );

        assert_eq!(fs::read_to_string(&target).unwrap(), "old complete content");
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, ["note.md"]);
    }

    #[test]
    fn write_atomic_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("n.md");
        write_atomic(&target, b"one").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"one");
        write_atomic(&target, b"two").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"two");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Regression: a reader racing a naive truncate-and-write observes empty or partial
    /// files. With rename it sees exactly one of the two complete versions. Fails under
    /// `fs::write` in practice within a handful of iterations at this file size.
    #[cfg(unix)]
    #[test]
    fn concurrent_reader_never_observes_a_partial_file() {
        // Scoped to this test: it is the only user, and the test is unix-only, so at
        // module level the import is dead code on Windows and `-D warnings` fails the
        // build there. Cheap to get wrong and invisible until a Windows job runs.
        use std::io::Read;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("n.md");
        let a = vec![b'a'; 1 << 20];
        let b = vec![b'b'; 1 << 20];
        write_atomic(&target, &a).unwrap();

        let stop = Arc::new(AtomicBool::new(false));
        let reader = {
            let stop = stop.clone();
            let target = target.clone();
            let (a, b) = (a.clone(), b.clone());
            std::thread::spawn(move || {
                let mut reads = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    let mut buf = Vec::new();
                    File::open(&target).unwrap().read_to_end(&mut buf).unwrap();
                    assert!(
                        buf == a || buf == b,
                        "observed a partial file of {} bytes",
                        buf.len()
                    );
                    reads += 1;
                }
                reads
            })
        };
        for i in 0..200 {
            write_atomic(&target, if i % 2 == 0 { &b } else { &a }).unwrap();
        }
        stop.store(true, Ordering::Relaxed);
        let reads = reader.join().expect("reader saw a partial file");
        assert!(reads > 0, "the reader must actually have raced the writer");
    }

    // --- change detection ----------------------------------------------------------

    /// Regression: same mtime, same size, different bytes. This is what a bind mount or
    /// an archive extraction produces. An mtime-only comparison reports Unchanged here;
    /// this test fails against that implementation.
    #[test]
    fn changed_content_with_identical_mtime_and_size_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("n.md");
        fs::write(&p, "content version A").unwrap();
        let before = Fingerprint::of(&p).unwrap();
        let mtime = before.mtime.expect("this filesystem reports mtimes");

        fs::write(&p, "content version B").unwrap(); // same length
        File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(mtime)
            .unwrap();

        let after = Fingerprint::of(&p).unwrap();
        assert_eq!(
            after.mtime, before.mtime,
            "the test must reproduce identical mtimes"
        );
        assert_eq!(after.size, before.size);
        assert_eq!(before.compare(&after), Change::ContentChanged);
        assert_ne!(before.hash_hex(), after.hash_hex());
    }

    #[test]
    fn touched_but_identical_content_is_reported_as_touched_not_changed() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("n.md");
        fs::write(&p, "same").unwrap();
        let before = Fingerprint::of(&p).unwrap();
        let later = before.mtime.unwrap() + std::time::Duration::from_secs(3600);
        File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(later)
            .unwrap();
        let after = Fingerprint::of(&p).unwrap();
        assert_eq!(before.compare(&after), Change::TouchedOnly);
        assert_eq!(before.compare(&before), Change::Unchanged);
    }

    #[test]
    fn fingerprint_of_bytes_is_pure() {
        let x = Fingerprint::of_bytes(None, b"abc");
        assert_eq!(x, Fingerprint::of_bytes(None, b"abc"));
        assert_eq!(x.size, 3);
        assert_eq!(x.hash_hex().len(), 64);
        assert_ne!(x.hash, Fingerprint::of_bytes(None, b"abd").hash);
    }
}
