//! What the launcher remembers between starts: the projects it had open.
//!
//! Deliberately not in the store. The store belongs to the project, and a per-machine "these
//! are the folders this person works in" does not; a settings file that travels with a
//! repository would arrive on a colleague's machine pointing at paths that are not there —
//! and now that it is a list, it would name other people's projects as well.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// The projects that were open, and are opened again on the next start. Empty on a
    /// first run, which is what makes the launcher ask instead of guessing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<PathBuf>,

    /// Where a project's page opens. Everyone decides this for themselves, which is why it
    /// is a setting and not a replacement: the browser was the only way until now, and
    /// somebody who wants their memory in the same window as their tabs is not wrong.
    #[serde(default, skip_serializing_if = "OpenIn::is_default")]
    pub open_in: OpenIn,

    /// What every version up to 0.3.0 wrote, when a launcher held one project.
    ///
    /// Read and folded into `projects` by [`load`], never written again: `skip_serializing`
    /// is what makes the migration happen once rather than every time. Anyone upgrading has
    /// this key in their file, and losing the project they had open would be a small
    /// betrayal for no reason.
    #[serde(default, skip_serializing)]
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpenIn {
    /// A window of its own, hosted by WebView2. The default: a program with a Start menu
    /// entry and a notification area icon that answers by opening a browser tab is a
    /// program people file under "browser tab".
    #[default]
    Window,
    /// The system browser, which is what every version up to 0.3.0 did. Kept because it is
    /// a real preference — bookmarks, extensions, a window already full of tabs — and
    /// because it is the way back when WebView2 is missing on an older machine.
    Browser,
}

impl OpenIn {
    fn is_default(&self) -> bool {
        *self == OpenIn::default()
    }
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
    let mut settings: Settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    // The one-project key from before this launcher could hold several. Folded in front:
    // it is the project that was open, so it is the one to reopen first.
    if let Some(old) = settings.project_dir.take()
        && !settings.projects.contains(&old)
    {
        settings.projects.insert(0, old);
    }
    settings
}

/// What a running launcher leaves for the next one that starts: what it has open, and
/// where each of them is listening.
///
/// This is the whole of the "already running" conversation. A second launcher does not
/// need to talk to the first one, ask it anything, or bring a window forward — it needs an
/// address, and then it can open the browser itself and get out of the way. No IPC, no
/// message broadcast, no window handle to find.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Instance {
    /// In the order they were opened, so the last one is the one that was opened most
    /// recently — which is what a second launch shows, being the likeliest thing the
    /// person was just looking at.
    #[serde(default)]
    pub open: Vec<Open>,

    /// The single address written by every version up to 0.3.0. Read so that starting a
    /// new launcher while an old one holds the mutex still opens something, instead of
    /// saying it cannot find an address it is looking for in the wrong shape.
    #[serde(default, skip_serializing)]
    url: Option<String>,
    #[serde(default, skip_serializing)]
    project_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Open {
    pub url: String,
    pub project_dir: PathBuf,
}

impl Instance {
    /// What a launcher has open right now. The legacy fields are read, never constructed.
    pub fn new(open: Vec<Open>) -> Instance {
        Instance {
            open,
            url: None,
            project_dir: None,
        }
    }

    /// The address to show, or `None` when there is nothing usable in the file.
    pub fn latest(&self) -> Option<&str> {
        self.open.last().map(|o| o.url.as_str())
    }
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
    let mut instance: Instance = toml::from_str(&text).ok()?;
    if instance.open.is_empty()
        && let (Some(url), Some(project_dir)) = (instance.url.take(), instance.project_dir.take())
    {
        instance.open.push(Open { url, project_dir });
    }
    Some(instance)
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
    fn saved_folders_come_back_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("nested/desktop.toml");
        let s = Settings {
            projects: vec![
                PathBuf::from("/home/x/proj"),
                PathBuf::from("/home/x/other"),
            ],
            ..Settings::default()
        };
        save(&file, &s).unwrap();
        assert_eq!(load(&file), s);
    }

    /// Everyone who has 0.3.0 has the old key in their file. Losing the project they had
    /// open, on the start after an update, would be a small betrayal for no reason.
    #[test]
    fn the_one_project_of_an_older_launcher_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("desktop.toml");
        std::fs::write(&file, "project_dir = \"/home/x/proj\"\n").unwrap();
        assert_eq!(load(&file).projects, vec![PathBuf::from("/home/x/proj")]);
    }

    /// Migrated once, not on every start: the old key is read and never written back, so a
    /// project the person has since closed does not reappear tomorrow.
    #[test]
    fn the_old_key_is_not_written_again() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("desktop.toml");
        std::fs::write(&file, "project_dir = \"/home/x/proj\"\n").unwrap();
        let migrated = load(&file);
        save(&file, &migrated).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("project_dir"), "{text}");
        assert!(text.contains("projects"), "{text}");
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
    fn a_running_instance_leaves_its_addresses_and_takes_them_back() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("state/desktop-instance.toml");
        let i = Instance::new(vec![
            Open {
                url: "http://127.0.0.1:54312/".to_string(),
                project_dir: PathBuf::from("/home/x/proj"),
            },
            Open {
                url: "http://127.0.0.1:54313/".to_string(),
                project_dir: PathBuf::from("/home/x/other"),
            },
        ]);
        write_instance(&file, &i).unwrap();
        assert_eq!(read_instance(&file), Some(i));
        // The most recently opened one is what a second launch shows.
        assert_eq!(
            read_instance(&file).unwrap().latest(),
            Some("http://127.0.0.1:54313/")
        );
        clear_instance(&file);
        assert_eq!(read_instance(&file), None);
        // Clearing a file that is already gone is not an error anyone should hear about.
        clear_instance(&file);
    }

    /// An old launcher still running while a new one is started: it wrote one address at
    /// the top level, and that is still an address worth opening.
    #[test]
    fn the_single_address_of_an_older_launcher_is_still_read() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("desktop-instance.toml");
        std::fs::write(
            &file,
            "url = \"http://127.0.0.1:54312/\"\nproject_dir = \"/home/x/proj\"\n",
        )
        .unwrap();
        let i = read_instance(&file).unwrap();
        assert_eq!(i.latest(), Some("http://127.0.0.1:54312/"));
        assert_eq!(i.open[0].project_dir, PathBuf::from("/home/x/proj"));
    }

    #[test]
    fn a_broken_instance_file_reads_as_no_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("desktop-instance.toml");
        std::fs::write(&bad, "url = ").unwrap();
        assert_eq!(read_instance(&bad), None);
    }

    /// The window is the default because the note this was built from asks for it, so the
    /// check is that an existing file without the key gets the window rather than the old
    /// behaviour by accident.
    #[test]
    fn a_settings_file_from_before_this_setting_opens_a_window() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("desktop.toml");
        std::fs::write(&file, "projects = [\"/home/x/proj\"]\n").unwrap();
        assert_eq!(load(&file).open_in, OpenIn::Window);
    }

    #[test]
    fn choosing_the_browser_comes_back() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("desktop.toml");
        let s = Settings {
            open_in: OpenIn::Browser,
            ..Settings::default()
        };
        save(&file, &s).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("browser"), "{text}");
        assert_eq!(load(&file).open_in, OpenIn::Browser);
    }

    #[test]
    fn a_first_run_writes_no_key_at_all() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("desktop.toml");
        save(&file, &Settings::default()).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(!text.contains("project"), "{text}");
    }
}
