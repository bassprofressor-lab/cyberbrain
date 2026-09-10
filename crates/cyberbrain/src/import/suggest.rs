//! Look at a folder and propose a plan for it.
//!
//! The reason this exists: `import` needs a TOML file, and writing one means knowing the
//! shape of a tree you have not read yet. That is the first thing between a company and a
//! store with anything in it, and an empty store is how this kind of tool dies — week one
//! it is empty, week three nobody asks it anything.
//!
//! What it does **not** do is import. It writes a plan to standard output for somebody to
//! read, edit and then run. A tool that guesses at somebody's filing and then acts on the
//! guess is worse than one that makes them write the file themselves: the second is
//! tedious, the first is confidently wrong.

use cyberbrain_core::{NoteKind, Result, Ring};
use std::collections::BTreeMap;
use std::path::Path;

/// One folder that would become a group.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposed {
    /// Path relative to the root, or empty for files lying loose at the top.
    pub dir: String,
    pub files: usize,
    /// Bytes of Markdown, for the "is this a page or a book" question.
    pub bytes: u64,
    /// What the folder name suggests, and why. Never certain, and the text says so.
    pub kind: NoteKind,
    /// A bereich, when the folder name looks like a department. Absent when it does not:
    /// inventing one would make notes shareable that nobody meant to share.
    pub bereich: Option<String>,
}

/// What a scan found, including what it could not use.
#[derive(Debug, Clone, PartialEq)]
pub struct Survey {
    pub root: String,
    pub groups: Vec<Proposed>,
    /// Extensions seen and skipped, with counts. Named rather than dropped, because "your
    /// PDFs were ignored" is the sentence somebody needs before they trust the result.
    pub skipped: BTreeMap<String, usize>,
    pub total_files: usize,
}

/// Folder names that say what a folder holds, in the two languages this is used in.
/// Deliberately short: a wrong guess costs an edit, but a long list of clever guesses makes
/// somebody trust the output instead of reading it.
fn kind_for(name: &str) -> NoteKind {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    // Stems rather than whole words: German makes plurals with an umlaut ("Vorfall" →
    // "Vorfälle"), and a folder is far more often named in the plural than the singular.
    // "vorfall" alone missed every folder anybody would actually create.
    if has(&["regel", "rule", "policy", "richtlinie", "vorgabe"]) {
        NoteKind::Decision
    } else if has(&["anleitung", "howto", "how-to", "guide", "handbuch", "manual"]) {
        NoteKind::Reference
    } else if has(&[
        "vorfall", "vorfäll", "vorfael", "incident", "stoerung", "störung", "postmortem",
        "panne",
    ]) {
        NoteKind::Bug
    } else if has(&["lehre", "lesson", "erfahrung"]) {
        NoteKind::Lesson
    } else {
        NoteKind::Knowledge
    }
}

/// A folder name becomes a bereich only when it reads like one: a single word, no spaces,
/// not a generic container — and not a word that already told us the *kind*.
///
/// That last one matters and was wrong first: a folder called "Anleitungen" is a shelf, not
/// a department, and proposing it as a bereich would offer a company's manuals to whichever
/// team happened to be granted "Anleitungen". A name can say what is inside or who it
/// belongs to; when it says the first, it is not saying the second.
fn bereich_for(name: &str) -> Option<String> {
    let n = name.trim();
    if n.is_empty() || n.contains(' ') || n.contains('/') {
        return None;
    }
    let generic = [
        "docs", "doc", "notes", "notizen", "wiki", "dokumente", "documents", "allgemein",
        "misc", "sonstiges", "temp", "tmp", "archiv", "archive",
    ];
    if generic.contains(&n.to_lowercase().as_str()) {
        return None;
    }
    // The name already told us the kind, so it is describing content and not ownership.
    if kind_for(n) != NoteKind::Knowledge {
        return None;
    }
    cyberbrain_core::frontmatter::validate_bereich(n).ok()?;
    Some(n.to_string())
}

/// Walk `root` one level deep and describe what is there.
///
/// One level, not all of them: a proposal somebody has to read is only useful while it is
/// short, and a deep tree turns into a hundred groups nobody checks.
pub fn survey(root: &Path) -> Result<Survey> {
    let mut groups: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let mut skipped: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;

    let mut walk = |dir: &Path, label: &str| -> Result<()> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(());
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                continue;
            }
            total += 1;
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("(no extension)")
                .to_lowercase();
            if ext != "md" && ext != "markdown" {
                *skipped.entry(ext).or_default() += 1;
                continue;
            }
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            let g = groups.entry(label.to_string()).or_insert((0, 0));
            g.0 += 1;
            g.1 += size;
        }
        Ok(())
    };

    walk(root, "")?;
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            walk(&p, name)?;
        }
    }

    let out = groups
        .into_iter()
        .filter(|(_, (files, _))| *files > 0)
        .map(|(dir, (files, bytes))| Proposed {
            kind: kind_for(&dir),
            bereich: if dir.is_empty() {
                None
            } else {
                bereich_for(&dir)
            },
            dir,
            files,
            bytes,
        })
        .collect();

    Ok(Survey {
        root: root.display().to_string(),
        groups: out,
        skipped,
        total_files: total,
    })
}

/// Render a survey as a plan file, with the guesses marked as guesses.
///
/// Everything lands in ring 2. Rings 0 and 1 are the operator's, and a folder full of files
/// nobody has read is not where invariants come from.
pub fn to_plan(s: &Survey) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Proposed by `cyberbrain import --suggest {}`.\n\
         #\n\
         # Read it before you run it. Every `kind` and every `bereich` below is a guess from\n\
         # a folder name — the tool has not read a single file. What it is sure of is how\n\
         # many files it found and which ones it cannot use.\n\
         #\n\
         # Everything is ring 2. Rings 0 and 1 are yours: an invariant does not come from a\n\
         # folder nobody has read yet.\n\
         #\n\
         # A group without a `bereich` is imported and never shared with a hub. That is the\n\
         # safe default; add one where a folder really belongs to a department.\n\n",
        s.root
    ));
    out.push_str("version = 1\n");
    out.push_str(&format!("root = {:?}\n", s.root));
    out.push_str("tags = [\"imported\"]\n\n");

    if !s.skipped.is_empty() {
        // Named extensions, never `**/*`. Every skip rule is evaluated before every group,
        // so a catch-all here swallows the very files the groups below are for — which is
        // exactly what the first version of this did: a plan that ran clean and imported
        // nothing.
        let parts: Vec<String> = s
            .skipped
            .iter()
            .map(|(ext, n)| format!("{n}× .{ext}"))
            .collect();
        let globs: Vec<String> = s
            .skipped
            .keys()
            .filter(|e| *e != "(no extension)")
            .map(|e| format!("\"**/*.{e}\""))
            .collect();
        out.push_str(&format!("# Found and left alone: {}\n", parts.join(", ")));
        if !globs.is_empty() {
            out.push_str(&format!(
                "[[skip]]\npaths = [{}]\nreason = \"not Markdown\"\n\n",
                globs.join(", ")
            ));
        }
    }

    for g in &s.groups {
        let label = if g.dir.is_empty() { "top-level" } else { &g.dir };
        let paths = if g.dir.is_empty() {
            "\"*.md\", \"*.markdown\"".to_string()
        } else {
            format!("\"{}/*.md\", \"{}/*.markdown\"", g.dir, g.dir)
        };
        out.push_str(&format!(
            "# {} file(s), {} KB\n[[group]]\nname = {:?}\npaths = [{}]\nring = 2\nkind = {:?}\n",
            g.files,
            g.bytes / 1024,
            label,
            paths,
            kind_str(g.kind),
        ));
        // A note name is unique across the store, and the same file name in two folders is
        // the normal case in a real filing system, not an edge one — "notes.md" in six
        // department folders is six collisions and an import that stops. The folder goes in
        // front of the name so the second one has somewhere to be.
        if !g.dir.is_empty() {
            // Both, and on purpose. The template gives a readable name where the folder is
            // available; `collisions = "hash"` is the net, because a plan that stops on the
            // second "notes.md" hands somebody a failure instead of a store. Each renamed
            // item is still listed in the report, so nothing is quietly mangled.
            out.push_str("note_name = \"{dir}-{stem}\"\ncollisions = \"hash\"\n");
        }
        match &g.bereich {
            Some(b) => out.push_str(&format!(
                "# Guessed from the folder name. Wrong here means these notes reach the wrong\n\
                 # department, so check it.\nbereich = {b:?}\n"
            )),
            None => out.push_str(
                "# No bereich: these stay on this machine. Add one to share them.\n\
                 # bereich = \"…\"\n",
            ),
        }
        out.push('\n');
    }

    if s.groups.is_empty() {
        out.push_str("# No Markdown found. Nothing to propose.\n");
    }
    out
}

fn kind_str(k: NoteKind) -> String {
    serde_json::to_value(k)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "knowledge".into())
}

/// The one-paragraph summary printed beside the plan, for somebody who wants the number
/// before the file.
pub fn summary(s: &Survey) -> String {
    let files: usize = s.groups.iter().map(|g| g.files).sum();
    let with_bereich = s.groups.iter().filter(|g| g.bereich.is_some()).count();
    let mut t = format!(
        "{} Markdown file(s) in {} group(s), out of {} file(s) in {}.\n",
        files,
        s.groups.len(),
        s.total_files,
        s.root
    );
    if !s.skipped.is_empty() {
        let parts: Vec<String> = s
            .skipped
            .iter()
            .map(|(ext, n)| format!("{n}× .{ext}"))
            .collect();
        t.push_str(&format!("Not Markdown and left alone: {}.\n", parts.join(", ")));
    }
    t.push_str(&format!(
        "{with_bereich} group(s) got a suggested bereich from the folder name; the rest stay \
         on this machine until you give them one.\n\
         Nothing has been imported. Save the plan, read it, then run \
         `cyberbrain import --plan <file>`.\n"
    ));
    t
}

const _: Option<Ring> = None;

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder name says either what is inside or who it belongs to, never both. Getting
    /// this wrong the first time proposed `bereich = "Anleitungen"`, which would have
    /// offered a company's manuals to whichever team was granted that name.
    #[test]
    fn a_kind_name_is_not_a_department() {
        for n in ["Anleitungen", "Regeln", "Vorfaelle", "howto", "policy"] {
            assert_eq!(bereich_for(n), None, "{n} was proposed as a bereich");
        }
        for n in ["Disposition", "HR", "Vertrieb"] {
            assert_eq!(bereich_for(n).as_deref(), Some(n), "{n} should be one");
        }
    }

    /// Generic containers name nothing, and a bereich that means nothing is worse than none:
    /// it makes notes shareable that nobody decided to share.
    #[test]
    fn a_generic_folder_gets_no_bereich() {
        for n in ["docs", "Notizen", "wiki", "misc", "Archiv", "with space"] {
            assert_eq!(bereich_for(n), None, "{n}");
        }
    }

    #[test]
    fn folder_names_suggest_a_kind() {
        assert_eq!(kind_for("Regeln"), NoteKind::Decision);
        assert_eq!(kind_for("Anleitungen"), NoteKind::Reference);
        assert_eq!(kind_for("Vorfaelle"), NoteKind::Bug);
        assert_eq!(kind_for("Disposition"), NoteKind::Knowledge);
    }

    /// The failure the first version shipped with: a `**/*` skip rule is evaluated before
    /// every group, so the plan ran clean and imported nothing. Only real extensions here.
    #[test]
    fn the_skip_rule_never_swallows_everything() {
        let s = Survey {
            root: "/x".into(),
            groups: vec![Proposed {
                dir: "Disposition".into(),
                files: 2,
                bytes: 100,
                kind: NoteKind::Knowledge,
                bereich: Some("Disposition".into()),
            }],
            skipped: [("pdf".to_string(), 1)].into_iter().collect(),
            total_files: 3,
        };
        let plan = to_plan(&s);
        assert!(!plan.contains(r#"paths = ["**/*"]"#), "catch-all skip is back");
        assert!(plan.contains(r#""**/*.pdf""#));
    }

    /// Everything lands in ring 2. A folder nobody has read is not where invariants live.
    #[test]
    fn a_proposal_never_reaches_the_resident_rings() {
        let s = Survey {
            root: "/x".into(),
            groups: vec![Proposed {
                dir: "Regeln".into(),
                files: 1,
                bytes: 10,
                kind: NoteKind::Decision,
                bereich: None,
            }],
            skipped: Default::default(),
            total_files: 1,
        };
        let plan = to_plan(&s);
        assert!(plan.contains("ring = 2"));
        assert!(!plan.contains("ring = 0"));
        assert!(!plan.contains("ring = 1"));
    }
}
