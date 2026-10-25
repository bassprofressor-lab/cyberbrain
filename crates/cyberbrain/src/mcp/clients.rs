//! Who may call `cyberbrain mcp --http`, and as whom (C3, 2026-10-03).
//!
//! One line per client in `~/.config/cyberbrain/mcp-clients`, beside the identity and the
//! AgentGuard key and for the same reason: not in the store, which lives in a repository. A
//! line is `<name> <sha256 of the token, hex>`, optionally followed by `expires=<RFC 3339>`;
//! the token itself is shown once, at `add`, and never stored. A line without an expiry is a
//! token without one, which is every line written before expiries existed. A request authenticates with `Authorization: Bearer <token>` and is served as
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

/// One registered client.
#[derive(Debug, Clone, PartialEq)]
pub struct Client {
    pub name: String,
    pub digest: String,
    /// When its token stops working; `None` for a token without an end.
    pub expires_at: Option<String>,
}

impl Client {
    pub fn new(name: &str, digest: String, expires_at: Option<String>) -> Self {
        Self {
            name: name.to_string(),
            digest,
            expires_at,
        }
    }
}

/// Read the file's text. Lines that do not parse are skipped; an unknown extra field is
/// ignored, so a later version may add one.
pub fn parse(text: &str) -> Vec<Client> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let (n, d) = (it.next()?, it.next()?);
            if validate_name(n).is_err() || d.len() != 64 {
                return None;
            }
            let expires_at = it
                .find_map(|f| f.strip_prefix("expires="))
                .map(str::to_string);
            Some(Client::new(n, d.to_string(), expires_at))
        })
        .collect()
}

pub fn load() -> Vec<Client> {
    let Some(p) = path() else { return Vec::new() };
    std::fs::read_to_string(p)
        .map(|t| parse(&t))
        .unwrap_or_default()
}

/// What a bearer token is.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    Client(String),
    /// A registered client whose token ran out. Told apart from an unknown token so the
    /// answer can say what to do.
    Expired {
        name: String,
        at: String,
    },
    Unknown,
}

/// The client a bearer token belongs to, judged against `now`.
pub fn lookup(clients: &[Client], token: &str, now: jiff::Timestamp) -> Lookup {
    let d = digest(token);
    let Some(c) = clients
        .iter()
        .find(|c| constant_time_eq(c.digest.as_bytes(), d.as_bytes()))
    else {
        return Lookup::Unknown;
    };
    match crate::expiry::state(c.expires_at.as_deref(), now) {
        crate::expiry::State::Expired => Lookup::Expired {
            name: c.name.clone(),
            at: c.expires_at.clone().unwrap_or_default(),
        },
        _ => Lookup::Client(c.name.clone()),
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Register a client; returns its token, which is not kept. `expires_at` as
/// [`crate::expiry::parse`] returns it.
pub fn add(name: &str, expires_at: Option<String>) -> Result<String> {
    validate_name(name)?;
    let mut all = load();
    if all.iter().any(|c| c.name == name) {
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
    all.push(Client::new(name, digest(&token), expires_at));
    save(&all)?;
    Ok(token)
}

/// Remove a client. `Ok(false)` when there was none of that name.
pub fn remove(name: &str) -> Result<bool> {
    let mut all = load();
    let before = all.len();
    all.retain(|c| c.name != name);
    if all.len() == before {
        return Ok(false);
    }
    save(&all)?;
    Ok(true)
}

fn save(all: &[Client]) -> Result<()> {
    let p = path().ok_or_else(|| Error::Config("no configuration directory".into()))?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
    }
    let text: String = all.iter().map(render).collect();
    // Only hashes, but owner-only from the start all the same, and replaced in one step so
    // a reader never sees half a list.
    crate::hub::client::write_owner_only(&p, &text).map_err(|e| Error::Io {
        path: p.clone(),
        source: e,
    })?;
    Ok(())
}

/// One line of the file. A client without an expiry is written as before, so a file that
/// uses none reads the same to an older version.
fn render(c: &Client) -> String {
    match &c.expires_at {
        None => format!("{} {}\n", c.name, c.digest),
        Some(at) => format!("{} {} expires={at}\n", c.name, c.digest),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> jiff::Timestamp {
        "2026-10-25T12:00:00Z".parse().unwrap()
    }

    #[test]
    fn a_token_finds_its_client_and_nothing_else_does() {
        let clients = vec![
            Client::new("seo", digest("cbm_a"), None),
            Client::new("n8n", digest("cbm_b"), None),
        ];
        assert_eq!(
            lookup(&clients, "cbm_b", now()),
            Lookup::Client("n8n".into())
        );
        assert_eq!(lookup(&clients, "cbm_c", now()), Lookup::Unknown);
        assert_eq!(lookup(&clients, "", now()), Lookup::Unknown);
    }

    /// A file written before expiries existed reads as clients without one, and a client
    /// without one is written exactly as before.
    #[test]
    fn a_file_from_before_expiries_reads_and_writes_the_same() {
        let d = digest("cbm_a");
        let old = format!("seo {d}\n");
        let read = parse(&old);
        assert_eq!(read, [Client::new("seo", d.clone(), None)]);
        assert_eq!(render(&read[0]), old);
        assert_eq!(
            lookup(&read, "cbm_a", now()),
            Lookup::Client("seo".into()),
            "no end, no expiry"
        );
    }

    #[test]
    fn an_expired_token_is_named_as_expired() {
        let line = format!("n8n {} expires=2026-10-01T00:00:00Z\n", digest("cbm_b"));
        let clients = parse(&line);
        assert_eq!(
            clients[0].expires_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        assert_eq!(render(&clients[0]), line);
        assert_eq!(
            lookup(&clients, "cbm_b", now()),
            Lookup::Expired {
                name: "n8n".into(),
                at: "2026-10-01T00:00:00Z".into()
            }
        );
        let later = parse(&format!(
            "n8n {} expires=2027-10-01T00:00:00Z\n",
            digest("cbm_b")
        ));
        assert_eq!(lookup(&later, "cbm_b", now()), Lookup::Client("n8n".into()));
    }

    #[test]
    fn names_are_plain() {
        assert!(validate_name("seo-agent.1").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("a b").is_err());
        assert!(validate_name("x:y").is_err());
    }
}
