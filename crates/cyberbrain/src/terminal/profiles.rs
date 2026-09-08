//! Saved command lines: the servers you connect to and the tools you start.
//!
//! **Not in the store**, and that is not a filing preference. The store lives in a
//! repository and travels with it, so a host name and a user name in it would be committed
//! on behalf of whoever clones it next — and half of what makes a saved connection useful is
//! exactly the part nobody wants in a public repository. They live beside the identity and
//! the hub tokens, in the user's own configuration directory, which is per person and per
//! machine, which is what a list of "the servers I use" is.
//!
//! Plain TOML, meant to be edited by hand as readily as through the page:
//!
//! ```toml
//! [[profile]]
//! name = "S2"
//! command = "ssh -o ServerAliveInterval=60 root@api2.example.com"
//! ```
//!
//! Read on every request rather than at startup, so editing the file takes effect without
//! restarting anything.

use cyberbrain_core::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profiles {
    #[serde(default, rename = "profile")]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// What the button says.
    pub name: String,
    /// The command line, exactly as it would be typed. Split by the same code that splits a
    /// line somebody types, so a favourite and a typed line cannot behave differently.
    pub command: String,
}

/// `%APPDATA%\cyberbrain\terminals.toml`, or `~/.config/cyberbrain/terminals.toml`.
pub fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("cyberbrain").join("terminals.toml"))
}

/// What is saved. A missing file is an empty list, not an error: most people have none.
///
/// A file that does not parse **is** an error, and is reported rather than silently read as
/// empty — somebody who mistyped their list should hear about it, not watch it vanish.
pub fn load() -> Result<Profiles> {
    let Some(path) = path() else {
        return Ok(Profiles::default());
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| {
            Error::Config(format!(
                "{} does not parse: {e}",
                cyberbrain_core::Slash(&path)
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Profiles::default()),
        Err(e) => Err(Error::Io { path, source: e }),
    }
}

/// Write the list back, after checking every entry can actually be run.
///
/// The check is here rather than at use: a favourite that fails to split is a button that
/// does nothing, and finding that out at the moment you need the connection is the worst
/// time. An empty name is refused for the same reason — a button with no label.
pub fn save(profiles: &Profiles) -> Result<PathBuf> {
    for p in &profiles.profiles {
        if p.name.trim().is_empty() {
            return Err(Error::Config("a saved command needs a name".into()));
        }
        match crate::serve::command::tokenise(&p.command) {
            Err(why) => {
                return Err(Error::Config(format!("{}: {why}", p.name)));
            }
            Ok(None) => {
                return Err(Error::Config(format!(
                    "{}: there is no command in {:?}",
                    p.name, p.command
                )));
            }
            Ok(Some(_)) => {}
        }
    }
    let path = path().ok_or_else(|| {
        Error::Config("this platform named no configuration directory to save into".into())
    })?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
    }
    let text = toml::to_string_pretty(profiles)
        .map_err(|e| Error::Config(format!("the list does not serialise: {e}")))?;
    std::fs::write(&path, text).map_err(|e| Error::Io {
        path: path.clone(),
        source: e,
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str, command: &str) -> Profile {
        Profile {
            name: name.into(),
            command: command.into(),
        }
    }

    #[test]
    fn a_saved_command_has_to_be_runnable() {
        let bad = Profiles {
            profiles: vec![p("S2", "ssh \"unclosed")],
        };
        let why = save(&bad).unwrap_err().to_string();
        assert!(why.contains("S2"), "{why}");
        assert!(why.contains("quote"), "{why}");
    }

    #[test]
    fn a_saved_command_has_to_be_a_command() {
        let why = save(&Profiles {
            profiles: vec![p("empty", "   ")],
        })
        .unwrap_err()
        .to_string();
        assert!(why.contains("no command"), "{why}");
    }

    #[test]
    fn a_button_needs_a_label() {
        let why = save(&Profiles {
            profiles: vec![p("  ", "ssh host")],
        })
        .unwrap_err()
        .to_string();
        assert!(why.contains("needs a name"), "{why}");
    }

    /// The format people will edit by hand, so it has to read back as what it looks like.
    #[test]
    fn the_file_is_the_shape_the_documentation_promises() {
        let text =
            "[[profile]]\nname = \"S2\"\ncommand = \"ssh -o ServerAliveInterval=60 root@x\"\n";
        let got: Profiles = toml::from_str(text).unwrap();
        assert_eq!(
            got.profiles,
            vec![p("S2", "ssh -o ServerAliveInterval=60 root@x")]
        );
    }

    #[test]
    fn a_file_that_does_not_parse_is_reported_rather_than_read_as_empty() {
        // The failure this prevents: a mistyped list silently becoming no list at all, so
        // the page shows nothing and nobody knows why.
        let e = toml::from_str::<Profiles>("[[profile]]\nname = \n").unwrap_err();
        assert!(!e.to_string().is_empty());
    }
}
