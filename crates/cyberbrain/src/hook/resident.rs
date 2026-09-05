//! Rings 0 and 1: read, rendered with citations, fingerprinted.
//!
//! The resident rings are read straight from `notes/r0` and `notes/r1` with `read_dir`,
//! not through `Store::list`, which walks all five ring directories and so costs what the
//! whole store costs. Two consequences, both deliberate:
//!
//! - The cost is bounded by the resident cap (§3.2), not by the store. That is what makes
//!   the change check affordable on every prompt.
//! - `.cyberbrainignore` is not consulted. An ignore rule that hides an operator invariant
//!   from the agent would be a silent hole; here every `.md` in r0/r1 is either injected
//!   or reported as unreadable.
//!
//! Every block carries its citation, computed by `blocks_of` in core, the same function
//! the index uses, so an id the agent quotes from the injected text resolves with
//! `cyberbrain recall --id`.

use super::session::ResidentMark;
use cyberbrain_core::blocks::{MAX_BLOCK_TOKENS, approx_tokens, blocks_of};
use cyberbrain_core::{Block, Fingerprint, Note, Ring, Store};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct ResidentNote {
    pub note: Note,
    pub blocks: Vec<Block>,
    pub fingerprint: Fingerprint,
}

impl ResidentNote {
    pub fn key(&self) -> String {
        format!("{}/{}", self.note.front.ring, self.note.front.name)
    }

    pub fn mark(&self) -> ResidentMark {
        ResidentMark {
            ring: self.note.front.ring.as_u8(),
            path: self.note.path.display().to_string(),
            hash: self.fingerprint.hash_hex(),
            size: self.fingerprint.size,
            updated: self.note.front.updated.to_string(),
        }
    }
}

#[derive(Default)]
pub struct Resident {
    /// Ring 0 first, then ring 1, each sorted by name.
    pub notes: Vec<ResidentNote>,
    /// Files in r0/r1 that could not be read or parsed, with why. Never dropped quietly:
    /// an invariant the agent cannot see is the loudest thing this hook has to say.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Non-`.md`, non-hidden entries that were passed over.
    pub other_files: usize,
    /// Ring directories that do not exist.
    pub missing_dirs: Vec<Ring>,
    /// `approx_tokens` over every readable body: the figure the cap is measured in.
    pub tokens: usize,
}

pub fn read(store: &Store) -> Resident {
    let mut r = Resident::default();
    for ring in [Ring::Invariant, Ring::Protocol] {
        let dir = store.ring_dir(ring);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                r.missing_dirs.push(ring);
                continue;
            }
            Err(e) => {
                r.unreadable.push((dir, e.to_string()));
                continue;
            }
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                r.other_files += 1;
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                r.other_files += 1;
                continue;
            }
            paths.push(path);
        }
        paths.sort();
        for path in paths {
            let fingerprint = match Fingerprint::of(&path) {
                Ok(f) => f,
                Err(e) => {
                    r.unreadable.push((path, e.to_string()));
                    continue;
                }
            };
            match store.read_path(&path) {
                Ok(note) => {
                    if note.front.ring != ring {
                        // read_path already rejects this, but the cost of stating it is
                        // one comparison.
                        r.unreadable.push((
                            path,
                            format!(
                                "frontmatter says ring {} but the file is in {ring}",
                                note.front.ring
                            ),
                        ));
                        continue;
                    }
                    r.tokens += approx_tokens(&note.body) as usize;
                    let (blocks, _oversized) = blocks_of(&note, MAX_BLOCK_TOKENS);
                    r.notes.push(ResidentNote {
                        note,
                        blocks,
                        fingerprint,
                    });
                }
                Err(e) => r.unreadable.push((path, e.to_string())),
            }
        }
    }
    r
}

/// Everything the marks record, keyed like [`ResidentNote::key`].
pub fn marks(r: &Resident) -> BTreeMap<String, ResidentMark> {
    r.notes.iter().map(|n| (n.key(), n.mark())).collect()
}

/// What changed between the marks of an earlier injection and the rings now.
#[derive(Default)]
pub struct Changes<'a> {
    pub changed: Vec<&'a ResidentNote>,
    pub added: Vec<&'a ResidentNote>,
    /// Keys that were injected and are gone or unreadable now.
    pub removed: Vec<String>,
}

impl Changes<'_> {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

pub fn diff<'a>(before: &BTreeMap<String, ResidentMark>, now: &'a Resident) -> Changes<'a> {
    let mut c = Changes::default();
    let mut seen = std::collections::BTreeSet::new();
    for n in &now.notes {
        let key = n.key();
        seen.insert(key.clone());
        match before.get(&key) {
            None => c.added.push(n),
            Some(m) if m.hash != n.fingerprint.hash_hex() || m.size != n.fingerprint.size => {
                c.changed.push(n)
            }
            Some(_) => {}
        }
    }
    for key in before.keys() {
        if !seen.contains(key) {
            c.removed.push(key.clone());
        }
    }
    c
}

// ---------------------------------------------------------------------------------------
// Rendering

pub fn ring_title(ring: Ring) -> &'static str {
    match ring {
        Ring::Invariant => "Ring 0: operator invariants (override everything below)",
        Ring::Protocol => "Ring 1: operating protocol and handoff state (override rings 2+)",
        Ring::Knowledge => "Ring 2: curated project knowledge",
        Ring::Session => "Ring 3: session records",
        Ring::External => "Ring 4: external, unverified",
    }
}

/// One note, every block prefixed with its citation.
pub fn render_note(n: &ResidentNote, out: &mut String) {
    let f = &n.note.front;
    out.push_str(&format!("### {} `{}`", f.ring, f.name));
    let mut meta = vec![format!("kind: {}", kind_name(f.kind))];
    meta.push(format!("updated: {}", f.updated.strftime("%Y-%m-%d")));
    if !f.tags.is_empty() {
        meta.push(format!("tags: {}", f.tags.join(", ")));
    }
    out.push_str(&format!(" ({})\n", meta.join(", ")));
    if n.blocks.is_empty() {
        out.push_str("(empty body)\n\n");
        return;
    }
    for b in &n.blocks {
        out.push_str(&format!("[{}]\n{}\n\n", b.citation, b.text.trim_end()));
    }
}

/// Rings 0 and 1 in full, with what could not be read.
pub fn render_rings(r: &Resident, out: &mut String) {
    for ring in [Ring::Invariant, Ring::Protocol] {
        out.push_str(&format!("## {}\n\n", ring_title(ring)));
        let notes: Vec<&ResidentNote> = r
            .notes
            .iter()
            .filter(|n| n.note.front.ring == ring)
            .collect();
        if r.missing_dirs.contains(&ring) {
            out.push_str(&format!(
                "(the directory notes/{} does not exist; `cyberbrain init` creates it)\n\n",
                ring.dir()
            ));
        } else if notes.is_empty() {
            out.push_str(&format!("(ring {} is empty)\n\n", ring.as_u8()));
        }
        for n in notes {
            render_note(n, out);
        }
    }
    if !r.unreadable.is_empty() {
        out.push_str("## Resident notes that could NOT be injected\n\n");
        out.push_str(
            "These files sit in ring 0 or 1 and failed to read or parse. Whatever they say \
             is not in your context; tell the operator.\n\n",
        );
        for (p, why) in &r.unreadable {
            out.push_str(&format!("- {}: {why}\n", p.display()));
        }
        out.push('\n');
    }
}

pub fn kind_name(k: cyberbrain_core::NoteKind) -> String {
    match serde_json::to_value(k) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{k:?}").to_lowercase(),
    }
}
