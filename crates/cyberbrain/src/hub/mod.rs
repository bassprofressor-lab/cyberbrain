//! The hub role: one machine collects what the others did.
//!
//! # Why this is not `serve` with the address opened up
//!
//! `serve` binds loopback and has no authentication, and SPEC §8.2 ties those two facts
//! together on purpose: there is nothing to authenticate because there is no remote access.
//! It can also read, write, delete and apply retention. Opening its bind would hand all of
//! that to anyone on the network.
//!
//! So the hub is a second surface with its own, much smaller job. It cannot read a note, it
//! has no notes: it takes audit rows, checks that they continue where the sender left off,
//! and keeps them. Everything a client sends is evidence about actions, never content —
//! `subject` arrives as it was written, and what a note *said* never leaves the machine that
//! holds it.
//!
//! # What the anchor does here
//!
//! Each device's rows form one chain. The hub remembers the hash of the last row it accepted
//! from that device, and the next bundle has to anchor exactly there. That single comparison
//! is the whole gap detection: a bundle that skips a period does not anchor, and a bundle
//! replayed twice does not either.

use cyberbrain_core::{Error, Result};
use cyberbrain_policy::bundle;
use std::path::PathBuf;

pub mod api;
pub mod store;

#[cfg(test)]
mod tests;

#[cfg_attr(not(test), allow(unused_imports))]
pub use store::{Device, HubStore};

/// Where the hub keeps its record, unless told otherwise.
pub const DEFAULT_DATA: &str = "cyberbrain-hub.db";

/// The outcome of one delivery, as the sender is told it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Accepted {
    pub device: String,
    /// Rows taken from this delivery. Zero is a valid answer: an empty period is a fact.
    pub accepted: usize,
    /// What the next bundle from this device has to anchor at.
    pub next_anchor: String,
    pub total_rows: i64,
}

/// Why a delivery was refused. Kept apart from the message so the caller can map it to a
/// status code without matching on prose.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// No token, an unknown one, or one belonging to a revoked device.
    NotAuthorised(String),
    /// The file is not a valid bundle, or its chain does not hold.
    BadBundle(String),
    /// The chain is fine but does not continue this device's.
    WrongAnchor { expected: String, got: String },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NotAuthorised(m) => write!(f, "{m}"),
            Refusal::BadBundle(m) => write!(f, "{m}"),
            Refusal::WrongAnchor { expected, got } => write!(
                f,
                "this device's chain is at {expected}, the delivery starts at {got}: \
                 something is missing between them, or this was already delivered"
            ),
        }
    }
}

/// Take one delivery: authenticate, verify, check that it continues, append.
///
/// The order matters and is the order a reader would want it in. Authentication first,
/// because an unknown sender's file is not worth parsing. The bundle's own integrity next,
/// because a broken file tells the sender something different from a gap. The anchor last,
/// because that comparison is only meaningful once the chain inside the file is known good.
pub fn ingest(
    hub: &mut HubStore,
    token: Option<&str>,
    body: &str,
    version: Option<&str>,
    now: &str,
) -> std::result::Result<Accepted, Refusal> {
    let token = token.ok_or_else(|| {
        Refusal::NotAuthorised("no device token; send it as `Authorization: Bearer …`".into())
    })?;
    let device = hub
        .device_by_token(token)
        .map_err(|e| Refusal::NotAuthorised(format!("cannot check the token: {e}")))?
        .ok_or_else(|| Refusal::NotAuthorised("unknown device token".into()))?;
    if !device.is_active() {
        return Err(Refusal::NotAuthorised(format!(
            "device {} was revoked; its record is kept, but it may not send",
            device.id
        )));
    }

    let (report, rows) =
        bundle::verify_rows(body).map_err(|e| Refusal::BadBundle(e.to_string()))?;

    // Where in this delivery does the part the hub does not have yet begin?
    //
    // A sender does not know the hub's anchor, and asking it first would make every delivery
    // two round trips and still race. So overlap is the normal case, not an error: a client
    // that resends the last week after a bad connection is behaving correctly. What must
    // never pass is a *gap*, and the difference between the two is exactly whether this
    // delivery contains the row the hub stopped at.
    let fresh: &[cyberbrain_policy::AuditEvent] = if report.anchor == device.anchor {
        &rows
    } else if let Some(i) = rows
        .iter()
        .position(|r| r.chain_hash() == Some(device.anchor.as_str()))
    {
        &rows[i + 1..]
    } else {
        return Err(Refusal::WrongAnchor {
            expected: device.anchor.clone(),
            got: report.anchor,
        });
    };

    // An empty delivery still counts as contact: it moves `last_seen`, so a device that has
    // nothing to say is visibly different from one that has stopped saying anything. The
    // same holds for a delivery that turned out to be entirely overlap.
    let new_anchor = fresh
        .last()
        .and_then(|r| r.chain_hash())
        .unwrap_or(&device.anchor)
        .to_string();
    let total = hub
        .append(&device, fresh, &new_anchor, version, now)
        .map_err(|e| Refusal::BadBundle(format!("could not store the delivery: {e}")))?;

    Ok(Accepted {
        device: device.id,
        accepted: fresh.len(),
        next_anchor: new_anchor,
        total_rows: total,
    })
}

/// Path of the hub's record, from the flag or the default.
pub fn data_path(explicit: Option<PathBuf>) -> PathBuf {
    explicit.unwrap_or_else(|| PathBuf::from(DEFAULT_DATA))
}

/// The address the hub listens on.
///
/// Unlike `serve` this one is a parameter, because a hub that only its own machine can reach
/// is not a hub. That is exactly why the surface it exposes is small and authenticated, and
/// why the two are different commands rather than a flag on one.
pub fn parse_addr(s: &str) -> Result<std::net::SocketAddr> {
    s.parse()
        .map_err(|e| Error::Config(format!("--addr {s:?} is not an address:port pair: {e}")))
}
