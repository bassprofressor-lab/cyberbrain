//! Anchors: a note's claim about code, tied to the bytes it was written against.
//!
//! A note that says "`exchange.py:120` refuses `carry`" is true of one version of that file.
//! When the file changes, the note may still be right or may have gone stale, and nothing
//! said which. An anchor records the file (relative to the project), optionally the lines,
//! and the SHA-256 of the whole file at the moment the note was written or re-anchored.
//! Recall compares that hash with the file as it is now and says "code changed since …"
//! when they differ; it does not judge whether the note is still right. That is the reader's
//! job, and the mark tells them it is due.
//!
//! This module holds only what is pure: the type in the head, its validation, and the
//! detection of `path:line` references in a body. Hashing files and deciding what the
//! project root is belong to the application, which knows where the store sits.

use serde::{Deserialize, Serialize};

/// The longest anchored path accepted. Longer is not a path anybody typed.
const MAX_PATH: usize = 512;
/// At most this many anchors are taken from a body by detection, so a note that pastes a
/// stack trace does not hash forty files.
pub const MAX_DETECTED: usize = 16;

/// One anchored file in a note's head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// Relative to the project root, `/`-separated, never `..`.
    pub path: String,
    /// `120` or `120-140`, when the note is about part of the file. Informational: the hash
    /// is over the whole file, because line numbers move when anything above them changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<String>,
    /// SHA-256 of the file's bytes when it was anchored, lowercase hex.
    pub sha256: String,
    /// When it was anchored.
    pub at: jiff::Timestamp,
}

impl Anchor {
    /// `path` or `path:lines`, the way a reader would cite it.
    pub fn label(&self) -> String {
        match &self.lines {
            Some(l) => format!("{}:{l}", self.path),
            None => self.path.clone(),
        }
    }
}

/// Checks on an anchor that serde cannot express. Shared by parse and render, like every
/// other head check, so no write path can put an anchor on disk the next read refuses.
pub fn validate(a: &Anchor) -> Result<(), String> {
    validate_path(&a.path).map_err(|why| format!("path `{}`: {why}", a.path))?;
    if let Some(l) = &a.lines {
        validate_lines(l).map_err(|why| format!("lines `{l}` of {}: {why}", a.path))?;
    }
    if a.sha256.len() != 64
        || !a
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(format!(
            "sha256 of {} must be 64 lowercase hex digits",
            a.path
        ));
    }
    Ok(())
}

/// A project-relative path: no root, no drive, no `.` or `..`, no empty component.
pub fn validate_path(p: &str) -> Result<(), &'static str> {
    if p.is_empty() {
        return Err("must not be empty");
    }
    if p.len() > MAX_PATH {
        return Err("is longer than 512 bytes");
    }
    if p.chars().any(char::is_control) {
        return Err("must not contain control characters");
    }
    if p.contains('\\') {
        return Err("must use `/` as the separator");
    }
    if p.starts_with('/') || p.as_bytes().get(1) == Some(&b':') {
        return Err("must be relative to the project");
    }
    if p.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        return Err("must not contain empty, `.` or `..` components");
    }
    Ok(())
}

/// `N` or `N-M`, 1-based, `M >= N`.
pub fn validate_lines(l: &str) -> Result<(), &'static str> {
    let (a, b) = match l.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (l, None),
    };
    let num = |s: &str| -> Option<u32> {
        (!s.is_empty() && s.len() <= 9 && s.bytes().all(|c| c.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
            .filter(|n| *n >= 1)
    };
    let start = num(a).ok_or("must be a line number from 1, or a range N-M")?;
    if let Some(b) = b {
        let end = num(b).ok_or("must be a line number from 1, or a range N-M")?;
        if end < start {
            return Err("the range ends before it starts");
        }
    }
    Ok(())
}

/// Split `path:lines` into its parts. A suffix that is not a line spec stays in the path,
/// so `notes:draft` is a path, and `src/a.rs:12:5` (line and column) keeps the line.
pub fn split_spec(spec: &str) -> (&str, Option<&str>) {
    let spec = spec.trim();
    // `path:line:column`, as compilers print it: the column is dropped.
    if let Some((head, col)) = spec.rsplit_once(':')
        && !col.is_empty()
        && col.bytes().all(|b| b.is_ascii_digit())
        && let Some((path, line)) = head.rsplit_once(':')
        && validate_lines(line).is_ok()
        && !path.is_empty()
    {
        return (path, Some(line));
    }
    match spec.rsplit_once(':') {
        Some((path, lines)) if !path.is_empty() && validate_lines(lines).is_ok() => {
            (path, Some(lines))
        }
        _ => (spec, None),
    }
}

/// References to code in a body: `dir/file.ext:120`, `file.ext:12-30`, `a/b.rs:7:3`.
///
/// Only references **with a line** count. A path named in passing ("see `backend/main.py`")
/// is context, not a claim about particular code, and anchoring every such mention would
/// mark notes stale whenever a busy file changes. To anchor a whole file, say so with
/// `--anchor`. Whether a candidate exists, and lies inside the project, is decided by the
/// caller; this only finds the shape. URLs are skipped.
pub fn detect(body: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let is_delim = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '`' | '(' | ')' | '[' | ']' | '<' | '>' | '"' | '\'' | ',' | ';' | '|' | '*'
            )
    };
    for raw in body.split(is_delim) {
        if out.len() >= MAX_DETECTED {
            break;
        }
        if raw.contains("://") {
            continue;
        }
        let token = raw.trim_end_matches(['.', ':', '!', '?']);
        let (path, Some(lines)) = split_spec(token) else {
            continue;
        };
        let path = path.strip_prefix("./").unwrap_or(path);
        if !looks_like_file(path) {
            continue;
        }
        let entry = (path.to_string(), lines.to_string());
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// A path whose last component has an extension of letters and digits, and which carries
/// nothing that a source path does not: `exchange.py`, `crates/x/src/lib.rs`. Not `12:30`
/// (a time), not `r2-abc:3`, not `v0.10:4`.
fn looks_like_file(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH {
        return false;
    }
    if !path
        .chars()
        .all(|c| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '+' | '@'))
    {
        return false;
    }
    let last = path.rsplit('/').next().unwrap_or(path);
    match last.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && (1..=10).contains(&ext.len())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_takes_references_with_a_line_and_nothing_else() {
        let body = "The guard sits in `backend/routes/exchange.py:120` and \
                    (crates/a/src/lib.rs:10-20). See backend/main.py for context, \
                    https://example.com/x.rs:3 is a URL, 12:30 is a time, \
                    and main.rs:7:3, was printed by rustc. Twice: main.rs:7.";
        let found = detect(body);
        assert_eq!(
            found,
            vec![
                ("backend/routes/exchange.py".into(), "120".into()),
                ("crates/a/src/lib.rs".into(), "10-20".into()),
                ("main.rs".into(), "7".into()),
            ]
        );
    }

    #[test]
    fn detection_is_capped() {
        let body: String = (0..40).map(|i| format!("f{i}.rs:1 ")).collect();
        assert_eq!(detect(&body).len(), MAX_DETECTED);
    }

    #[test]
    fn specs_split_into_path_and_lines() {
        assert_eq!(split_spec("a/b.rs:12"), ("a/b.rs", Some("12")));
        assert_eq!(split_spec("a/b.rs:12-40"), ("a/b.rs", Some("12-40")));
        assert_eq!(split_spec("a/b.rs:12:5"), ("a/b.rs", Some("12")));
        assert_eq!(split_spec("a/b.rs"), ("a/b.rs", None));
        assert_eq!(split_spec("a/b.rs:40-12"), ("a/b.rs:40-12", None));
        assert_eq!(split_spec("notes:draft"), ("notes:draft", None));
    }

    #[test]
    fn paths_must_stay_inside_the_project() {
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "./a",
            "C:/x",
            "a\\b",
        ] {
            assert!(validate_path(bad).is_err(), "{bad:?} was accepted");
        }
        assert!(validate_path("crates/x/src/lib.rs").is_ok());
    }

    #[test]
    fn an_anchor_needs_a_real_hash() {
        let mut a = Anchor {
            path: "a.rs".into(),
            lines: Some("3".into()),
            sha256: "0".repeat(64),
            at: jiff::Timestamp::UNIX_EPOCH,
        };
        assert!(validate(&a).is_ok());
        a.sha256 = "A".repeat(64);
        assert!(validate(&a).is_err());
        a.sha256 = "0".repeat(63);
        assert!(validate(&a).is_err());
        a.sha256 = "0".repeat(64);
        a.lines = Some("0".into());
        assert!(validate(&a).is_err());
    }

    const HEAD: &str = "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: guard\nring: 2\nkind: knowledge\n\
created: 2026-11-13T10:00:00Z\nupdated: 2026-11-13T10:00:00Z\npii: none\n";

    /// A head with anchors reads and writes back byte for byte; one without them is what it
    /// always was, so every note written before anchors stays as it is.
    #[test]
    fn anchors_round_trip_in_the_head_and_their_absence_changes_nothing() {
        let sha = "ab".repeat(32);
        let text = format!(
            "{HEAD}anchors:\n- path: src/lib.rs\n  lines: 1-3\n  sha256: {sha}\n  at: 2026-11-13T10:00:00Z\n---\n\nBody.\n"
        );
        let p = crate::frontmatter::parse(std::path::Path::new("guard.md"), &text).unwrap();
        assert_eq!(p.front.anchors.len(), 1);
        assert_eq!(p.front.anchors[0].label(), "src/lib.rs:1-3");
        assert_eq!(crate::frontmatter::render(&p.front, p.body).unwrap(), text);

        let plain = format!("{HEAD}---\n\nBody.\n");
        let p = crate::frontmatter::parse(std::path::Path::new("guard.md"), &plain).unwrap();
        assert!(p.front.anchors.is_empty());
        assert_eq!(crate::frontmatter::render(&p.front, p.body).unwrap(), plain);
    }

    /// A hand-edited anchor that leaves the project, or carries no real hash, is refused
    /// with the field named.
    #[test]
    fn a_bad_anchor_in_the_head_is_refused() {
        for bad in [
            "- path: ../etc/passwd\n  sha256: {SHA}\n  at: 2026-11-13T10:00:00Z\n",
            "- path: src/lib.rs\n  sha256: nothex\n  at: 2026-11-13T10:00:00Z\n",
            "- path: src/lib.rs\n  sha256: {SHA}\n  at: 2026-11-13T10:00:00Z\n  extra: 1\n",
        ] {
            let text = format!(
                "{HEAD}anchors:\n{}---\n\nBody.\n",
                bad.replace("{SHA}", &"ab".repeat(32))
            );
            let e = crate::frontmatter::parse(std::path::Path::new("guard.md"), &text)
                .unwrap_err()
                .to_string();
            assert!(e.contains("anchor"), "{bad}: {e}");
        }
    }
}
