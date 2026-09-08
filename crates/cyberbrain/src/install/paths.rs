//! Where each client keeps the file this command writes, and how we decide it is there.
//!
//! Split out because it is the part that is easy to get wrong and hard to try: every path
//! here belongs to a program that is not installed on the machine this is built on. So
//! resolution is a pure function of [`Env`] plus a directory listing, and the tests build
//! the directory trees Windows and macOS would have.

use std::path::{Path, PathBuf};

/// The parts of this machine the answers depend on, read once.
///
/// Taken as a value rather than from the process environment inside each function, which
/// is what lets a Linux build check what a Windows machine would be told.
#[derive(Debug, Clone, Default)]
pub struct Env {
    pub os: Os,
    pub home: Option<PathBuf>,
    /// `%APPDATA%`: roaming, and the path every guide on the internet names.
    pub appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%`: among other things, where MSIX puts a packaged application's
    /// redirected profile.
    pub local_appdata: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Os {
    Windows,
    Mac,
    #[default]
    Other,
}

impl Env {
    pub fn current() -> Env {
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        };
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
        Env {
            os,
            home: var("HOME").or_else(|| var("USERPROFILE")),
            appdata: var("APPDATA"),
            local_appdata: var("LOCALAPPDATA"),
        }
    }
}

/// A place a client's configuration may live.
///
/// `marker` is the evidence, and it is deliberately not the configuration file: the file
/// may already exist somewhere the application never reads (see [`claude_desktop`]), so
/// its presence proves nothing about which installation is on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub why: &'static str,
}

/// Where a candidate comes from, before the filesystem is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// One fixed file, present when `marker` is a directory.
    Fixed {
        path: PathBuf,
        marker: PathBuf,
        why: &'static str,
    },
    /// Every subdirectory of `parent` whose name starts with `prefix` is one installation,
    /// with its file at `suffix` inside it. For MSIX, where the folder name carries a
    /// publisher hash that is not ours to hard-code. `suffix` is components rather than one
    /// string so that the separator is the platform's and not a guess.
    Packaged {
        parent: PathBuf,
        prefix: &'static str,
        suffix: &'static [&'static str],
        why: &'static str,
    },
}

/// Turn places into the ones that are actually on this machine.
pub fn resolve(places: &[Place]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for place in places {
        match place {
            Place::Fixed { path, marker, why } => {
                if marker.is_dir() {
                    out.push(Candidate {
                        path: path.clone(),
                        why,
                    });
                }
            }
            Place::Packaged {
                parent,
                prefix,
                suffix,
                why,
            } => {
                let Ok(entries) = std::fs::read_dir(parent) else {
                    continue;
                };
                let mut found: Vec<PathBuf> = entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with(*prefix))
                    .filter(|e| e.path().is_dir())
                    .map(|e| suffix.iter().fold(e.path(), |p, part| p.join(part)))
                    .collect();
                // A listing has no order worth relying on, and the report names these files.
                found.sort();
                out.extend(found.into_iter().map(|path| Candidate { path, why }));
            }
        }
    }
    out
}

/// Claude Desktop's `claude_desktop_config.json` — more than one place, and that is the
/// entire point of this function.
///
/// The Microsoft Store build is packaged as MSIX, and MSIX virtualises `%APPDATA%`: the
/// application reads and writes inside its own package folder, while the *Edit Config*
/// button in that same application resolves the real `%APPDATA%` and opens a different
/// file. The two are never synchronised, nothing warns, and a server configured in the
/// documented location simply never loads (anthropics/claude-code#26073, open since
/// February 2026). Writing only the documented path is therefore the exact failure this
/// command exists to remove, so every installation that is present gets the entry.
pub fn claude_desktop(env: &Env) -> Vec<Place> {
    const FILE: &str = "claude_desktop_config.json";
    match env.os {
        Os::Windows => {
            let mut places = Vec::new();
            if let Some(local) = &env.local_appdata {
                places.push(Place::Packaged {
                    parent: local.join("Packages"),
                    // The publisher hash after the underscore is not ours to hard-code, and
                    // a wrong guess here is silence rather than an error.
                    prefix: "Claude_",
                    suffix: &["LocalCache", "Roaming", "Claude", FILE],
                    why: "the Microsoft Store build reads this one; the app's own Edit \
                          Config button does not open it",
                });
            }
            if let Some(appdata) = &env.appdata {
                let dir = appdata.join("Claude");
                places.push(Place::Fixed {
                    path: dir.join(FILE),
                    marker: dir,
                    why: "the installer build reads this one, and it is what Edit Config \
                          opens on any build",
                });
            }
            places
        }
        Os::Mac => {
            let Some(home) = &env.home else {
                return Vec::new();
            };
            let dir = home
                .join("Library")
                .join("Application Support")
                .join("Claude");
            vec![Place::Fixed {
                path: dir.join(FILE),
                marker: dir,
                why: "where Claude Desktop keeps its settings on macOS",
            }]
        }
        Os::Other => {
            let Some(home) = &env.home else {
                return Vec::new();
            };
            let dir = home.join(".config").join("Claude");
            vec![Place::Fixed {
                path: dir.join(FILE),
                marker: dir,
                why: "where a Linux build of Claude Desktop keeps its settings",
            }]
        }
    }
}

/// Codex CLI's `config.toml`. Not written by this command — see the module documentation
/// in `mod.rs` — but its presence is worth reporting, because the person has a client we
/// can serve and would otherwise never learn it.
pub fn codex_config(env: &Env) -> Option<PathBuf> {
    let home = env.home.as_ref()?;
    let dir = home.join(".codex");
    dir.is_dir().then(|| dir.join("config.toml"))
}

/// The project-scoped settings file Claude Code reads. Always answered: unlike the desktop
/// clients this is a file in the user's own project, and creating it is the normal case.
pub fn claude_code(project: &Path) -> PathBuf {
    project.join(".claude").join("settings.json")
}
