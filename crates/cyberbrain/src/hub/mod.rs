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
pub mod client;
pub mod licence;
pub mod report;
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

/// What the licence says about this moment, for every caller that has to act on it.
///
/// One place decides, so the CLI, the API and the fleet view cannot drift into three
/// different opinions about whether a hub may still collect.
#[derive(Debug, Clone, PartialEq)]
pub enum LicenceState {
    /// No licence installed. The hub runs read-only from the start.
    Missing,
    /// Installed and unreadable, or signed by somebody else.
    Invalid(String),
    /// Good, with a warning to show when the end is close.
    Valid {
        customer: String,
        seats: usize,
        valid_until: String,
        warning: Option<String>,
    },
    /// Past its end date. Collection stops; nothing else does.
    Expired {
        customer: String,
        valid_until: String,
    },
}

impl LicenceState {
    /// Read the installed licence and judge it against `now`.
    pub fn read(hub: &HubStore, now: jiff::Timestamp) -> Self {
        let Ok(Some(text)) = hub.licence_text() else {
            return LicenceState::Missing;
        };
        match licence::parse(&text) {
            Err(e) => LicenceState::Invalid(e.to_string()),
            Ok(l) if l.not_yet_valid(now) => LicenceState::Invalid(format!(
                "the licence for {} does not start until {}",
                l.licence().customer,
                l.licence().valid_from
            )),
            Ok(l) if l.expired(now) => LicenceState::Expired {
                customer: l.licence().customer.clone(),
                valid_until: l.licence().valid_until.clone(),
            },
            Ok(l) => LicenceState::Valid {
                customer: l.licence().customer.clone(),
                seats: l.licence().seats,
                valid_until: l.licence().valid_until.clone(),
                warning: l.warning(now),
            },
        }
    }

    /// May the hub take new rows right now?
    pub fn may_collect(&self) -> bool {
        matches!(self, LicenceState::Valid { .. })
    }

    /// Seats, when there are any. `None` means no limit is in force because no licence is.
    pub fn seats(&self) -> Option<usize> {
        match self {
            LicenceState::Valid { seats, .. } => Some(*seats),
            _ => None,
        }
    }

    /// One line for a person, always — including "everything is fine", because a status
    /// display that says nothing when things are good teaches people to ignore it.
    pub fn line(&self) -> String {
        match self {
            LicenceState::Missing => concat!(
                "no licence installed: the hub will not accept rows. ",
                "Install one with `cyberbrain hub licence install <file>`."
            )
            .to_string(),
            LicenceState::Invalid(why) => format!("licence not usable: {why}"),
            LicenceState::Expired {
                customer,
                valid_until,
            } => format!(
                concat!(
                    "licence for {} ended on {}. New rows are not accepted; the record ",
                    "stays readable and exportable, and clients keep buffering."
                ),
                customer, valid_until
            ),
            LicenceState::Valid {
                customer,
                seats,
                valid_until,
                warning,
            } => match warning {
                Some(w) => format!("⚠ {w}"),
                None => format!("licence: {customer}, {seats} seat(s), until {valid_until}"),
            },
        }
    }
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
    /// The licence does not currently allow collecting. Nothing is wrong with the delivery.
    NotCollecting(String),
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
            Refusal::NotCollecting(m) => write!(f, "{m}"),
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
    licence: &LicenceState,
    token: Option<&str>,
    body: &str,
    version: Option<&str>,
    now: &str,
) -> std::result::Result<Accepted, Refusal> {
    // The licence is judged by the caller and handed in. Deciding whether a hub may collect
    // and deciding whether one delivery is sound are two jobs, and a function that does both
    // could only be tested by whoever holds the issuer's private key.
    //
    // It is checked before anything else, because the answer is the same for every sender
    // and is not about them — and the wording matters: a sender that is told "not now"
    // should keep what it has rather than throw it away.
    if !licence.may_collect() {
        return Err(Refusal::NotCollecting(format!(
            "{} Keep buffering: nothing is lost, and a renewed licence takes what you held.",
            licence.line()
        )));
    }

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

    // From here on the sender is known, so a refusal can be recorded against it. Anything
    // above this line has no device to record against, which is also why it cannot be shown
    // in the fleet view: an unknown token is not a device having trouble.
    let note = |r: Refusal| -> Refusal {
        let _ = hub.note_refusal(&device.id, &r.to_string(), now);
        r
    };

    let (report, rows) =
        bundle::verify_rows(body).map_err(|e| note(Refusal::BadBundle(e.to_string())))?;

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
        return Err(note(Refusal::WrongAnchor {
            expected: device.anchor.clone(),
            got: report.anchor,
        }));
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
