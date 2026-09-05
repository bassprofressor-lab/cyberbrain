//! The tree walk: which files reach an extractor, and an account of every one that does
//! not.
//!
//! SPEC §14.5: a silent early return is a bug. Every path this walk declines is counted
//! under the reason it was declined, and a directory declined by an ignore rule counts
//! once as "not entered" — the files under it were never looked at, and a count that
//! claimed to know how many there were would be a lie.
//!
//! Ignore rules, most specific first, first match wins:
//! 1. `.cyberbrainignore` in the current directory or any parent up to the root
//!    (gitignore syntax via the `ignore` crate, nearer files override farther ones),
//! 2. `.gitignore` likewise, whether or not the tree is a git repository — `target/` and
//!    `node_modules/` are what an operator means by it either way.
//!
//! Before either: the store itself (`.cyberbrain/`, whose notes are `recall`'s domain,
//! not `find`'s), hidden entries, symlinks (never followed: a link out of the tree or a
//! loop is not worth the risk), lockfiles by name, files with no extractor, files over
//! the size cap, and files that hold a NUL byte in their first 8 KiB.
//!
//! The walk is done by hand over `read_dir` rather than with the crate's `WalkBuilder`,
//! because the builder never yields the entries it skips and the whole point here is to
//! report them. Entries are visited in sorted order so results are stable across runs.

use crate::{FindOptions, IGNORE_FILE, Language, Skipped};
use cyberbrain_core::{Error, Result, Slash};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::fs;
use std::path::{Path, PathBuf};

/// Lockfiles are machine-written, huge, and every key in them is a package name; a hit
/// inside one is never what an agent asked for.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "npm-shrinkwrap.json",
    "poetry.lock",
    "Pipfile.lock",
    "uv.lock",
    "composer.lock",
    "Gemfile.lock",
    "go.sum",
    "flake.lock",
    "bun.lock",
    "bun.lockb",
];

/// Bytes inspected for a NUL to call a file binary.
const BINARY_PROBE: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Cyberbrain,
    Git,
}

struct Matcher {
    gi: Gitignore,
    source: Source,
}

/// What the walk hands to the caller per readable source file.
pub(crate) struct File<'a> {
    /// Root-relative, forward slashes.
    pub rel: &'a str,
    pub language: Language,
    pub text: &'a str,
}

pub(crate) struct Walk<'o> {
    root: PathBuf,
    opts: &'o FindOptions,
    excludes: Vec<PathBuf>,
    pub skipped: Skipped,
    pub ignore_files: Vec<String>,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
}

impl<'o> Walk<'o> {
    pub(crate) fn new(root: &Path, opts: &'o FindOptions) -> Result<Self> {
        let root = absolute(root)?;
        let md = fs::metadata(&root).map_err(|e| Error::Io {
            path: root.clone(),
            source: e,
        })?;
        if !md.is_dir() {
            return Err(Error::Config(format!(
                "{} is not a directory; find scans a project tree",
                Slash(&root)
            )));
        }
        let excludes = opts
            .exclude
            .iter()
            .map(|p| absolute(p))
            .collect::<Result<Vec<_>>>()?;
        Ok(Walk {
            root,
            opts,
            excludes,
            skipped: Skipped::default(),
            ignore_files: Vec::new(),
            files_scanned: 0,
            bytes_scanned: 0,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Walk the tree, calling `f` for every file that reaches an extractor.
    pub(crate) fn run(&mut self, f: &mut dyn FnMut(File<'_>)) -> Result<()> {
        let mut matchers: Vec<Matcher> = Vec::new();
        let root = self.root.clone();
        self.dir(&root, "", &mut matchers, f)
    }

    fn load_matchers(
        &mut self,
        dir: &Path,
        rel: &str,
        matchers: &mut Vec<Matcher>,
    ) -> Result<usize> {
        let mut added = 0;
        // Git first, cyberbrain second: the later one is checked first.
        for (file, source) in [
            (".gitignore", Source::Git),
            (IGNORE_FILE, Source::Cyberbrain),
        ] {
            if source == Source::Git && !self.opts.honour_gitignore {
                continue;
            }
            let path = dir.join(file);
            if !path.is_file() {
                continue;
            }
            let mut b = GitignoreBuilder::new(dir);
            if let Some(e) = b.add(&path) {
                return Err(Error::Config(format!(
                    "{}: cannot be read as an ignore file: {e}",
                    Slash(&path)
                )));
            }
            let gi = b.build().map_err(|e| {
                Error::Config(format!("{}: invalid ignore pattern: {e}", Slash(&path)))
            })?;
            matchers.push(Matcher { gi, source });
            added += 1;
            self.ignore_files.push(join_rel(rel, file));
        }
        Ok(added)
    }

    /// The first matcher, nearest directory first, with an opinion about `path`.
    fn ignored_by(matchers: &[Matcher], path: &Path, is_dir: bool) -> Option<Source> {
        for m in matchers.iter().rev() {
            let r = m.gi.matched(path, is_dir);
            if r.is_ignore() {
                return Some(m.source);
            }
            if r.is_whitelist() {
                return None;
            }
        }
        None
    }

    fn dir(
        &mut self,
        dir: &Path,
        rel: &str,
        matchers: &mut Vec<Matcher>,
        f: &mut dyn FnMut(File<'_>),
    ) -> Result<()> {
        let added = self.load_matchers(dir, rel, matchers)?;

        let mut entries: Vec<(String, fs::DirEntry)> = match fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e))
                .collect(),
            Err(e) => {
                self.skipped.unreadable.push((
                    if rel.is_empty() {
                        ".".into()
                    } else {
                        rel.to_string()
                    },
                    e.to_string(),
                ));
                matchers.truncate(matchers.len() - added);
                return Ok(());
            }
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, entry) in entries {
            let path = entry.path();
            let child_rel = join_rel(rel, &name);
            let Ok(ft) = entry.file_type() else {
                self.skipped
                    .unreadable
                    .push((child_rel, "file type unknown".into()));
                continue;
            };
            if ft.is_symlink() {
                self.skipped.symlinks += 1;
                continue;
            }
            let is_dir = ft.is_dir();
            if self.excludes.iter().any(|x| x == &path) {
                self.skipped.excluded_entries += 1;
                continue;
            }
            if !self.opts.include_hidden && name.starts_with('.') {
                self.skipped.hidden_entries += 1;
                continue;
            }
            match Self::ignored_by(matchers, &path, is_dir) {
                Some(Source::Cyberbrain) => {
                    self.skipped.ignored_entries += 1;
                    continue;
                }
                Some(Source::Git) => {
                    self.skipped.gitignored_entries += 1;
                    continue;
                }
                None => {}
            }
            if is_dir {
                self.dir(&path, &child_rel, matchers, f)?;
                continue;
            }
            if !ft.is_file() {
                self.skipped
                    .unreadable
                    .push((child_rel, "not a regular file".into()));
                continue;
            }
            if LOCKFILES.contains(&name.as_str()) {
                self.skipped.lockfiles += 1;
                continue;
            }
            let Some(language) = Language::of_path(&path) else {
                self.skipped.unsupported += 1;
                let ext = Path::new(&name)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_else(|| "(no extension)".into());
                *self
                    .skipped
                    .unsupported_by_extension
                    .entry(ext)
                    .or_insert(0) += 1;
                continue;
            };
            let size = match entry.metadata() {
                Ok(m) => m.len(),
                Err(e) => {
                    self.skipped.unreadable.push((child_rel, e.to_string()));
                    continue;
                }
            };
            if size > self.opts.max_file_bytes {
                self.skipped.too_large += 1;
                continue;
            }
            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    self.skipped.unreadable.push((child_rel, e.to_string()));
                    continue;
                }
            };
            let probe = &bytes[..bytes.len().min(BINARY_PROBE)];
            if memchr::memchr(0, probe).is_some() {
                self.skipped.binary += 1;
                continue;
            }
            let text = String::from_utf8_lossy(&bytes);
            self.files_scanned += 1;
            self.bytes_scanned += size;
            f(File {
                rel: &child_rel,
                language,
                text: &text,
            });
        }
        matchers.truncate(matchers.len() - added);
        Ok(())
    }
}

fn join_rel(rel: &str, name: &str) -> String {
    if rel.is_empty() {
        name.to_string()
    } else {
        format!("{rel}/{name}")
    }
}

fn absolute(p: &Path) -> Result<PathBuf> {
    std::path::absolute(p).map_err(|e| Error::Io {
        path: p.to_path_buf(),
        source: e,
    })
}
