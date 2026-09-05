//! Per-session state: `<store>/sessions/<session_id>.json`.
//!
//! One small file per harness session, keyed by the sanitised `session_id`. It is what
//! lets the events talk to each other without touching the index or the audit log on the
//! hot path: `session-start` records what it injected, `user-prompt-submit` compares the
//! resident rings against that, `stop` counts turns, `pre-compact` counts compactions,
//! and `session-start` on `resume`/`compact` reads it all back.
//!
//! It is cache-grade, not a record: a missing or unparseable file means "no baseline" and
//! the hook proceeds as on a fresh session. Written with a plain `fs::write` rather than
//! the fsyncing atomic writer, because a torn file costs one re-injection and an fsync on
//! every turn costs milliseconds on every turn. Files older than [`STATE_MAX_AGE_DAYS`]
//! are swept at `startup`.
//!
//! Not part of the layout in SPEC §4; `Store::list` only walks `notes/`, so it is invisible
//! to scan, doctor and the cap. See the report.

use cyberbrain_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const SESSIONS_DIR: &str = "sessions";
pub const STATE_MAX_AGE_DAYS: u64 = 30;
const MAX_ID_LEN: usize = 96;

/// What a resident note looked like when it was last injected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ResidentMark {
    pub ring: u8,
    pub path: String,
    /// blake3 of the file bytes, hex.
    pub hash: String,
    pub size: u64,
    /// The frontmatter `updated` at injection time, for the change banner.
    pub updated: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SessionState {
    pub session_id: String,
    pub started_at: Option<String>,
    /// The `source` of the first `session-start` seen.
    pub source: Option<String>,
    pub cwd: Option<String>,
    /// Times rings 0/1 were injected in full (session-start, or a prompt with no baseline).
    pub injections: u32,
    pub last_injection_at: Option<String>,
    /// Times a changed resident note was re-injected at a prompt.
    pub reinjections: u32,
    /// `stop` events seen: one per assistant turn.
    pub turns: u32,
    pub last_stop_at: Option<String>,
    pub compactions: u32,
    pub last_compaction_at: Option<String>,
    pub last_compaction_trigger: Option<String>,
    /// Keyed `r{ring}/{name}`.
    pub resident: BTreeMap<String, ResidentMark>,
}

impl SessionState {
    pub fn new(session_id: &str, now: &str) -> Self {
        SessionState {
            session_id: session_id.to_string(),
            started_at: Some(now.to_string()),
            ..Default::default()
        }
    }
}

/// A file name from a harness-supplied id. Keeps `[A-Za-z0-9._-]`, replaces the rest,
/// refuses empty, dot-only and over-long results. `None` means "no usable id": the events
/// then run without state and say so.
pub fn safe_id(raw: &str) -> Option<String> {
    let mut s: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.len() > MAX_ID_LEN {
        s.truncate(MAX_ID_LEN);
    }
    let s = s.trim_matches('.').to_string();
    if s.is_empty() || s.chars().all(|c| c == '_') {
        return None;
    }
    Some(s)
}

pub fn sessions_dir(root: &Path) -> PathBuf {
    root.join(SESSIONS_DIR)
}

pub fn state_path(root: &Path, id: &str) -> PathBuf {
    sessions_dir(root).join(format!("{id}.json"))
}

/// `Ok(None)` when there is no file. A file that does not parse is reported as an error;
/// the caller treats it as "no baseline" and says so.
pub fn load(root: &Path, id: &str) -> Result<Option<SessionState>> {
    let path = state_path(root, id);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Io { path, source: e }),
    };
    serde_json::from_str(&text).map(Some).map_err(|e| {
        Error::Config(format!(
            "{}: session state does not parse: {e}",
            path.display()
        ))
    })
}

pub fn save(root: &Path, id: &str, state: &SessionState) -> Result<()> {
    let dir = sessions_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io {
        path: dir.clone(),
        source: e,
    })?;
    let path = state_path(root, id);
    let text = serde_json::to_string_pretty(state)
        .map_err(|e| Error::Config(format!("session state does not serialise: {e}")))?;
    std::fs::write(&path, text).map_err(|e| Error::Io { path, source: e })
}

/// Remove state files older than [`STATE_MAX_AGE_DAYS`]. Returns how many were removed;
/// a directory that does not exist yet is zero, not an error.
pub fn sweep(root: &Path, now: SystemTime) -> Result<usize> {
    let dir = sessions_dir(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => {
            return Err(Error::Io {
                path: dir,
                source: e,
            });
        }
    };
    let max_age = Duration::from_secs(STATE_MAX_AGE_DAYS * 24 * 3600);
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        let Ok(mtime) = md.modified() else { continue };
        if now.duration_since(mtime).is_ok_and(|age| age > max_age)
            && std::fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}
