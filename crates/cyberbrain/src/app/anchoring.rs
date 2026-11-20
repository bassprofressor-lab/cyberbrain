//! Anchoring notes to code, and saying when the code moved (Graphify's "code changed —
//! re-verify", 2026-11-13).
//!
//! A note can carry `anchors` in its head: a project-relative path, optionally lines, and the
//! SHA-256 of the whole file when the note was written against it. They come from
//! `write --anchor`, from `cyberbrain anchor`, and, where somebody is writing about code they
//! just looked at (CLI `write`, MCP, the page), from `path:line` references in the body when
//! that file exists inside the project.
//!
//! Recall and `find` then compare each anchor of what they return with the file now and mark
//! the ones that differ or are gone. What this costs, because a memory may not get slower for
//! it: nothing is hashed at write time beyond the files named, nothing is scanned at recall
//! beyond the anchors of the hits (one indexed query per hit note), and a file is hashed again
//! only when its mtime or size moved since the last time (`anchor_files` in the index).
//!
//! The project is the directory the store sits in (`<project>/.cyberbrain`). A store placed
//! elsewhere has no project, and its notes are not anchored.

use super::*;
use cyberbrain_core::anchor::{self, Anchor};
use cyberbrain_core::{AnchorDrift, DriftState, Hit};
use cyberbrain_index::FileHash;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// A file larger than this is not anchored: a note about a 64 MiB file is about a data file,
/// and hashing it on every changed recall would be the cost this feature must not add.
const MAX_ANCHOR_BYTES: u64 = 64 << 20;
/// At most this many anchors per note, whatever asked for them.
const MAX_ANCHORS: usize = 32;

/// SHA-256 of a file, streamed.
pub(crate) fn sha256_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>())
}

/// `cyberbrain anchor <note> [path[:lines]…] [--remove path…]`.
#[derive(Debug, Clone, Default)]
pub struct AnchorRequest {
    pub name: String,
    /// Anchor these, replacing an anchor on the same path and lines. Empty, together with an
    /// empty `remove`: re-anchor everything the note has, which says "checked against the
    /// code as it is now".
    pub specs: Vec<String>,
    /// Drop anchors on these paths (`path`, every lines; `path:lines`, that one).
    pub remove: Vec<String>,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnchoredNote {
    pub name: String,
    pub ring: Ring,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    /// The anchors the head carries now.
    pub anchors: Vec<Anchor>,
    /// `path[:lines]` of every anchor hashed by this call.
    pub anchored: Vec<String>,
    pub removed: Vec<String>,
    pub dry_run: bool,
    pub audit_preview: Vec<String>,
}

/// A note anchored to a file a `find` hit lies in.
#[derive(Debug, Clone, Serialize)]
pub struct AnchoredFinding {
    /// Relative to the project, as the anchor says it.
    pub path: String,
    pub note: String,
    pub ring: Ring,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<String>,
    pub since: jiff::Timestamp,
    /// `None`: the file is what the note was written against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift: Option<DriftState>,
}

impl App {
    /// The project this store belongs to: the directory holding `.cyberbrain`. `None` for a
    /// store placed anywhere else, which says nothing about where its code is.
    pub(crate) fn anchor_root(&self) -> Option<PathBuf> {
        let is_default = self
            .root
            .file_name()
            .is_some_and(|n| n == DEFAULT_STORE_DIR);
        let parent = self.root.parent().filter(|p| !p.as_os_str().is_empty())?;
        is_default.then(|| parent.to_path_buf())
    }

    /// Whether this caller reaches the memory only (MCP, an agent, a hook, or a served
    /// store, which answers any local process), not the file system (N5, review 2026-11-19).
    fn anchors_restricted(&self) -> bool {
        matches!(self.actor, Actor::Mcp | Actor::Agent(_) | Actor::Hook(_))
            || self.refuses_resident_writes()
    }

    /// One anchor, hashed now. `Err` says why not, in words for the person who named it.
    ///
    /// For a caller that reaches only the memory (see `anchors_restricted`) the file must be
    /// one `find` would show (no hidden entry, not ignored), and every refusal is the same
    /// sentence: `write` with `anchors: [".env"]` put the SHA-256 of `.env` into the note's
    /// head, and the different refusals said which paths exist (N5).
    fn resolve_anchor(
        &self,
        root: &Path,
        raw: &str,
        lines: Option<&str>,
        at: jiff::Timestamp,
    ) -> std::result::Result<Anchor, String> {
        if !self.anchors_restricted() {
            return self.resolve_anchor_any(root, raw, lines, at);
        }
        const REFUSED: &str = "cannot be anchored from here: only files inside the project \
             that `find` shows (not hidden, not ignored) are";
        let a = self
            .resolve_anchor_any(root, raw, lines, at)
            .map_err(|_| REFUSED.to_string())?;
        match cyberbrain_code::excluded(root, Path::new(&a.path)) {
            Some(false) => Ok(a),
            _ => Err(REFUSED.to_string()),
        }
    }

    fn resolve_anchor_any(
        &self,
        root: &Path,
        raw: &str,
        lines: Option<&str>,
        at: jiff::Timestamp,
    ) -> std::result::Result<Anchor, String> {
        let given = Path::new(raw);
        let joined = if given.is_absolute() {
            given.to_path_buf()
        } else {
            root.join(given)
        };
        let canon_root = root
            .canonicalize()
            .map_err(|e| format!("the project {} cannot be resolved: {e}", Slash(root)))?;
        // Canonical, so a symlink or `..` cannot carry the anchor out of the project.
        let canon = joined
            .canonicalize()
            .map_err(|_| format!("{} does not exist", Slash(&joined)))?;
        if !canon.starts_with(&canon_root) {
            return Err(format!("lies outside the project {}", Slash(&canon_root)));
        }
        if let Ok(store) = self.root.canonicalize()
            && canon.starts_with(&store)
        {
            return Err(
                "lies inside the store; a note names another note with [[name]], it does not \
                 anchor to it"
                    .into(),
            );
        }
        let md = std::fs::metadata(&canon).map_err(|e| e.to_string())?;
        if !md.is_file() {
            return Err("is not a file".into());
        }
        if md.len() > MAX_ANCHOR_BYTES {
            return Err(format!(
                "is {} bytes; files over {MAX_ANCHOR_BYTES} are not anchored",
                md.len()
            ));
        }
        let rel = canon
            .strip_prefix(&canon_root)
            .map_err(|_| "lies outside the project".to_string())?;
        let rel = cyberbrain_core::slash(rel);
        anchor::validate_path(&rel).map_err(str::to_string)?;
        if let Some(l) = lines {
            anchor::validate_lines(l).map_err(|why| format!("lines `{l}`: {why}"))?;
        }
        let sha256 = sha256_file(&canon).map_err(|e| format!("cannot be read: {e}"))?;
        Ok(Anchor {
            path: rel,
            lines: lines.map(str::to_string),
            sha256,
            at,
        })
    }

    /// The anchors a written note carries: what it had, then what the request names (hashed
    /// now, replacing the same path and lines), then what detection finds on paths not yet
    /// anchored. An anchor the note already has keeps its hash: nobody said it was checked
    /// again, and re-hashing on every edit would clear the mark for a typo fix.
    pub(crate) fn anchors_for_write(
        &self,
        existing: &[Anchor],
        body: &str,
        req: &Anchoring,
        now: jiff::Timestamp,
    ) -> Result<Vec<Anchor>> {
        let mut out = existing.to_vec();
        if req.explicit.is_empty() && !req.detect {
            return Ok(out);
        }
        let Some(root) = self.anchor_root() else {
            if req.explicit.is_empty() {
                return Ok(out);
            }
            return Err(Error::Config(format!(
                "the store {} does not sit in a project (`<project>/{DEFAULT_STORE_DIR}`), so \
                 there is no project to anchor a note to",
                Slash(&self.root)
            )));
        };
        for spec in &req.explicit {
            let (p, lines) = anchor::split_spec(spec);
            let a = self
                .resolve_anchor(&root, p, lines, now)
                .map_err(|why| Error::Config(format!("anchor `{spec}`: {why}")))?;
            out.retain(|o| !(o.path == a.path && o.lines == a.lines));
            out.push(a);
        }
        if req.detect {
            for (p, lines) in anchor::detect(body) {
                if out.len() >= MAX_ANCHORS {
                    break;
                }
                // A reference that does not resolve is prose, not an error: the agent may
                // have meant a file of another project, or one that is gone.
                if let Ok(a) = self.resolve_anchor(&root, &p, Some(&lines), now)
                    && !out.iter().any(|o| o.path == a.path)
                {
                    out.push(a);
                }
            }
        }
        if out.len() > MAX_ANCHORS {
            return Err(Error::Config(format!(
                "a note carries at most {MAX_ANCHORS} anchors; this one would have {}",
                out.len()
            )));
        }
        Ok(out)
    }

    /// The hash `abs` has now: from the cache when its mtime and size did not move, else read
    /// and remembered. `None`: not a readable file.
    fn current_hash(ix: &Index, abs: &Path) -> Option<String> {
        let stamp = FileHash::of(abs)?;
        if let Ok(Some(h)) = ix.cached_file_hash(abs, stamp) {
            return Some(h);
        }
        if stamp.size > MAX_ANCHOR_BYTES {
            // Grew past the cap since it was anchored: changed, without reading it.
            return Some(String::new());
        }
        let h = sha256_file(abs).ok()?;
        // A cache: failing to fill it costs the next recall one more read, nothing else.
        let _ = ix.remember_file_hash(abs, stamp, &h);
        Some(h)
    }

    fn drift(ix: &Index, root: &Path, a: &Anchor) -> Option<DriftState> {
        match Self::current_hash(ix, &root.join(&a.path)) {
            None => Some(DriftState::Missing),
            Some(h) if h != a.sha256 => Some(DriftState::Changed),
            Some(_) => None,
        }
    }

    /// Mark every hit whose note is anchored to a file that changed or went missing since.
    /// Never fails a recall: an index that cannot answer leaves the hits unmarked.
    pub(crate) fn annotate_code_drift(&self, hits: &mut [Hit]) {
        if hits.is_empty() {
            return;
        }
        let Some(root) = self.anchor_root() else {
            return;
        };
        let Ok(ix) = lock_index(&self.index) else {
            return;
        };
        let mut memo: HashMap<NoteId, Vec<AnchorDrift>> = HashMap::new();
        for h in hits.iter_mut() {
            let drift = memo.entry(h.note_id).or_insert_with(|| {
                ix.anchors_of(&h.note_id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|row| {
                        let state = Self::drift(&ix, &root, &row.anchor)?;
                        Some(AnchorDrift {
                            path: row.anchor.path,
                            lines: row.anchor.lines,
                            since: row.anchor.at,
                            state,
                        })
                    })
                    .collect()
            });
            h.code_changed = drift.clone();
        }
    }

    /// For a note shown whole (the page's note view): its anchors that drifted.
    pub fn note_drift(&self, note: &Note) -> Vec<AnchorDrift> {
        let (Some(root), Ok(ix)) = (self.anchor_root(), lock_index(&self.index)) else {
            return Vec::new();
        };
        note.front
            .anchors
            .iter()
            .filter_map(|a| {
                Some(AnchorDrift {
                    path: a.path.clone(),
                    lines: a.lines.clone(),
                    since: a.at,
                    state: Self::drift(&ix, &root, a)?,
                })
            })
            .collect()
    }

    /// Notes anchored to the files of these `find` hits, each with whether the file moved.
    pub(crate) fn anchored_findings(&self, report: &FindReport) -> Vec<AnchoredFinding> {
        let (Some(root), Ok(ix)) = (self.anchor_root(), lock_index(&self.index)) else {
            return Vec::new();
        };
        let mut seen: HashSet<&str> = HashSet::new();
        let mut out = Vec::new();
        for h in &report.hits {
            if !seen.insert(h.path.as_str()) {
                continue;
            }
            for row in ix.anchored_to(&h.path).unwrap_or_default() {
                out.push(AnchoredFinding {
                    drift: Self::drift(&ix, &root, &row.anchor),
                    path: row.anchor.path,
                    note: row.note_name,
                    ring: row.ring,
                    lines: row.anchor.lines,
                    since: row.anchor.at,
                });
            }
        }
        out
    }

    /// `cyberbrain anchor`: anchor a note to files, re-anchor it, or drop anchors, without
    /// touching its text. A head-only write like `invalidate`: the ring owner's guard, one
    /// `note.write` row (`op: anchor`), a reindex. An agent may do it outside rings 0 and 1;
    /// re-anchoring says "I checked it against the code", which is what the mark asked for.
    pub fn anchor(&self, req: AnchorRequest) -> Result<AnchoredNote> {
        let name = frontmatter::normalize_name(req.name.trim()).into_owned();
        let w = self.writers(req.dry_run)?;
        let policy = w.policy.get();
        let existing = self.store.read(&name)?;
        self.refuse_resident_unless_operator(
            &name,
            existing.front.ring,
            Some(&existing),
            req.dry_run,
        )?;
        let now = jiff::Timestamp::now();
        let mut anchors = existing.front.anchors.clone();
        let mut removed = Vec::new();
        for spec in &req.remove {
            let (p, lines) = anchor::split_spec(spec);
            let p = p.strip_prefix("./").unwrap_or(p);
            let before = anchors.len();
            anchors.retain(|a| !(a.path == p && (lines.is_none() || a.lines.as_deref() == lines)));
            if anchors.len() == before {
                return Err(Error::Config(format!(
                    "{name} has no anchor `{spec}`; it has: {}",
                    existing
                        .front
                        .anchors
                        .iter()
                        .map(Anchor::label)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            removed.push(spec.clone());
        }
        let specs: Vec<String> = if req.specs.is_empty() && req.remove.is_empty() {
            if anchors.is_empty() {
                return Err(Error::Config(format!(
                    "{name} has no anchors to renew; name a file: `cyberbrain anchor {name} \
                     path/to/file.rs:120`"
                )));
            }
            anchors.iter().map(Anchor::label).collect()
        } else {
            req.specs.clone()
        };
        let mut anchored = Vec::new();
        if !specs.is_empty() {
            let Some(root) = self.anchor_root() else {
                return Err(Error::Config(format!(
                    "the store {} does not sit in a project, so there is nothing to anchor to",
                    Slash(&self.root)
                )));
            };
            for spec in &specs {
                let (p, lines) = anchor::split_spec(spec);
                let a = self.resolve_anchor(&root, p, lines, now).map_err(|why| {
                    Error::Config(format!(
                        "anchor `{spec}`: {why}. To drop it: `cyberbrain anchor {name} --remove \
                         {spec}`"
                    ))
                })?;
                anchored.push(a.label());
                anchors.retain(|o| !(o.path == a.path && o.lines == a.lines));
                anchors.push(a);
            }
        }
        if anchors.len() > MAX_ANCHORS {
            return Err(Error::Config(format!(
                "a note carries at most {MAX_ANCHORS} anchors; this one would have {}",
                anchors.len()
            )));
        }
        let mut front = existing.front.clone();
        front.anchors = anchors;
        front.updated = now;
        let note = Note {
            front,
            body: existing.body.clone(),
            path: PathBuf::new(),
        };
        let bytes = frontmatter::render(&note.front, &note.body)?.len();
        let path = w.notes.write(&note)?;
        let note = Note { path, ..note };
        if let Err(e) = policy
            .record_write_as(&note.front, bytes, Some("anchor"))
            .and_then(|()| {
                if req.dry_run {
                    Ok(())
                } else {
                    self.attest_resident(policy, &note, "anchor")
                }
            })
        {
            Self::undo_note_write(&w, &note, Some(&existing), &e);
            return Err(e);
        }
        let _ = self.index_written(&w, &note);
        Ok(AnchoredNote {
            name,
            ring: note.front.ring,
            path: note.path,
            anchors: note.front.anchors,
            anchored,
            removed,
            dry_run: req.dry_run,
            audit_preview: w.policy.preview(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Project {
        _dir: tempfile::TempDir,
        root: PathBuf,
        app: App,
    }

    fn project(actor: Actor) -> Project {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn guard() -> bool {\n    true\n}\n",
        )
        .unwrap();
        let store = root.join(DEFAULT_STORE_DIR);
        App::init(&store, &Actor::Operator).unwrap();
        let app = App::open(Some(&store), actor).unwrap();
        Project {
            _dir: dir,
            root,
            app,
        }
    }

    fn write(app: &App, name: &str, body: &str, anchors: Anchoring) -> Result<WriteOutcome> {
        app.write(WriteRequest {
            ring: Ring::Knowledge,
            kind: NoteKind::Knowledge,
            name: name.to_string(),
            body: body.to_string(),
            tags: Vec::new(),
            bereich: None,
            retention: None,
            force: false,
            choice: None,
            expected_updated: None,
            supersedes: None,
            valid_from: None,
            invalid_at: None,
            arriving: None,
            anchors,
            dry_run: false,
        })
    }

    fn explicit(specs: &[&str]) -> Anchoring {
        Anchoring {
            explicit: specs.iter().map(|s| s.to_string()).collect(),
            detect: false,
        }
    }

    fn hits(app: &App, q: &str) -> Vec<Hit> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(app.recall(q, &RecallRequest::default()))
            .unwrap()
            .hits
    }

    fn drift_of(app: &App, q: &str) -> Vec<AnchorDrift> {
        let hits = hits(app, q);
        assert!(!hits.is_empty(), "no hit for {q}");
        hits[0].code_changed.clone()
    }

    /// The whole loop: anchored, unchanged, changed, renewed, gone.
    #[test]
    fn recall_says_when_anchored_code_changed_and_renewing_clears_it() {
        let p = project(Actor::Operator);
        write(
            &p.app,
            "guard-note",
            "The guard returns true unconditionally.",
            explicit(&["src/lib.rs:1-3"]),
        )
        .unwrap();
        let note = p.app.store().read("guard-note").unwrap();
        assert_eq!(note.front.anchors.len(), 1);
        let a = &note.front.anchors[0];
        assert_eq!(
            (a.path.as_str(), a.lines.as_deref()),
            ("src/lib.rs", Some("1-3"))
        );
        assert_eq!(
            a.sha256,
            sha256_file(&p.root.join("src/lib.rs")).unwrap(),
            "the head carries the file's SHA-256"
        );
        assert!(drift_of(&p.app, "guard unconditionally").is_empty());

        std::fs::write(
            p.root.join("src/lib.rs"),
            "pub fn guard() -> bool {\n    false\n}\n",
        )
        .unwrap();
        let d = drift_of(&p.app, "guard unconditionally");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].state, DriftState::Changed);
        assert_eq!(d[0].path, "src/lib.rs");
        assert_eq!(d[0].since, a.at);

        p.app
            .anchor(AnchorRequest {
                name: "guard-note".into(),
                ..AnchorRequest::default()
            })
            .unwrap();
        assert!(
            drift_of(&p.app, "guard unconditionally").is_empty(),
            "renewed against the code as it is now"
        );

        std::fs::remove_file(p.root.join("src/lib.rs")).unwrap();
        let d = drift_of(&p.app, "guard unconditionally");
        assert_eq!(d[0].state, DriftState::Missing);
    }

    /// The second recall of an unchanged file reads its hash from the cache.
    #[test]
    fn a_recall_hashes_an_unchanged_file_once() {
        let p = project(Actor::Operator);
        write(
            &p.app,
            "cached",
            "Cache me if you can.",
            explicit(&["src/lib.rs"]),
        )
        .unwrap();
        let abs = p.root.join("src/lib.rs");
        let stamp = FileHash::of(&abs).unwrap();
        assert_eq!(
            lock_index(&p.app.index)
                .unwrap()
                .cached_file_hash(&abs, stamp)
                .unwrap(),
            None
        );
        assert!(drift_of(&p.app, "cache me").is_empty());
        let cached = lock_index(&p.app.index)
            .unwrap()
            .cached_file_hash(&abs, stamp)
            .unwrap();
        assert_eq!(cached, Some(sha256_file(&abs).unwrap()));
    }

    /// Detection anchors `path:line` references to files inside the project, and nothing
    /// else: not a file that does not exist, not one outside, not the store, not a path
    /// without a line, and not at all when the writer did not ask for it.
    #[test]
    fn detection_anchors_only_real_files_inside_the_project() {
        let p = project(Actor::Operator);
        let outside = p.root.parent().unwrap().join("outside.rs");
        std::fs::write(&outside, "x").unwrap();
        let body = format!(
            "See src/lib.rs:2 and src/gone.rs:4, {}:1, ../outside.rs:1, src/lib.rs in passing, \
             and .cyberbrain/cyberbrain.toml:1.",
            outside.display()
        );
        write(
            &p.app,
            "detected",
            &body,
            Anchoring {
                explicit: Vec::new(),
                detect: true,
            },
        )
        .unwrap();
        let anchors = p.app.store().read("detected").unwrap().front.anchors;
        assert_eq!(anchors.len(), 1, "{anchors:?}");
        assert_eq!(
            (anchors[0].path.as_str(), anchors[0].lines.as_deref()),
            ("src/lib.rs", Some("2"))
        );

        write(
            &p.app,
            "undetected",
            "See src/lib.rs:2.",
            Anchoring::default(),
        )
        .unwrap();
        assert!(
            p.app
                .store()
                .read("undetected")
                .unwrap()
                .front
                .anchors
                .is_empty()
        );
    }

    /// An edit keeps the hash an anchor was taken with; only naming it again renews it.
    #[test]
    fn an_edit_keeps_the_old_hash() {
        let p = project(Actor::Operator);
        write(&p.app, "kept", "First text.", explicit(&["src/lib.rs"])).unwrap();
        let before = p.app.store().read("kept").unwrap().front.anchors;
        std::fs::write(p.root.join("src/lib.rs"), "changed\n").unwrap();
        write(
            &p.app,
            "kept",
            "Second text, src/lib.rs:1 mentioned.",
            Anchoring {
                explicit: Vec::new(),
                detect: true,
            },
        )
        .unwrap();
        assert_eq!(p.app.store().read("kept").unwrap().front.anchors, before);
        write(&p.app, "kept", "Third text.", explicit(&["src/lib.rs"])).unwrap();
        let after = p.app.store().read("kept").unwrap().front.anchors;
        assert_eq!(after.len(), 1);
        assert_ne!(after[0].sha256, before[0].sha256);
    }

    /// A named anchor that cannot be taken refuses the write; nothing is written.
    #[test]
    fn an_explicit_anchor_outside_the_project_refuses_the_write() {
        let p = project(Actor::Operator);
        let outside = p.root.parent().unwrap().join("outside.rs");
        std::fs::write(&outside, "x").unwrap();
        for spec in [
            outside.to_string_lossy().into_owned(),
            "../outside.rs".to_string(),
            "src/missing.rs".to_string(),
            ".cyberbrain/cyberbrain.toml".to_string(),
            "src".to_string(),
        ] {
            let e = write(&p.app, "refused", "text", explicit(&[&spec])).unwrap_err();
            assert!(e.to_string().contains("anchor"), "{spec}: {e}");
            assert!(
                p.app.store().read("refused").is_err(),
                "{spec} wrote the note"
            );
        }
    }

    /// N5: over MCP (or from an agent, or a served store) a named anchor is limited to files
    /// `find` would show, and every refusal reads the same, so it tells nothing about which
    /// paths exist. The operator at a terminal keeps anchoring anything in the project.
    #[test]
    fn a_memory_only_caller_anchors_only_what_find_shows_and_learns_nothing_else() {
        let p = project(Actor::Mcp);
        std::fs::write(p.root.join(".env"), "SECRET=1\n").unwrap();
        std::fs::write(p.root.join(".cyberbrainignore"), "geheim/\n").unwrap();
        std::fs::create_dir_all(p.root.join("geheim")).unwrap();
        std::fs::write(p.root.join("geheim/x.rs"), "fn x() {}\n").unwrap();
        let mut said = Vec::new();
        for spec in [
            ".env",
            "geheim/x.rs",
            "src/missing.rs",
            "src",
            "../outside.rs",
        ] {
            let e = write(&p.app, "probe", "text", explicit(&[spec])).unwrap_err();
            said.push(e.to_string().replace(spec, "<spec>"));
            assert!(
                p.app.store().read("probe").is_err(),
                "{spec} wrote the note"
            );
        }
        said.dedup();
        assert_eq!(said.len(), 1, "one sentence for every refusal: {said:?}");
        write(&p.app, "ok", "Guard.", explicit(&["src/lib.rs:1"])).unwrap();
        // The operator at a terminal is not limited.
        let op = App::open(Some(p.app.root()), Actor::Operator).unwrap();
        write(&op, "env", "Env.", explicit(&[".env"])).unwrap();
    }

    /// `find` names the notes anchored to the file it found something in.
    #[test]
    fn find_names_notes_anchored_to_the_file() {
        let p = project(Actor::Operator);
        write(&p.app, "about-guard", "Guard.", explicit(&["src/lib.rs:1"])).unwrap();
        let r = p.app.find("guard", 10).unwrap();
        assert_eq!(r.anchored.len(), 1, "{:?}", r.anchored);
        assert_eq!(r.anchored[0].note, "about-guard");
        assert_eq!(r.anchored[0].drift, None);
        std::fs::write(p.root.join("src/lib.rs"), "pub fn guard() {}\n").unwrap();
        let r = p.app.find("guard", 10).unwrap();
        assert_eq!(r.anchored[0].drift, Some(DriftState::Changed));
    }

    /// Rings 0 and 1 stay the operator's: an agent may not re-anchor them either.
    #[test]
    fn an_agent_may_not_anchor_a_resident_note() {
        let p = project(Actor::Operator);
        p.app
            .write(WriteRequest {
                ring: Ring::Protocol,
                ..write_req("proto", "Protocol.")
            })
            .unwrap();
        let agent = App::open(Some(p.app.root()), Actor::Agent("t".into())).unwrap();
        let e = agent
            .anchor(AnchorRequest {
                name: "proto".into(),
                specs: vec!["src/lib.rs".into()],
                ..AnchorRequest::default()
            })
            .unwrap_err();
        assert!(matches!(e, Error::PolicyRefusal { .. }), "{e}");
    }

    fn write_req(name: &str, body: &str) -> WriteRequest {
        WriteRequest {
            ring: Ring::Knowledge,
            kind: NoteKind::Knowledge,
            name: name.into(),
            body: body.into(),
            tags: Vec::new(),
            bereich: None,
            retention: None,
            force: false,
            choice: None,
            expected_updated: None,
            supersedes: None,
            valid_from: None,
            invalid_at: None,
            arriving: None,
            anchors: Anchoring::default(),
            dry_run: false,
        }
    }
}
