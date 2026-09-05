//! Source discovery and rule matching.
//!
//! Two counts of the same tree are taken on purpose. [`list_files`] is the walk the
//! importer works from; [`count_files_independently`] is a second, trivially simple walk
//! that shares nothing with it but `std`. The report compares the two. A walker that
//! quietly stops listing a class of files (the failure this importer exists to prevent)
//! then shows up as a mismatch instead of a report that balances over fewer files.

use super::plan::ImportPlan;
use cyberbrain_core::{Error, Result, Slash};
use std::path::{Path, PathBuf};

/// A file under the source root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Relative to the root, always `/`-separated (SPEC §14.8: no platform-dependent
    /// normalisation leaks into names or tags).
    pub rel: String,
    pub abs: PathBuf,
}

/// Every file under `root`, sorted by relative path. Directories are entered, symlinks
/// are listed as files and not followed as directories.
pub fn list_files(root: &Path) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Found>) -> Result<()> {
    let io = |source: std::io::Error| Error::Io {
        path: dir.to_path_buf(),
        source,
    };
    for entry in std::fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let path = entry.path();
        let ft = entry.file_type().map_err(io)?;
        if ft.is_dir() {
            walk(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| {
                    Error::Config(format!("{} is not under {}", Slash(&path), Slash(root)))
                })?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Found { rel, abs: path });
        }
    }
    Ok(())
}

/// The second tally. Deliberately not shared with [`list_files`].
pub fn count_files_independently(root: &Path) -> Result<usize> {
    fn go(dir: &Path) -> std::io::Result<usize> {
        let mut n = 0;
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                n += go(&e.path())?;
            } else {
                n += 1;
            }
        }
        Ok(n)
    }
    go(root).map_err(|e| Error::Io {
        path: root.to_path_buf(),
        source: e,
    })
}

/// Match a plan pattern against a relative path. A pattern without `/` is matched
/// against the file name; one with `/` against the whole relative path.
pub fn glob_match(pattern: &str, rel: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    if pattern.contains('/') {
        segments_match(
            &pattern.split('/').collect::<Vec<_>>(),
            &rel.split('/').collect::<Vec<_>>(),
        )
    } else {
        let name = rel.rsplit('/').next().unwrap_or(rel);
        segment_match(pattern, name)
    }
}

fn segments_match(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => {
            // Zero or more segments.
            (0..=path.len()).any(|k| segments_match(rest, &path[k..]))
        }
        Some((first, rest)) => match path.split_first() {
            Some((seg, path_rest)) => segment_match(first, seg) && segments_match(rest, path_rest),
            None => false,
        },
    }
}

/// `*` and `?` within one segment, byte-wise on chars.
fn segment_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|k| go(rest, &t[k..])),
            Some(('?', rest)) => !t.is_empty() && go(rest, &t[1..]),
            Some((c, rest)) => t.first() == Some(c) && go(rest, &t[1..]),
        }
    }
    go(&p, &t)
}

/// Which rule a file falls under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Skip(usize),
    Group(usize),
    Unmapped,
}

/// Skip rules first, then groups, first match wins.
pub fn classify(plan: &ImportPlan, rel: &str) -> Class {
    for (i, s) in plan.skip.iter().enumerate() {
        if s.paths.iter().any(|p| glob_match(p, rel)) {
            return Class::Skip(i);
        }
    }
    for (i, g) in plan.groups.iter().enumerate() {
        if g.paths.iter().any(|p| glob_match(p, rel)) {
            return Class::Group(i);
        }
    }
    Class::Unmapped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob_match("*.md", "a.md"));
        assert!(glob_match("*.md", "deep/er/a.md"));
        assert!(!glob_match("*.md", "a.mdx"));
        assert!(glob_match("PREREG_*.md", "PREREG_va_2026-08-25.md"));
        assert!(!glob_match("PREREG_*.md", "prereg_x.md"));
        assert!(glob_match("*.bak-*", "STATUS.md.bak-20260903-2025"));
        assert!(glob_match("backups/**", "backups/x"));
        assert!(glob_match("backups/**", "backups/a/b/c.md"));
        assert!(!glob_match("backups/**", "not/backups/x"));
        assert!(glob_match("proposals/*.md", "proposals/x.md"));
        assert!(!glob_match("proposals/*.md", "proposals/sub/x.md"));
        assert!(glob_match("**/x.md", "x.md"));
        assert!(glob_match("**/x.md", "a/b/x.md"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("a?c", "ac"));
    }

    #[test]
    fn both_counts_agree_and_symlinked_dirs_are_not_entered() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        std::fs::create_dir_all(r.join("a/b")).unwrap();
        std::fs::write(r.join("a/b/one.md"), "x").unwrap();
        std::fs::write(r.join("two.md"), "x").unwrap();
        std::fs::write(r.join(".hidden"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(r.join("a"), r.join("link-to-a")).unwrap();
        let listed = list_files(r).unwrap();
        let rels: Vec<&str> = listed.iter().map(|f| f.rel.as_str()).collect();
        #[cfg(unix)]
        assert_eq!(rels, [".hidden", "a/b/one.md", "link-to-a", "two.md"]);
        #[cfg(not(unix))]
        assert_eq!(rels, [".hidden", "a/b/one.md", "two.md"]);
        assert_eq!(count_files_independently(r).unwrap(), listed.len());
    }
}
