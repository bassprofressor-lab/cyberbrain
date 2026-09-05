//! Rendering a path for a human or for a machine that will compare it.
//!
//! A `PathBuf` prints with the platform's separator, which is right when you hand it to a
//! shell and wrong when the string ends up in a report. Cyberbrain's output gets pasted
//! into notes, diffed between runs and compared across machines: a note recorded on
//! Windows should not disagree with the same note recorded on Linux over a backslash.
//!
//! So: **paths that are opened stay `PathBuf`, paths that are rendered go through here.**
//! `cyberbrain-code` already promised forward slashes in its reports; this makes the rest
//! of the tool keep the same promise instead of two subsystems spelling one path two ways.
//!
//! Only the separator changes. A drive letter, a UNC prefix and every other character are
//! left exactly as they are — this is for display, not for constructing a path to open.

use std::fmt;
use std::path::Path;

/// The path with forward slashes, whatever the platform uses.
pub fn slash(p: &Path) -> String {
    let s = p.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        s.into_owned()
    } else {
        s.replace(std::path::MAIN_SEPARATOR, "/")
    }
}

/// `Display` wrapper for [`slash`], so a path can go straight into a `format!` without an
/// intermediate `String`: `format!("{}", Slash(&p))`.
pub struct Slash<'a>(pub &'a Path);

impl fmt::Display for Slash<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0.to_string_lossy();
        if std::path::MAIN_SEPARATOR == '/' {
            f.write_str(&s)
        } else {
            f.write_str(&s.replace(std::path::MAIN_SEPARATOR, "/"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn a_relative_path_renders_with_forward_slashes() {
        let p: PathBuf = ["notes", "r2", "a-note.md"].iter().collect();
        assert_eq!(slash(&p), "notes/r2/a-note.md");
        assert_eq!(Slash(&p).to_string(), "notes/r2/a-note.md");
    }

    /// The point of the type: the same logical path renders identically no matter which
    /// platform built it, so a report written on one machine matches one written on
    /// another.
    #[test]
    fn the_platform_separator_does_not_reach_the_output() {
        let p: PathBuf = ["a", "b", "c"].iter().collect();
        let out = slash(&p);
        assert!(!out.contains('\\'), "{out}");
        assert_eq!(out, "a/b/c");
    }

    #[test]
    fn only_the_separator_changes() {
        // Spaces, dots, unicode and case survive untouched; this is display, not sanitising.
        let p: PathBuf = ["Ordner mit Leerzeichen", "Ä.md"].iter().collect();
        assert_eq!(slash(&p), "Ordner mit Leerzeichen/Ä.md");
    }
}
