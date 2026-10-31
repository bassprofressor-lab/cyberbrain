//! Where inside the store a path points, if inside at all.
//!
//! `pre-tool-use` sees a file-editing tool's `file_path` and has to decide whether that
//! file is the audit log, a resident note, the config, or nothing of ours. The comparison
//! is lexical: components after `.`/`..` folding, no `canonicalize` (a `Write` targets a
//! file that does not exist yet, and a `stat` per tool call is a cost paid on every action
//! the agent takes). Symlinked stores are therefore not seen through; documented.
//!
//! SPEC §14.8: platform-dependent normalisation is a hazard. The rules are parameterised by
//! [`Platform`] and all are tested on every host: Windows folds case and accepts either
//! separator and a drive or UNC prefix; Unix folds nothing and splits only on `/`; a Mac
//! splits like Unix but folds case, as its default file system does (C5, 2026-10-31). Getting
//! a match wrong here is fail-safe in one direction only — an over-match produces an `ask`
//! or a `deny` the operator can see, an under-match lets a raw edit through — so the
//! Windows rules err towards matching.

use cyberbrain_core::{Ring, slash};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    /// Unix paths on a file system that, by default (APFS, HFS+), ignores case.
    Mac,
    Windows,
}

impl Platform {
    pub fn host() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else {
            Platform::Unix
        }
    }

    /// Whether two names that differ only in case name the same file here.
    pub fn folds_case(self) -> bool {
        matches!(self, Platform::Windows | Platform::Mac)
    }
}

/// What a path inside the store is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreTarget {
    /// `audit.db` and its WAL/journal side files.
    AuditLog,
    /// `notes/r0/*` or `notes/r1/*`.
    ResidentNote(Ring),
    /// `notes/r2..r4/*`.
    Note(Ring),
    /// `cyberbrain.toml`.
    Config,
    /// `cyberbrain.db` and side files.
    IndexCache,
    /// `sessions/*`: this module's own state.
    SessionState,
    /// `proposals/*`: notes waiting for somebody else to accept them.
    Proposal,
    /// `keys/*`: the audit log's signing key.
    SigningKey,
    /// Inside the store, none of the above (`models/`, a stray file).
    Other,
}

/// Lexically normalised components. `prefix` is the drive/UNC/root marker so `C:\a` and
/// `D:\a` never compare equal and a relative path never equals an absolute one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Normal {
    prefix: String,
    parts: Vec<String>,
}

fn fold(platform: Platform, s: &str) -> String {
    if platform.folds_case() {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

fn is_sep(platform: Platform, c: char) -> bool {
    c == '/' || (platform == Platform::Windows && c == '\\')
}

/// Absolute for the platform: `/x` on Unix; `C:\`, `C:/`, `\\server\share` or `//server`
/// on Windows.
pub fn is_absolute(platform: Platform, s: &str) -> bool {
    match platform {
        Platform::Unix | Platform::Mac => s.starts_with('/'),
        Platform::Windows => {
            let s = &*long_form(s);
            let b = s.as_bytes();
            (b.len() >= 3
                && b[0].is_ascii_alphabetic()
                && b[1] == b':'
                && (b[2] == b'\\' || b[2] == b'/'))
                || s.starts_with(r"\\")
                || s.starts_with("//")
        }
    }
}

fn normalise(platform: Platform, s: &str) -> Normal {
    let s = if platform == Platform::Windows {
        long_form(s)
    } else {
        std::borrow::Cow::Borrowed(s)
    };
    let s = &*s;
    let (prefix, rest) = match platform {
        Platform::Unix | Platform::Mac => {
            if let Some(r) = s.strip_prefix('/') {
                ("/".to_string(), r)
            } else {
                (String::new(), s)
            }
        }
        Platform::Windows => {
            let b = s.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                let rest = &s[2..];
                let rooted = rest.starts_with(['\\', '/']);
                (
                    format!(
                        "{}:{}",
                        fold(platform, &s[..1]),
                        if rooted { "\\" } else { "" }
                    ),
                    rest.trim_start_matches(['\\', '/']),
                )
            } else if let Some(r) = s.strip_prefix(r"\\").or_else(|| s.strip_prefix("//")) {
                (r"\\".to_string(), r)
            } else if let Some(r) = s.strip_prefix(['\\', '/']) {
                ("\\".to_string(), r)
            } else {
                (String::new(), s)
            }
        }
    };
    let mut parts: Vec<String> = Vec::new();
    for raw in rest.split(|c| is_sep(platform, c)) {
        match raw {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(fold(platform, p)),
        }
    }
    Normal { prefix, parts }
}

/// A Windows path without its long-path prefix: `\\?\UNC\server\share\x` is
/// `\\server\share\x`, and `\\?\C:\x` and the device form `\\.\C:\x` are `C:\x`. Only the
/// `\\?\` prefix was taken off before 2026-10-31, and `UNC\server\…` then read as a relative
/// path (C8).
fn long_form(s: &str) -> std::borrow::Cow<'_, str> {
    for unc in [r"\\?\UNC\", r"\\?\unc\", "//?/UNC/", "//?/unc/"] {
        if let Some(rest) = s.strip_prefix(unc) {
            return std::borrow::Cow::Owned(format!(r"\\{rest}"));
        }
    }
    for prefix in [r"\\?\", r"\\.\", "//?/", "//./"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return std::borrow::Cow::Borrowed(rest);
        }
    }
    std::borrow::Cow::Borrowed(s)
}

/// `C:x`: a drive letter and no separator after it.
fn is_drive_relative(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && !matches!(b.get(2), Some(b'\\' | b'/'))
}

/// Make `target` absolute against `cwd` when it is relative. Both are strings from the
/// harness or the CLI; no file system is touched.
///
/// Windows has two relative forms that are not relative to `cwd`: `\x` is rooted on the
/// drive (or share) `cwd` is on, and `C:x` is relative to drive C's own current directory.
/// That directory is `cwd` when `cwd` is on C; for another drive it is not known here, and
/// the drive's root is the guess.
fn absolutise(platform: Platform, cwd: &str, target: &str) -> Normal {
    if is_absolute(platform, target) {
        normalise(platform, target)
    } else if platform == Platform::Windows && is_drive_relative(target) {
        let (drive, rest) = target.split_at(2);
        if cwd.get(..2).is_some_and(|d| d.eq_ignore_ascii_case(drive)) {
            normalise(platform, &format!("{cwd}\\{rest}"))
        } else {
            normalise(platform, &format!("{drive}\\{rest}"))
        }
    } else if platform == Platform::Windows && target.starts_with(['\\', '/']) {
        let base = normalise(platform, cwd);
        let rooted = normalise(platform, target);
        // A share's root is `\\server\share`, two components below the prefix.
        let keep = if base.prefix == r"\\" { 2 } else { 0 };
        let mut parts: Vec<String> = base.parts.into_iter().take(keep).collect();
        parts.extend(rooted.parts);
        Normal {
            prefix: base.prefix,
            parts,
        }
    } else {
        let sep = if platform == Platform::Windows {
            '\\'
        } else {
            '/'
        };
        normalise(platform, &format!("{cwd}{sep}{target}"))
    }
}

/// `Some(target)` when `path` lies inside `store_root`; `None` otherwise. `cwd` resolves
/// relative paths, both for the target and for a store root given relatively (the CI job
/// runs `init --store .cyberbrain`).
pub fn classify(
    platform: Platform,
    store_root: &str,
    cwd: &str,
    path: &str,
) -> Option<StoreTarget> {
    let root = absolutise(platform, cwd, store_root);
    let target = absolutise(platform, cwd, path);
    if root.prefix != target.prefix || target.parts.len() <= root.parts.len() {
        return None;
    }
    if target.parts[..root.parts.len()] != root.parts[..] {
        return None;
    }
    let inside: Vec<&str> = target.parts[root.parts.len()..]
        .iter()
        .map(String::as_str)
        .collect();
    let base = |name: &str| -> bool {
        // `audit.db`, `audit.db-wal`, `audit.db-shm`, `audit.db-journal`.
        inside.len() == 1 && (inside[0] == name || inside[0].starts_with(&format!("{name}-")))
    };
    if base("audit.db") {
        return Some(StoreTarget::AuditLog);
    }
    if base("cyberbrain.db") {
        return Some(StoreTarget::IndexCache);
    }
    if inside.len() == 1 && inside[0] == "cyberbrain.toml" {
        return Some(StoreTarget::Config);
    }
    if inside.len() >= 2 && inside[0] == "sessions" {
        return Some(StoreTarget::SessionState);
    }
    if inside.len() >= 2 && inside[0] == "proposals" {
        return Some(StoreTarget::Proposal);
    }
    if inside.len() >= 2 && inside[0] == "keys" {
        return Some(StoreTarget::SigningKey);
    }
    if inside.len() >= 3 && inside[0] == "notes" {
        let ring = Ring::ALL.iter().copied().find(|r| r.dir() == inside[1]);
        return Some(match ring {
            Some(r) if r.is_resident() => StoreTarget::ResidentNote(r),
            Some(r) => StoreTarget::Note(r),
            None => StoreTarget::Other,
        });
    }
    Some(StoreTarget::Other)
}

/// Host-platform convenience over [`classify`] for `Path`s.
pub fn classify_host(store_root: &Path, cwd: &Path, path: &Path) -> Option<StoreTarget> {
    classify(
        Platform::host(),
        &store_root.to_string_lossy(),
        &cwd.to_string_lossy(),
        &path.to_string_lossy(),
    )
}

/// The path relative to the store root, for messages. Falls back to the path itself.
pub fn display_inside(store_root: &Path, cwd: &Path, path: &Path) -> String {
    let platform = Platform::host();
    let root = absolutise(
        platform,
        &cwd.to_string_lossy(),
        &store_root.to_string_lossy(),
    );
    let target = absolutise(platform, &cwd.to_string_lossy(), &path.to_string_lossy());
    if target.parts.len() > root.parts.len() && target.parts[..root.parts.len()] == root.parts[..] {
        format!(
            "{}/{}",
            store_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".cyberbrain".into()),
            target.parts[root.parts.len()..].join("/")
        )
    } else {
        slash(path)
    }
}
