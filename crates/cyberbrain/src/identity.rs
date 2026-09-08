//! Who is at this machine, for the review workflow.
//!
//! **Not in `cyberbrain.toml`.** Two reasons, and either alone would settle it. The store is
//! meant to live in a repository, so a name in it is a name committed on behalf of whoever
//! cloned it next. And `Config` denies unknown fields, so a new section there would make
//! every older binary refuse the store outright — every command, not just this one.
//!
//! So it lives where the hub token lives: the user's own configuration directory, one file,
//! outside any repository. Environment first, then that file, then what git already knows,
//! because on a developer's machine git usually knows.
//!
//! **This is not an authentication.** Anyone who can write the file is anyone. What it buys
//! is a name on an audit row and a two-person rule that a person cannot walk into by
//! accident — not one they cannot walk around on purpose. The hub's version of the same rule
//! is backed by tokens; this one is a workflow with a record.

use cyberbrain_core::{Error, Result};
use std::path::{Path, PathBuf};

pub const ENV: &str = "CYBERBRAIN_IDENTITY";

/// `%APPDATA%\cyberbrain\identity` on Windows, `~/.config/cyberbrain/identity` elsewhere.
pub fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("cyberbrain").join("identity"))
}

/// The name to put on a proposal or a review.
///
/// `store` is where the git fallback is asked from, so the answer is the identity that
/// repository is configured with rather than whatever the working directory happens to be.
pub fn who(store: &Path) -> Result<String> {
    if let Some(name) = from_env() {
        return Ok(name);
    }
    if let Some(name) = from_file() {
        return Ok(name);
    }
    if let Some(name) = from_git(store) {
        return Ok(name);
    }
    Err(Error::Config(format!(
        "this machine has no identity, and a proposal has to say who made it.\n\
         Set one of:\n  \
         {ENV}=<you>\n  \
         {}\n  \
         git config user.email",
        path()
            .map(|p| format!("a line in {}", cyberbrain_core::Slash(&p)))
            .unwrap_or_else(|| "a configuration directory this platform did not name".into())
    )))
}

fn clean(raw: &str) -> Option<String> {
    // One line, trimmed. A name with a newline in it would put a forged row into a rendered
    // audit log, and a name is a name.
    let name = raw.lines().next().unwrap_or_default().trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn from_env() -> Option<String> {
    clean(&std::env::var(ENV).ok()?)
}

fn from_file() -> Option<String> {
    clean(&std::fs::read_to_string(path()?).ok()?)
}

/// What git is configured with, asked in the store's own directory.
///
/// A fallback rather than the first choice: git's identity is the one commits are signed
/// with, and somebody may well want the two to differ. It is here because on the machines
/// this is for, it is already correct.
fn from_git(store: &Path) -> Option<String> {
    let dir = store.parent().unwrap_or(store);
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["config", "--get", "user.email"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    clean(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_trimmed_line() {
        assert_eq!(clean("  someone  \n"), Some("someone".to_string()));
        // A forged second line would land in a rendered audit log looking like a row.
        assert_eq!(clean("someone\nnote.write x"), Some("someone".to_string()));
        assert_eq!(clean("   "), None);
        assert_eq!(clean(""), None);
    }

    #[test]
    fn the_environment_wins_over_everything() {
        // Not a real store; `who` must not need one to answer from the environment.
        unsafe { std::env::set_var(ENV, "from-env") };
        let got = who(Path::new("/nonexistent/.cyberbrain")).unwrap();
        unsafe { std::env::remove_var(ENV) };
        assert_eq!(got, "from-env");
    }
}
