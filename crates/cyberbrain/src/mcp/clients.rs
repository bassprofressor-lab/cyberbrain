//! Who may call `cyberbrain mcp --http`, and as whom (C3, 2026-10-03).
//!
//! One line per client in `~/.config/cyberbrain/mcp-clients`, beside the identity and the
//! AgentGuard key and for the same reason: not in the store, which lives in a repository. A
//! line is `<name> <sha256 of the token, hex>`; the token itself is shown once, at `add`, and
//! never stored. A request authenticates with `Authorization: Bearer <token>` and is served as
//! the actor `agent:mcp:<name>`, which is what `[provenance] untrusted_clients` names.

use cyberbrain_core::{Error, Result};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub const TOKEN_PREFIX: &str = "cbm_";

pub fn path() -> Option<PathBuf> {
    crate::identity::path().map(|p| p.with_file_name("mcp-clients"))
}

/// A client name: what follows `agent:mcp:` in the audit log.
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if ok {
        Ok(())
    } else {
        Err(Error::Config(format!(
            "client name `{name}`: 1 to 64 of a-z, A-Z, 0-9, `-`, `_`, `.`"
        )))
    }
}

pub fn digest(token: &str) -> String {
    let h = Sha256::digest(token.as_bytes());
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// `(name, digest)` per line; lines that do not parse are skipped.
pub fn load() -> Vec<(String, String)> {
    let Some(p) = path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(p) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (n, d) = (it.next()?, it.next()?);
            (validate_name(n).is_ok() && d.len() == 64).then(|| (n.to_string(), d.to_string()))
        })
        .collect()
}

/// The client a bearer token belongs to.
pub fn lookup(clients: &[(String, String)], token: &str) -> Option<String> {
    let d = digest(token);
    clients
        .iter()
        .find(|(_, cd)| constant_time_eq(cd.as_bytes(), d.as_bytes()))
        .map(|(n, _)| n.clone())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Register a client; returns its token, which is not kept.
pub fn add(name: &str) -> Result<String> {
    validate_name(name)?;
    let mut all = load();
    if all.iter().any(|(n, _)| n == name) {
        return Err(Error::Config(format!(
            "a client named {name} exists; `cyberbrain mcp-client remove {name}` first"
        )));
    }
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).map_err(|e| Error::Config(format!("no randomness: {e}")))?;
    let token = format!(
        "{TOKEN_PREFIX}{}",
        raw.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    all.push((name.to_string(), digest(&token)));
    save(&all)?;
    Ok(token)
}

/// Remove a client. `Ok(false)` when there was none of that name.
pub fn remove(name: &str) -> Result<bool> {
    let mut all = load();
    let before = all.len();
    all.retain(|(n, _)| n != name);
    if all.len() == before {
        return Ok(false);
    }
    save(&all)?;
    Ok(true)
}

fn save(all: &[(String, String)]) -> Result<()> {
    let p = path().ok_or_else(|| Error::Config("no configuration directory".into()))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
    }
    let text: String = all.iter().map(|(n, d)| format!("{n} {d}\n")).collect();
    std::fs::write(&p, text).map_err(|e| Error::Io {
        path: p.clone(),
        source: e,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_finds_its_client_and_nothing_else_does() {
        let clients = vec![
            ("seo".to_string(), digest("cbm_a")),
            ("n8n".to_string(), digest("cbm_b")),
        ];
        assert_eq!(lookup(&clients, "cbm_b").as_deref(), Some("n8n"));
        assert_eq!(lookup(&clients, "cbm_c"), None);
        assert_eq!(lookup(&clients, ""), None);
    }

    #[test]
    fn names_are_plain() {
        assert!(validate_name("seo-agent.1").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("a b").is_err());
        assert!(validate_name("x:y").is_err());
    }
}
