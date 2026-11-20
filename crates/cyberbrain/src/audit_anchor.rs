//! The head anchor of a store's audit log, kept outside the store (S3, audits 2026-11-10 and
//! 2026-11-19).
//!
//! The hash chain shows that no row *inside* the log was changed, removed or reordered. It
//! cannot show that rows are missing *at the end*: cut the last three rows off, or put back
//! an older copy of `audit.db`, and what remains is a perfectly good, shorter chain. That
//! was nachgestellt twice ("chain holds over 5 row(s)" after three rows were cut).
//!
//! The anchor is the answer that needs no other machine: after every append to the main log,
//! the number of the row and its hash are written to a small file in the user's state
//! directory, not in the store, and signed with the store's audit key when it has one.
//! `policy audit --verify` then asks two things the chain alone cannot answer: is the log
//! at least as long as the anchor says, and is the row the anchor names still the row it
//! named. A store copied from a backup, a repository rolled back, a share somebody else
//! writes to: each of those leaves the anchor behind and is found.
//!
//! What it does **not** do, said here so nobody reads more into it: a process running as
//! the same user can rewrite the anchor file as easily as the log. Against that only a
//! record on another machine helps (the hub, a timestamping service). See SECURITY.md.

use cyberbrain_core::{Error, Result};
use cyberbrain_policy::AuditKey;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Points the anchors somewhere else. Tests use it; so can an operator who keeps the
/// anchors on a different disk.
pub const ENV: &str = "CYBERBRAIN_AUDIT_ANCHORS";

pub const KIND: &str = "cyberbrain.audit.head";
/// Signature domain of an anchor. Distinct from the row and bundle domains, so a signature
/// made for one can never be presented as one of the others.
pub const DOMAIN: &str = "cyberbrain.audit.head.v1\n";

/// The directory the anchors live in, or `None` when there is no home to put them in.
pub fn dir() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(ENV) {
        return Some(PathBuf::from(p));
    }
    // Unit tests never write into the real home directory.
    if cfg!(test) {
        return Some(std::env::temp_dir().join("cyberbrain-test-anchors"));
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state"))
            })
    }?;
    Some(base.join("cyberbrain").join("anchors"))
}

/// The anchor file of the store at `root`: named by its canonical path, so a copy of the
/// store elsewhere has an anchor of its own and a store put back at the same place meets
/// the anchor it left.
pub fn path_for(root: &Path) -> Option<PathBuf> {
    let key = cyberbrain_core::waivers::store_key(root);
    let name = blake3::hash(key.as_bytes()).to_hex();
    Some(dir()?.join(format!("{}.json", &name[..32])))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Anchor {
    pub kind: String,
    /// The store, as its canonical path. Informational: the file name is what finds it.
    pub store: String,
    /// The row number (`seq`) of the newest row this anchor saw.
    pub row: i64,
    /// That row's chain hash.
    pub hash: String,
    /// That row's chain timestamp.
    pub at: String,
    /// The public key (hex) the anchor is signed with, when the store has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

impl Anchor {
    fn signed_text(&self) -> String {
        format!(
            "{}\n{}\n{}\n{}\n{}\n",
            KIND, self.store, self.row, self.hash, self.at
        )
    }
}

/// The anchor of the store at `root`, if one was written. A file that is there but does not
/// parse is an error: it is the one thing an attacker who could not forge it would do.
pub fn load(root: &Path) -> Result<Option<Anchor>> {
    let Some(path) = path_for(root) else {
        return Ok(None);
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| {
            Error::Index(format!(
                "the audit head anchor {} does not parse ({e}); it was edited or damaged",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::Io { path, source: e }),
    }
}

/// Move the anchor forward to row `row`. Never backwards: two processes appending at once
/// may finish their anchors out of order, and the one with the lower row must not undo the
/// other. Losing that race costs the anchor one row of reach, never a false alarm.
pub fn advance(root: &Path, row: i64, hash: &str, at: &str, key: Option<&AuditKey>) -> Result<()> {
    let Some(path) = path_for(root) else {
        return Ok(());
    };
    if let Ok(Some(cur)) = load(root)
        && cur.row >= row
    {
        return Ok(());
    }
    let mut anchor = Anchor {
        kind: KIND.to_string(),
        store: cyberbrain_core::waivers::store_key(root),
        row,
        hash: hash.to_string(),
        at: at.to_string(),
        key: key.map(AuditKey::public_hex),
        sig: None,
    };
    anchor.sig = key.map(|k| k.sign(DOMAIN, anchor.signed_text().as_bytes()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
    }
    let text = serde_json::to_string(&anchor).expect("anchor serialises") + "\n";
    crate::hub::client::write_owner_only(&path, &text).map_err(|e| Error::Io { path, source: e })
}

/// Check the whole log against the anchor. `hashes` are the chain hashes of every row,
/// oldest first, as `(seq, hash)`. `own_key` is the store's public key, when it has one.
pub fn check(anchor: &Anchor, rows: &[(i64, String)], own_key: Option<&str>) -> Result<()> {
    let broken = |why: String| Error::Index(format!("audit head anchor: {why}"));
    if anchor.kind != KIND {
        return Err(broken(format!("{:?} is not an anchor", anchor.kind)));
    }
    match (&anchor.key, &anchor.sig, own_key) {
        (Some(k), Some(sig), Some(own)) => {
            if k != own {
                return Err(broken(
                    "it is signed with another key than this store's; the key was replaced".into(),
                ));
            }
            cyberbrain_policy::audit_key::verify_signature(
                k,
                DOMAIN,
                anchor.signed_text().as_bytes(),
                sig,
            )
            .map_err(|e| broken(format!("{e}; the anchor file was edited")))?;
        }
        (Some(_), None, _) | (None, Some(_), _) => {
            return Err(broken("key and signature come only together".into()));
        }
        // Signed, and the store has no key any more: the key went, not the anchor.
        (Some(_), Some(_), None) => {
            return Err(broken(
                "it is signed, but this store has no audit key any more".into(),
            ));
        }
        // An anchor written before the store had a key. The rows after it are signed and
        // say so themselves; the anchor still counts for its row.
        (None, None, _) => {}
    }
    let last = rows.last().map(|(s, _)| *s).unwrap_or(0);
    if last < anchor.row {
        return Err(broken(format!(
            "the log ends at row {last}, but row {} was written on {} ({}); rows were removed \
             from the end, or an older copy of audit.db was put back",
            anchor.row, anchor.at, anchor.hash
        )));
    }
    match rows.iter().find(|(s, _)| *s == anchor.row) {
        Some((_, h)) if *h == anchor.hash => Ok(()),
        Some((_, h)) => Err(broken(format!(
            "row {} has hash {h}, but the anchor recorded {} for it on {}; the log was \
             rewritten from that row on, or another copy was put in its place",
            anchor.row, anchor.hash, anchor.at
        ))),
        None => Err(broken(format!(
            "row {} is no longer in the log (written on {}); rows were removed",
            anchor.row, anchor.at
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: i64) -> Vec<(i64, String)> {
        (1..=n).map(|i| (i, format!("h{i}"))).collect()
    }

    fn anchor(row: i64, hash: &str, key: Option<&AuditKey>) -> Anchor {
        let mut a = Anchor {
            kind: KIND.into(),
            store: "/s".into(),
            row,
            hash: hash.into(),
            at: "2026-11-20T00:00:00Z".into(),
            key: key.map(AuditKey::public_hex),
            sig: None,
        };
        a.sig = key.map(|k| k.sign(DOMAIN, a.signed_text().as_bytes()));
        a
    }

    #[test]
    fn a_log_that_still_holds_the_anchored_row_passes() {
        assert!(check(&anchor(5, "h5", None), &rows(8), None).is_ok());
        assert!(check(&anchor(8, "h8", None), &rows(8), None).is_ok());
    }

    #[test]
    fn a_log_cut_at_the_end_is_found() {
        let e = check(&anchor(8, "h8", None), &rows(5), None).unwrap_err();
        assert!(e.to_string().contains("removed from the end"), "{e}");
    }

    #[test]
    fn a_rewritten_row_is_found() {
        let e = check(&anchor(5, "other", None), &rows(8), None).unwrap_err();
        assert!(e.to_string().contains("rewritten"), "{e}");
    }

    #[test]
    fn a_signed_anchor_must_carry_this_stores_key_and_hold() {
        let k = AuditKey::generate().unwrap();
        let other = AuditKey::generate().unwrap();
        let a = anchor(5, "h5", Some(&k));
        assert!(check(&a, &rows(8), Some(&k.public_hex())).is_ok());
        assert!(check(&a, &rows(8), Some(&other.public_hex())).is_err());
        let mut edited = a.clone();
        edited.row = 3;
        edited.hash = "h3".into();
        let e = check(&edited, &rows(8), Some(&k.public_hex())).unwrap_err();
        assert!(e.to_string().contains("edited"), "{e}");
    }

    #[test]
    fn the_anchor_only_moves_forward() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        std::fs::create_dir_all(&root).unwrap();
        advance(&root, 7, "h7", "t7", None).unwrap();
        advance(&root, 4, "h4", "t4", None).unwrap();
        assert_eq!(load(&root).unwrap().unwrap().row, 7);
        advance(&root, 9, "h9", "t9", None).unwrap();
        assert_eq!(load(&root).unwrap().unwrap().hash, "h9");
        let _ = std::fs::remove_file(path_for(&root).unwrap());
    }
}
