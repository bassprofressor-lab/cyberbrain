//! Where inside the store a path points, if inside at all.
//!
//! `pre-tool-use` sees a file-editing tool's `file_path` and has to decide whether that
//! file is the audit log, a resident note, the config, or nothing of ours. The comparison
//! is lexical: components after `.`/`..` folding, no `canonicalize` (a `Write` targets a
//! file that does not exist yet, and a `stat` per tool call is a cost paid on every action
//! the agent takes). Symlinked stores are therefore not seen through; documented.
//!
//! SPEC §14.8: platform-dependent normalisation is a hazard. The rules are parameterised by
//! [`Platform`] and both are tested on both hosts: Windows folds case and accepts either
//! separator and a drive or UNC prefix; Unix folds nothing and splits only on `/`. Getting
//! a match wrong here is fail-safe in one direction only — an over-match produces an `ask`
//! or a `deny` the operator can see, an under-match lets a raw edit through — so the
//! Windows rules err towards matching.

use cyberbrain_core::{Ring, slash};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Unix,
    Windows,
}

impl Platform {
    pub fn host() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        }
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
    match platform {
        Platform::Unix => s.to_string(),
        Platform::Windows => s.to_lowercase(),
    }
}

fn is_sep(platform: Platform, c: char) -> bool {
    c == '/' || (platform == Platform::Windows && c == '\\')
}

/// Absolute for the platform: `/x` on Unix; `C:\`, `C:/`, `\\server\share` or `//server`
/// on Windows.
pub fn is_absolute(platform: Platform, s: &str) -> bool {
    match platform {
        Platform::Unix => s.starts_with('/'),
        Platform::Windows => {
            let s = s.strip_prefix(r"\\?\").unwrap_or(s);
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
        s.strip_prefix(r"\\?\").unwrap_or(s)
    } else {
        s
    };
    let (prefix, rest) = match platform {
        Platform::Unix => {
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

/// Make `target` absolute against `cwd` when it is relative. Both are strings from the
/// harness or the CLI; no file system is touched.
fn absolutise(platform: Platform, cwd: &str, target: &str) -> Normal {
    if is_absolute(platform, target) {
        normalise(platform, target)
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
