//! Changing a JSON file that belongs to another program.
//!
//! Three rules, and they are the reason this is one function rather than three:
//! everything that is not ours is copied through untouched, the previous file is kept
//! beside the new one, and a file that does not parse is left exactly as it is. The last
//! one matters most — a settings file with a trailing comma in it is somebody's work in
//! progress, and replacing it with our idea of the truth would be the worst thing this
//! command could do.

use cyberbrain_core::{Error, Result, Slash};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// What happened to one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    /// Our entry was not there and now is.
    Added,
    /// It was there, saying something else. The new one is in the report.
    Updated,
    /// It was already exactly this. Nothing was written.
    Unchanged,
    /// `--undo`: it was there and is gone.
    Removed,
    /// `--undo` on a file that had nothing of ours in it.
    NothingToUndo,
}

impl Action {
    fn writes(self) -> bool {
        matches!(self, Action::Added | Action::Updated | Action::Removed)
    }
}

/// One file this command touched, or would have.
#[derive(Debug, Clone, Serialize)]
pub struct Change {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    /// Why this file and not another. Printed, because a person who has two of them needs
    /// to know which one their application reads.
    pub why: &'static str,
    pub action: Action,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "cyberbrain_core::path_serde::slash_opt"
    )]
    pub backup: Option<PathBuf>,
}

/// Read a JSON object, let `apply` change it, and write it back if it changed.
///
/// `apply` returns what it did; nothing is written unless it says something changed, so a
/// second run of the same command touches no file and produces no backup.
pub fn edit(
    path: &Path,
    why: &'static str,
    dry_run: bool,
    apply: impl FnOnce(&mut Map<String, Value>) -> Result<Action>,
) -> Result<Change> {
    let existed = path.exists();
    let mut root: Map<String, Value> = match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Map::new(),
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            Error::Config(format!(
                "{} is not valid JSON ({e}); fix it or move it aside, this command will not \
                 overwrite a file it cannot read",
                Slash(path)
            ))
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source: e,
            });
        }
    };

    let action = apply(&mut root)?;
    let mut change = Change {
        path: path.to_path_buf(),
        why,
        action,
        backup: None,
    };
    if !action.writes() || dry_run {
        return Ok(change);
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::Io {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    if existed {
        let backup = with_suffix(path, ".bak");
        std::fs::copy(path, &backup).map_err(|e| Error::Io {
            path: backup.clone(),
            source: e,
        })?;
        change.backup = Some(backup);
    }
    let text = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|e| Error::Index(format!("the configuration does not serialise: {e}")))?
        + "\n";
    std::fs::write(path, text).map_err(|e| Error::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    Ok(change)
}

/// `settings.json` becomes `settings.json.bak`, keeping the original name whole.
///
/// Not `Path::with_extension`, which would turn `claude_desktop_config.json` into
/// `claude_desktop_config.bak` — a name that no longer says what it is a copy of.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

/// Get an object at `key`, creating it. Errors rather than replacing when the file has
/// something else there: a `"hooks": 3` is a broken file, not an empty one.
pub fn object<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
    whose: &str,
) -> Result<&'a mut Map<String, Value>> {
    map.entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| Error::Config(format!("`{key}` in {whose} is not an object")))
}

/// Get an array at `key`, creating it. Same reasoning as [`object`].
pub fn array<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
    whose: &str,
) -> Result<&'a mut Vec<Value>> {
    map.entry(key)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| Error::Config(format!("`{key}` in {whose} is not a list")))
}
