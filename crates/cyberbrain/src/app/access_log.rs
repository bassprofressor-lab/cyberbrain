//! Who searched which bereich, and who changed who may see what (SPEC §12.6.3, 2026-11-08).
//!
//! Two rows, both through the one audit writer, so they are chained and signed like the rest:
//!
//! - `search.access` for a search or an expanded citation, by the clients `[audit] searches`
//!   names. It carries the bereiche of what was returned and never the query: a firm's search
//!   for a client's name is that name, and a log meant to show *that* somebody looked must not
//!   become a second copy of *what* they looked for. A keyed hash still says which searches
//!   were the same.
//! - `clients.changed` when `[clients]` differs from the last state the log recorded. The file
//!   is edited by `cb-rechte` or by hand, and neither goes through a command here, so the
//!   change is found where it takes effect: when a process loads the configuration.

use super::{App, lock_index};
use cyberbrain_core::config::SearchAudit;
use cyberbrain_core::{Error, Result};
use cyberbrain_policy::{Actor, AuditAction};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// The key the query hash is made with, relative to the store root. Beside the audit key,
/// under the same `.gitignore`, readable by the owner only.
pub const SEARCH_KEY_FILE: &str = "keys/search.key";

/// Who a `clients.changed` row is written as. The process that notices the change did not
/// make it: an MCP server started for anna must not be booked as the one who gave anna a file.
pub const CONFIG_FILE_ACTOR: &str = "config-file";

/// A hint from whoever changed the file, such as `cb-rechte`, recorded as `claimed_by`. It is
/// a claim and named as one: anybody who can set an environment variable can set this one.
pub const CHANGED_BY_ENV: &str = "CYBERBRAIN_CONFIG_CHANGED_BY";

/// Hex digits of the query hash kept in the row. 64 bits: two different searches of one store
/// colliding is not a concern at that length, and a shorter value is less of a fingerprint.
const QUERY_HASH_HEX: usize = 16;

impl App {
    /// Whether `[audit] searches` asks for this actor's searches: everybody's, nobody's, or
    /// those of a client limited to bereiche.
    pub(super) fn searches_recorded(&self) -> bool {
        match self.config.audit.searches {
            SearchAudit::Off => false,
            SearchAudit::All => true,
            SearchAudit::Limited => self.client_scope().is_some(),
        }
    }

    /// One `search.access` row, when this actor's searches are recorded.
    ///
    /// Failing to write it fails the search: a limited client whose access leaves no trace is
    /// the case the row exists for, and a caller that cannot record what it is about to hand
    /// over must not hand it over (§12.6).
    pub(super) fn record_search(
        &self,
        tool: &str,
        asked: &str,
        requested_bereich: Option<&str>,
        bereiche: &BTreeMap<String, usize>,
        hits: usize,
    ) -> Result<()> {
        if !self.searches_recorded() {
            return Ok(());
        }
        let mut detail = json!({
            "tool": tool,
            "limited": self.client_scope().is_some(),
            "bereiche": bereiche.keys().collect::<Vec<_>>(),
            "per_bereich": bereiche,
            "hits": hits,
            "query_hash": self.query_hash(asked),
        });
        if let Some(b) = requested_bereich {
            detail["requested_bereich"] = json!(b);
        }
        self.policy.audit().record(
            &self.actor,
            AuditAction::SearchAccess,
            format!("search:{tool}"),
            detail,
        )?;
        Ok(())
    }

    /// The bereich of every hit's note, counted. Hits without a bereich count under no name:
    /// a limited client never gets one, and for anybody else "none" is not a bereich.
    pub(super) fn hit_bereiche(
        &self,
        hits: &[cyberbrain_core::Hit],
    ) -> Result<BTreeMap<String, usize>> {
        let ix = lock_index(&self.index)?;
        let mut out = BTreeMap::new();
        for h in hits {
            if let Some(b) = ix.note(&h.note_id)?.and_then(|r| r.front.bereich) {
                *out.entry(b).or_insert(0) += 1;
            }
        }
        Ok(out)
    }

    /// The query as a keyed hash: equal for the same search (case and spacing aside), useless
    /// to anybody without the store's key. The key never leaves this machine, so an exported
    /// log, a hub or AgentGuard cannot test guesses against it. The operator, who holds the
    /// key, can — and can also read every note, which is why that is no loss.
    ///
    /// `None` when there is no key and none can be made (a read-only store): the row is then
    /// written without the hash rather than not at all.
    fn query_hash(&self, asked: &str) -> Option<String> {
        let key = self
            .search_key
            .get_or_init(|| load_or_create_key(&self.root))
            .as_ref()?;
        let normal = asked
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let h = blake3::keyed_hash(key, normal.as_bytes()).to_hex();
        Some(h[..QUERY_HASH_HEX].to_string())
    }

    /// Compare `[clients]` with the last state the log recorded and write the difference.
    ///
    /// Run at every open: cheap (one indexed read of a handful of rows), and the only place a
    /// hand edit can be noticed, because nothing else here sees the file change. A store that
    /// never limited anybody and has no such row writes nothing, so a single user's store
    /// stays as it was. Two processes opening at the same instant after an edit can both write
    /// the row; the second says the same as the first and does no harm.
    pub(super) fn record_clients_change(&self) -> Result<()> {
        let now = clients_state(&self.config.clients);
        let last = self
            .audit_sink
            .read_exact_action(AuditAction::ClientsChanged.as_str())?
            .pop()
            .and_then(|e| e.detail.get("after").cloned());
        let before = match last {
            Some(b) if b == now => return Ok(()),
            Some(b) => Some(b),
            None if now == clients_state(&Default::default()) => return Ok(()),
            None => None,
        };
        let empty = clients_state(&Default::default());
        let old = before.as_ref().unwrap_or(&empty);
        let lists = |v: &Value| -> BTreeMap<String, BTreeSet<String>> {
            v.get("bereiche")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(c, b)| {
                            let set = b
                                .as_array()
                                .map(|a| {
                                    a.iter()
                                        .filter_map(Value::as_str)
                                        .map(str::to_owned)
                                        .collect()
                                })
                                .unwrap_or_default();
                            (c.clone(), set)
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (was, is) = (lists(old), lists(&now));
        let diff = |a: &BTreeMap<String, BTreeSet<String>>,
                    b: &BTreeMap<String, BTreeSet<String>>| {
            let mut out = BTreeMap::new();
            for (c, set) in a {
                let gone: Vec<&String> = match b.get(c) {
                    Some(other) => set.difference(other).collect(),
                    None => set.iter().collect(),
                };
                if !gone.is_empty() {
                    out.insert(c.clone(), json!(gone));
                }
            }
            out
        };
        let mut detail = json!({
            "baseline": before.is_none(),
            "before": before,
            "after": now,
            "added": diff(&is, &was),
            "removed": diff(&was, &is),
            "clients_added": is.keys().filter(|c| !was.contains_key(*c)).collect::<Vec<_>>(),
            "clients_removed": was.keys().filter(|c| !is.contains_key(*c)).collect::<Vec<_>>(),
            "detected_by": self.actor.to_string(),
        });
        if old.get("unlisted") != now.get("unlisted") {
            detail["unlisted"] = json!({ "before": old["unlisted"], "after": now["unlisted"] });
        }
        if let Some(who) = std::env::var(CHANGED_BY_ENV)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            detail["claimed_by"] = json!(who);
        }
        self.policy.audit().record(
            &Actor::System(CONFIG_FILE_ACTOR.into()),
            AuditAction::ClientsChanged,
            "config:clients",
            detail,
        )?;
        Ok(())
    }

    /// The audit log is the operator's. A limited client is refused it like `status`: it
    /// names other clients, their bereiche and how much they searched.
    pub(super) fn refuse_limited_audit_reader(&self, what: &str) -> Result<()> {
        if self.client_scope().is_some() {
            return Err(Error::PolicyRefusal {
                profile: "clients".to_string(),
                reason: format!(
                    "{} is limited to bereiche in [clients.bereiche]; {what} reads the audit \
                     log of the whole store and is not shown to it",
                    self.actor
                ),
            });
        }
        Ok(())
    }
}

/// `[clients]` in the one form rows are compared in: client names trimmed (as the limit
/// matches them), bereiche sorted and without repeats, so reordering a list is no change.
fn clients_state(c: &cyberbrain_core::config::ClientsConfig) -> Value {
    let mut bereiche: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (client, list) in &c.bereiche {
        bereiche
            .entry(client.trim().to_string())
            .or_default()
            .extend(list.iter().cloned());
    }
    json!({
        "unlisted": serde_json::to_value(c.unlisted).unwrap_or(Value::Null),
        "bereiche": bereiche,
    })
}

/// The store's search key, made on first use. If two processes make one at the same moment,
/// the file that ends up on disk is read back and used by both from then on.
fn load_or_create_key(root: &std::path::Path) -> Option<[u8; 32]> {
    let path = root.join(SEARCH_KEY_FILE);
    let parse = |text: &str| -> Option<[u8; 32]> {
        let t = text.trim();
        if t.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&t[i * 2..i * 2 + 2], 16).ok()?;
        }
        Some(out)
    };
    if let Ok(text) = std::fs::read_to_string(&path) {
        return parse(&text);
    }
    let dir = path.parent()?;
    std::fs::create_dir_all(dir).ok()?;
    let ignore = dir.join(".gitignore");
    if !ignore.exists() {
        let _ = std::fs::write(&ignore, "# Private keys. Never commit these.\n*\n");
    }
    let mut seed = [0u8; 32];
    getrandom::fill(&mut seed).ok()?;
    let hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
    crate::hub::client::write_owner_only(&path, &format!("{hex}\n")).ok()?;
    parse(&std::fs::read_to_string(&path).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberbrain_core::config::{ClientsConfig, Unlisted};

    #[test]
    fn clients_state_ignores_order_repeats_and_padding() {
        let mut a = ClientsConfig::default();
        a.bereiche.insert(
            "agent:mcp:anna".into(),
            vec!["b".into(), "a".into(), "a".into()],
        );
        let mut b = ClientsConfig::default();
        b.bereiche
            .insert(" agent:mcp:anna ".into(), vec!["a".into(), "b".into()]);
        assert_eq!(clients_state(&a), clients_state(&b));
        b.unlisted = Unlisted::Nothing;
        assert_ne!(clients_state(&a), clients_state(&b));
        assert_eq!(clients_state(&b)["unlisted"], "nothing");
    }

    #[test]
    fn the_search_key_is_made_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let k1 = load_or_create_key(dir.path()).unwrap();
        let k2 = load_or_create_key(dir.path()).unwrap();
        assert_eq!(k1, k2);
        assert!(dir.path().join("keys/.gitignore").exists());
    }
}
