//! Waivers a store may ask for and only its user may grant (0.8.2, 2026-10-27).
//!
//! Three keys in `cyberbrain.toml` open a door to another machine: `inference.allow_public_endpoint`
//! and `inference.allow_overlay_network` (the inference, embedding and transcription endpoints) and
//! `hub.allow_public_hub`. Up to 0.8.1 the store's file alone decided. A store is meant to live in
//! a repository, so whoever wrote the repository decided: a cloned project whose
//! `.cyberbrain/cyberbrain.toml` set `allow_public_endpoint = true` and an endpoint of its choosing
//! had this machine send note text there, and the egress gate recorded it as permitted.
//!
//! From 0.8.2 such a key only takes effect when the user granted it for this store, in a file the
//! repository cannot write: `~/.config/cyberbrain/waivers.toml` (`%APPDATA%\cyberbrain\waivers.toml`
//! on Windows). Both have to say yes. The store says what it needs, the user says that this store
//! may have it. A key the store sets and the user did not grant is switched off and listed in
//! [`Config::waivers_ignored`](crate::config::Config::waivers_ignored), so `status` and `doctor`
//! can say so instead of the endpoint failing without a reason.
//!
//! The pattern is the one Codex (`[projects]` trust in the user config), VS Code
//! (`restrictedConfigurations`: only the user's value counts in an untrusted workspace) and Nix
//! (`accept-flake-config`, per-setting trust stored in the user's data directory) use. From mise's
//! CVE-2026-35533 comes the second rule: nothing that decides trust is ever read from the store.
//! There is no key in `cyberbrain.toml` that names, moves or widens this file; only the
//! environment can point elsewhere (`CYBERBRAIN_WAIVERS`), and a repository does not set the
//! environment of the person who clones it.
//!
//! ```toml
//! # ~/.config/cyberbrain/waivers.toml
//! ["/home/me/project/.cyberbrain"]
//! allow_public_endpoint = true
//! ```
//!
//! A store is named by its canonical path, so a symlink or a `..` cannot make one grant cover
//! another store. A file that does not parse grants nothing.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Points at a different waivers file. For tests and for people who keep their configuration
/// elsewhere; never read from the store.
pub const ENV: &str = "CYBERBRAIN_WAIVERS";

/// File name inside the user's cyberbrain configuration directory.
pub const FILE: &str = "waivers.toml";

/// The three keys, as they are written in `cyberbrain.toml`.
pub const KEYS: [&str; 3] = [
    "inference.allow_public_endpoint",
    "inference.allow_overlay_network",
    "hub.allow_public_hub",
];

/// What the user granted one store. Every field defaults to no.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grant {
    pub allow_public_endpoint: bool,
    pub allow_overlay_network: bool,
    pub allow_public_hub: bool,
}

impl Grant {
    /// Whether `key` (one of [`KEYS`]) is granted.
    pub fn allows(&self, key: &str) -> bool {
        match key {
            "inference.allow_public_endpoint" => self.allow_public_endpoint,
            "inference.allow_overlay_network" => self.allow_overlay_network,
            "hub.allow_public_hub" => self.allow_public_hub,
            _ => false,
        }
    }

    pub fn set(&mut self, key: &str, on: bool) {
        match key {
            "inference.allow_public_endpoint" => self.allow_public_endpoint = on,
            "inference.allow_overlay_network" => self.allow_overlay_network = on,
            "hub.allow_public_hub" => self.allow_public_hub = on,
            _ => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Grant::default()
    }
}

/// The user's waivers file: store path to grant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Waivers {
    pub stores: BTreeMap<String, Grant>,
}

/// Where the waivers file is: `CYBERBRAIN_WAIVERS`, else the user's configuration directory.
/// `None` when neither the variable nor a home directory is known; then nothing is granted.
pub fn path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(ENV) {
        return Some(PathBuf::from(p));
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("cyberbrain").join(FILE))
}

/// The key a store is filed under: its canonical path, with forward slashes so the file reads
/// the same on every system. A store that cannot be canonicalised keeps the path it was given.
pub fn store_key(store: &Path) -> String {
    let p = std::fs::canonicalize(store).unwrap_or_else(|_| store.to_path_buf());
    let s = p.to_string_lossy().replace('\\', "/");
    // Windows canonical paths carry the verbatim prefix; it is noise in a file people edit.
    s.strip_prefix("//?/").map(str::to_string).unwrap_or(s)
}

/// Read the waivers file. Missing: empty. Unreadable or malformed: empty, with the reason, so the
/// caller can say why a grant did not count. Nothing is granted on doubt.
pub fn load() -> (Waivers, Option<String>) {
    let Some(p) = path() else {
        return (Waivers::default(), None);
    };
    load_from(&p)
}

pub fn load_from(p: &Path) -> (Waivers, Option<String>) {
    match std::fs::read_to_string(p) {
        Ok(text) => match toml::from_str::<Waivers>(&text) {
            Ok(w) => (w, None),
            Err(e) => (
                Waivers::default(),
                Some(format!(
                    "{}: {}; no waiver in it counts",
                    p.to_string_lossy().replace('\\', "/"),
                    e.message()
                )),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Waivers::default(), None),
        Err(e) => (
            Waivers::default(),
            Some(format!(
                "{}: {e}; no waiver in it counts",
                p.to_string_lossy().replace('\\', "/")
            )),
        ),
    }
}

/// The grant for one store.
pub fn grant_for(w: &Waivers, store: &Path) -> Grant {
    w.stores.get(&store_key(store)).copied().unwrap_or_default()
}

/// Write `grant` for `store` into the file at `p`, keeping every other store's entry. An empty
/// grant removes the store's entry. Written to a temporary file and renamed, so a crash leaves
/// either the old file or the new one.
pub fn save_grant(p: &Path, store: &Path, grant: Grant) -> std::io::Result<()> {
    let (mut w, err) = load_from(p);
    if let Some(e) = err {
        // Overwriting a file we could not read would throw away whatever the user had in it.
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e));
    }
    let key = store_key(store);
    if grant.is_empty() {
        w.stores.remove(&key);
    } else {
        w.stores.insert(key, grant);
    }
    let body = toml::to_string_pretty(&w).map_err(|e| std::io::Error::other(e.to_string()))?;
    let text = format!(
        "# Waivers this user granted to individual cyberbrain stores (cyberbrain trust).\n\
         # A store's cyberbrain.toml may ask for these keys; they take effect only when granted here.\n\n{body}"
    );
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grant_is_filed_under_the_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("s");
        std::fs::create_dir(&store).unwrap();
        let via_dotdot = store.join("..").join("s");
        assert_eq!(store_key(&store), store_key(&via_dotdot));
    }

    #[test]
    fn save_keeps_other_stores_and_an_empty_grant_removes_the_entry() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("waivers.toml");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        let mut g = Grant::default();
        g.set("inference.allow_public_endpoint", true);
        save_grant(&file, &a, g).unwrap();
        save_grant(&file, &b, g).unwrap();
        save_grant(&file, &a, Grant::default()).unwrap();
        let (w, err) = load_from(&file);
        assert!(err.is_none());
        assert!(!grant_for(&w, &a).allow_public_endpoint);
        assert!(grant_for(&w, &b).allow_public_endpoint);
    }

    #[test]
    fn a_malformed_file_grants_nothing_and_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("waivers.toml");
        std::fs::write(&file, "[\"/x\"]\nallow_public_endpoint = \"yes\"\n").unwrap();
        let (w, err) = load_from(&file);
        assert!(w.stores.is_empty());
        assert!(err.unwrap().contains("no waiver in it counts"));
        assert!(save_grant(&file, dir.path(), Grant::default()).is_err());
        assert!(std::fs::read_to_string(&file).unwrap().contains("\"yes\""));
    }
}
