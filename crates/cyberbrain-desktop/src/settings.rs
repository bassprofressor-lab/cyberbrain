//! What the launcher remembers between starts: the project it had open.
//!
//! Deliberately not in the store. The store belongs to the project, and a per-machine "the
//! window was last showing this folder" does not; a settings file that travels with a
//! repository would arrive on a colleague's machine pointing at a path that is not there.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// The project whose store was open last. `None` on a first run, which is what makes
    /// the launcher ask instead of guessing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_dir: Option<PathBuf>,
}

/// `%APPDATA%\cyberbrain\desktop.toml` on Windows, `~/.config/cyberbrain/desktop.toml`
/// elsewhere. Returns `None` when the platform tells us neither, in which case the
/// launcher still runs and simply forgets between starts.
pub fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("cyberbrain").join("desktop.toml"))
}

/// Read the settings, or defaults. A corrupt or unreadable file is not worth a dialog on
/// startup: the launcher asks for a folder, and the answer overwrites the bad file.
pub fn load(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default()
}

/// What a running launcher leaves for the next one that starts: where it is listening.
///
/// This is the whole of the "already running" conversation. A second launcher does not
/// need to talk to the first one, ask it anything, or bring a window forward — it needs the
/// address, and then it can open the browser itself and get out of the way. No IPC, no
/// message broadcast, no window handle to find.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Instance {
    pub url: String,
    pub project_dir: PathBuf,
}

/// `%LOCALAPPDATA%\cyberbrain\desktop-instance.toml`, or the same under `~/.local/state`.
///
/// Local rather than roaming, unlike the settings: a port number on this machine is
/// meaningless on another one, and a roaming profile would carry it there.
pub fn instance_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("APPDATA"))
            .map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
    }?;
    Some(base.join("cyberbrain").join("desktop-instance.toml"))
}

pub fn read_instance(path: &Path) -> Option<Instance> {
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

pub fn write_instance(path: &Path, instance: &Instance) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(instance)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

/// Remove the file on the way out. Best effort: a stale file is not a problem, because the
/// address in it is checked before it is used.
pub fn clear_instance(path: &Path) {
    let _ = std::fs::remove_file(path);
}

pub fn save(path: &Path, settings: &Settings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saved_folder_comes_back() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("nested/desktop.toml");
        let s = Settings {
            project_dir: Some(PathBuf::from("/home/x/proj")),
        };
        save(&file, &s).unwrap();
        assert_eq!(load(&file), s);
    }

    #[test]
    fn a_missing_or_broken_file_is_defaults_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(&tmp.path().join("nothing.toml")), Settings::default());
        let bad = tmp.path().join("bad.toml");
        std::fs::write(&bad, "this is not toml = = =").unwrap();
        assert_eq!(load(&bad), Settings::default());
    }

    #[test]
    fn a_running_instance_leaves_its_address_and_takes_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state/desktop-instance.toml");
        let i = Instance {
            url: "http://127.0.0.1:54312/".to_string(),
            project_dir: PathBuf::from("/home/x/proj"),
        };
        write_instance(&file, &i).unwrap();
        assert_eq!(read_instance(&file), Some(i));
        clear_instance(&file);
        assert_eq!(read_instance(&file), None);
        // Clearing a file that is already gone is not an error anyone should hear about.
        clear_instance(&file);
    }

    #[test]
    fn a_broken_instance_file_reads_as_no_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("desktop-instance.toml");
        std::fs::write(&bad, "url = ").unwrap();
        assert_eq!(read_instance(&bad), None);
    }

    #[test]
    fn a_first_run_writes_no_key_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("desktop.toml");
        save(&file, &Settings::default()).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("project_dir"), "{text}");
    }
}
