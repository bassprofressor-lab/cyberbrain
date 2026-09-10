//! The hub role: one machine collects what the others did.
//!
//! # Why this is not `serve` with the address opened up
//!
//! `serve` binds loopback and has no authentication, and SPEC §8.2 ties those two facts
//! together on purpose: there is nothing to authenticate because there is no remote access.
//! It can also read, write, delete and apply retention. Opening its bind would hand all of
//! that to anyone on the network.
//!
//! So the hub is a second surface with its own, much smaller job: it takes audit rows,
//! checks that they continue where the sender left off, and keeps them. It cannot read,
//! write, delete or apply retention on anybody's store.
//!
//! It does hold note text, in one narrow case and never by default. A store that switches on
//! `allow_note_sync` can share the notes carrying a `bereich`, in rings 2 to 4, with the
//! devices that have a countersigned grant for that bereich — see `sync_access` for the rule
//! and `docs/HUB.md` for what it means to somebody who has to explain it. Everything else a
//! client sends is evidence about actions and not content: `subject` arrives as it was
//! written, and a row says that a note was written, never what it said.
//!
//! The distinction is not a comment. `EgressPurpose::AuditSync` and `EgressPurpose::NoteSync`
//! are separate entries in the register, the second carries `carries_note_content: true`, and
//! `cyberbrain policy egress` prints both — so the difference is something an operator reads
//! off their own machine rather than something they take on trust from this paragraph.
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

pub mod access;
pub mod admin;
pub mod api;
pub mod client;
pub mod licence;
pub mod page;
pub mod report;
pub mod service;
pub mod store;
pub mod sync_access;
pub mod tls;

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
/// One note as it arrives over the wire.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WireNote {
    pub id: String,
    pub name: String,
    pub ring: u8,
    pub kind: String,
    pub bereich: Option<String>,
    pub updated: String,
    pub frontmatter: String,
    pub body: String,
    /// The `updated` of the version this device started from, as it last had it from the
    /// hub. Absent means "I had nothing". It is what lets the hub tell a continuation from
    /// two machines that never saw each other, which last-write-wins cannot.
    #[serde(default)]
    pub based_on: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct NoteDelivery {
    pub notes: Vec<WireNote>,
}

/// What became of a delivery of notes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NotesAccepted {
    /// Notes that passed every check.
    pub accepted: usize,
    /// Of those, the ones that were newer than what the hub held.
    pub stored: usize,
    /// Refused notes, each with the reason. A delivery is not all-or-nothing: one note the
    /// sender should not have offered must not strand the rest, and the sender needs to
    /// learn which one it was.
    pub refused: Vec<RefusedNote>,
    /// Notes two machines changed independently. Both versions are held; nothing was
    /// overwritten and nothing was dropped. Somebody has to say which one stands.
    pub conflicts: Vec<ConflictedNote>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConflictedNote {
    pub name: String,
    pub conflict: String,
    /// What the hub holds, which this delivery did not replace.
    pub held_updated: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RefusedNote {
    pub name: String,
    pub why: String,
}

/// What a device gets when it asks what is there for it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Fetched {
    pub notes: Vec<store::SyncedNote>,
    /// Notes withdrawn since `since`, as bereich, name and when. Carried alongside the
    /// notes because "gone" and "not offered to you" look identical otherwise.
    pub erased: Vec<ErasedElsewhere>,
    /// The stamp to pass as `since` next time. The newest thing in this answer, so a puller
    /// never has to reason about clocks.
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ErasedElsewhere {
    pub bereich: String,
    pub name: String,
    pub erased_at: String,
}

/// Hand a device what it may read.
///
/// The hub decides what this device is allowed to see; the device decides what to do with
/// it. Neither half is enough on its own, and keeping them apart is why a hub that is taken
/// over still cannot write into anybody's store.
pub fn fetch_notes(
    hub: &store::HubStore,
    token: Option<&str>,
    since: Option<&str>,
) -> std::result::Result<Fetched, Refusal> {
    let token = token.ok_or_else(|| {
        Refusal::NotAuthorised("no device token; send it as `Authorization: Bearer …`".into())
    })?;
    let device = hub
        .device_by_token(token)
        .map_err(|e| Refusal::NotAuthorised(format!("cannot check the token: {e}")))?
        .ok_or_else(|| Refusal::NotAuthorised("unknown device token".into()))?;
    if !device.is_active() {
        return Err(Refusal::NotAuthorised(format!(
            "device {} was revoked",
            device.name
        )));
    }
    let notes = hub
        .notes_for_device(&device.id, since)
        .map_err(|e| Refusal::BadBundle(format!("cannot read notes: {e}")))?;
    let erased: Vec<ErasedElsewhere> = hub
        .erasures_for_device(&device.id, since)
        .map_err(|e| Refusal::BadBundle(format!("cannot read erasures: {e}")))?
        .into_iter()
        .map(|(bereich, name, erased_at)| ErasedElsewhere {
            bereich,
            name,
            erased_at,
        })
        .collect();
    let cursor = notes
        .iter()
        .map(|n| n.updated.clone())
        .chain(erased.iter().map(|e| e.erased_at.clone()))
        .max();
    Ok(Fetched {
        notes,
        erased,
        cursor,
    })
}

/// A request to erase one note everywhere the hub holds it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct EraseRequest {
    pub bereich: String,
    pub name: String,
}

/// Erase a note on behalf of a device.
///
/// Held to a different rule than a delivery: a device may erase in any bereich it was
/// granted in *either* direction. Withdrawing content is not the same act as sharing it,
/// and a receive-only device that finds something it should not have must be able to say
/// so. What it may never do is erase a bereich it has nothing to do with.
pub fn erase_note(
    hub: &mut store::HubStore,
    token: Option<&str>,
    body: &str,
    now: &str,
) -> std::result::Result<store::ErasureCount, Refusal> {
    let token = token.ok_or_else(|| {
        Refusal::NotAuthorised("no device token; send it as `Authorization: Bearer …`".into())
    })?;
    let device = hub
        .device_by_token(token)
        .map_err(|e| Refusal::NotAuthorised(format!("cannot check the token: {e}")))?
        .ok_or_else(|| Refusal::NotAuthorised("unknown device token".into()))?;
    if !device.is_active() {
        return Err(Refusal::NotAuthorised(format!(
            "device {} was revoked",
            device.name
        )));
    }
    let req: EraseRequest = serde_json::from_str(body)
        .map_err(|e| Refusal::BadBundle(format!("not an erase request: {e}")))?;

    let grants = hub
        .grants_for_device(&device.id)
        .map_err(|e| Refusal::BadBundle(format!("cannot read this device's grants: {e}")))?;
    let allowed = grants
        .iter()
        .any(|g| g.is_effective() && g.bereich == req.bereich);
    if !allowed {
        return Err(Refusal::NotAuthorised(format!(
            "device {} has no grant in bereich {}",
            device.name, req.bereich
        )));
    }

    let count = hub
        .erase_note(&req.bereich, &req.name, &device.id, now)
        .map_err(|e| Refusal::BadBundle(format!("erasure failed: {e}")))?;
    // The audit row says what was removed and from where. It does not say what it said.
    let _ = hub.record(
        &device.id,
        "notes.erased",
        serde_json::json!({
            "bereich": req.bereich,
            "name": req.name,
            "notes_removed": count.notes,
            "conflict_rows_removed": count.conflicts,
        }),
        now,
    );
    Ok(count)
}

/// Take a delivery of notes from a device.
///
/// Every note is checked here even though the sender checked it first. That is not
/// redundancy: the sender's check is a courtesy that keeps traffic down, and this one is
/// the rule. A hub that trusts its clients has no rule at all — it has a client that has
/// not yet been rewritten.
pub fn ingest_notes(
    hub: &mut store::HubStore,
    licence: &LicenceState,
    token: Option<&str>,
    body: &str,
    now: &str,
) -> std::result::Result<NotesAccepted, Refusal> {
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
            "device {} was revoked",
            device.name
        )));
    }

    let delivery: NoteDelivery = serde_json::from_str(body)
        .map_err(|e| Refusal::BadBundle(format!("delivery is not a note batch: {e}")))?;

    let grants = hub
        .grants_for_device(&device.id)
        .map_err(|e| Refusal::BadBundle(format!("cannot read this device's grants: {e}")))?;

    let mut out = NotesAccepted {
        accepted: 0,
        stored: 0,
        refused: Vec::new(),
        conflicts: Vec::new(),
    };
    for n in delivery.notes {
        let ring = match cyberbrain_core::Ring::try_from(n.ring) {
            Ok(r) => r,
            Err(e) => {
                out.refused.push(RefusedNote {
                    name: n.name,
                    why: format!("ring {}: {e}", n.ring),
                });
                continue;
            }
        };
        if let Err(denied) = sync_access::may_move(
            &device.id,
            ring,
            n.bereich.as_deref(),
            sync_access::Direction::Send,
            &grants,
        ) {
            out.refused.push(RefusedNote {
                name: n.name,
                why: denied.line(),
            });
            continue;
        }
        // `may_move` has already refused rings 0 and 1; this unwrap of the bereich is what
        // it guarantees, and the table's CHECK is the third place the same rule is stated.
        let Some(bereich) = n.bereich.as_deref() else {
            out.refused.push(RefusedNote {
                name: n.name,
                why: "no bereich".into(),
            });
            continue;
        };
        // A machine that still has its own copy must not be able to put back what somebody
        // asked to have removed. Checked before the note is looked at, not after.
        match hub.erased_at(bereich, &n.name) {
            Ok(Some(when)) => {
                out.refused.push(RefusedNote {
                    name: n.name,
                    why: format!(
                        "erased at {when}; delete your copy rather than re-offering it \
                         (GDPR Art. 17)"
                    ),
                });
                continue;
            }
            Ok(None) => {}
            Err(e) => {
                out.refused.push(RefusedNote {
                    name: n.name,
                    why: format!("cannot check whether it was erased: {e}"),
                });
                continue;
            }
        }
        match hub.offer_synced_note(
            &n.id,
            bereich,
            &n.name,
            n.ring,
            &n.kind,
            &n.updated,
            &n.frontmatter,
            &n.body,
            n.based_on.as_deref(),
            &device.id,
            now,
        ) {
            Ok(store::NoteOutcome::Stored) => {
                out.accepted += 1;
                out.stored += 1;
            }
            Ok(store::NoteOutcome::Unchanged) => out.accepted += 1,
            Ok(store::NoteOutcome::Conflict { id, held_updated }) => {
                out.accepted += 1;
                out.conflicts.push(ConflictedNote {
                    name: n.name,
                    conflict: id,
                    held_updated,
                });
            }
            Err(e) => out.refused.push(RefusedNote {
                name: n.name,
                why: format!("could not be stored: {e}"),
            }),
        }
    }

    let _ = hub.record(
        &device.id,
        "notes.ingested",
        serde_json::json!({
            "device": device.name,
            "accepted": out.accepted,
            "stored": out.stored,
            "refused": out.refused.len(),
            "conflicts": out.conflicts.len(),
        }),
        now,
    );
    Ok(out)
}

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

/// Where the hub keeps the fingerprint of the key it is currently serving with.
const PIN_SETTING: &str = "tls_cert_sha256";

/// Remember what an invitation should pin, or forget it.
///
/// The hub serves and `hub add` issues invitations, and those are two processes that never
/// meet: the only thing they share is this record. Written on every start rather than once,
/// so it describes the hub that is running now and not the one that ran in March.
pub fn remember_pin(hub: &HubStore, pin: Option<&str>) -> Result<()> {
    match pin {
        Some(p) => hub.set_setting(PIN_SETTING, p),
        None => hub.clear_setting(PIN_SETTING),
    }
}

/// What an invitation should pin, if this hub serves with a key it knows the fingerprint of.
pub fn pin_to_offer(hub: &HubStore) -> Option<String> {
    hub.setting(PIN_SETTING)
        .ok()
        .flatten()
        .filter(|p| !p.is_empty())
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
