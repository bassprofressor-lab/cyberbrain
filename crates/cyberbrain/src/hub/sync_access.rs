//! Who may send which bereich to whom, and why.
//!
//! Schnitt 1 of the note sync (entwurf 2026-09-10). This module decides eligibility; it
//! moves nothing. The transport is deliberately a later cut, so the rule that governs a
//! note leaving a machine exists before the first byte of note content can.
//!
//! Two properties are of the code and not of a grant, because a configuration that can
//! turn them on is not a guarantee:
//!
//!   * Rings 0 and 1 never leave the machine. Invariants and operating protocol belong to
//!     the operator of each host. A hub that could place them is a hub that decides how
//!     every agent in the company behaves.
//!   * A note without a bereich is never eligible. Nothing to check against is not the
//!     same as permission, and a sync that treats it as one leaks by default.

use cyberbrain_core::{Error, Result, Ring};
use serde::Serialize;

/// Which way a grant runs. Held apart because sending and receiving are different risks:
/// receiving admits foreign content, sending discloses your own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Send,
    Receive,
    Both,
}

impl Direction {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "send" => Ok(Direction::Send),
            "receive" => Ok(Direction::Receive),
            "both" => Ok(Direction::Both),
            other => Err(Error::Config(format!(
                "direction {other:?} does not exist; use send, receive or both"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Send => "send",
            Direction::Receive => "receive",
            Direction::Both => "both",
        }
    }

    fn covers(self, wanted: Direction) -> bool {
        self == Direction::Both || self == wanted
    }
}

/// One device's permission for one bereich.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BereichGrant {
    pub id: String,
    /// The device this applies to, from `devices.id`.
    pub device: String,
    pub bereich: String,
    pub direction: Direction,
    /// Why this grant exists. Not decoration: under Art. 5(1)(b) GDPR a department
    /// boundary is a purpose limitation, and a purpose nobody wrote down cannot be shown
    /// to a supervisory authority later.
    pub reason: String,
    /// The principal who granted it. Must hold `Role::Admin`.
    pub granted_by: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

impl BereichGrant {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

/// Why a note may not move. Every variant names the thing that would have to change, so a
/// refusal is actionable rather than a wall.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "denied", rename_all = "kebab-case")]
pub enum SyncDenied {
    /// Ring 0 or 1. Not a missing grant: no grant can exist for these.
    RingNeverLeaves { ring: u8 },
    /// The note carries no bereich, so there is nothing to check a grant against.
    NoteHasNoBereich,
    /// No grant at all for this device and bereich.
    NoGrant { device: String, bereich: String },
    /// A grant exists but runs the other way.
    WrongDirection {
        device: String,
        bereich: String,
        granted: Direction,
        wanted: Direction,
    },
    /// A grant existed and was withdrawn.
    Revoked {
        device: String,
        bereich: String,
        at: String,
    },
}

impl SyncDenied {
    /// One line, for a log or a terminal. Says what was refused and what would fix it.
    pub fn line(&self) -> String {
        match self {
            SyncDenied::RingNeverLeaves { ring } => format!(
                "ring {ring} never leaves the machine: invariants and protocol belong to the \
                 operator of each host, and no grant can permit them"
            ),
            SyncDenied::NoteHasNoBereich => {
                "the note carries no bereich, so no grant can apply to it; set one with \
                 `--bereich` before it can be shared"
                    .into()
            }
            SyncDenied::NoGrant { device, bereich } => format!(
                "device {device} has no grant for bereich {bereich}: \
                 `cyberbrain hub grant --device {device} --bereich {bereich} \
                 --direction … --reason …`"
            ),
            SyncDenied::WrongDirection {
                device,
                bereich,
                granted,
                wanted,
            } => format!(
                "device {device} may {} bereich {bereich}, not {}",
                granted.as_str(),
                wanted.as_str()
            ),
            SyncDenied::Revoked {
                device,
                bereich,
                at,
            } => format!("the grant of {bereich} to device {device} was withdrawn at {at}"),
        }
    }
}

/// May this device move a note of this ring and bereich, in this direction?
///
/// `grants` is every grant recorded for the device, revoked ones included: a withdrawn
/// grant must be reported as withdrawn rather than as absent, because the two call for
/// different answers from whoever reads the refusal.
pub fn may_move(
    device: &str,
    ring: Ring,
    bereich: Option<&str>,
    wanted: Direction,
    grants: &[BereichGrant],
) -> std::result::Result<(), SyncDenied> {
    // Checked before anything else, and never against a grant: see the module note.
    if matches!(ring, Ring::Invariant | Ring::Protocol) {
        return Err(SyncDenied::RingNeverLeaves { ring: ring.as_u8() });
    }
    let Some(bereich) = bereich else {
        return Err(SyncDenied::NoteHasNoBereich);
    };
    let mine: Vec<&BereichGrant> = grants
        .iter()
        .filter(|g| g.device == device && g.bereich == bereich)
        .collect();
    if mine.is_empty() {
        return Err(SyncDenied::NoGrant {
            device: device.into(),
            bereich: bereich.into(),
        });
    }
    if let Some(g) = mine
        .iter()
        .find(|g| g.is_active() && g.direction.covers(wanted))
    {
        let _ = g;
        return Ok(());
    }
    // Nothing active fits. Say which of the two reasons it is.
    if let Some(g) = mine.iter().find(|g| g.is_active()) {
        return Err(SyncDenied::WrongDirection {
            device: device.into(),
            bereich: bereich.into(),
            granted: g.direction,
            wanted,
        });
    }
    let newest = mine
        .iter()
        .filter_map(|g| g.revoked_at.clone())
        .max()
        .unwrap_or_else(|| "an unknown time".into());
    Err(SyncDenied::Revoked {
        device: device.into(),
        bereich: bereich.into(),
        at: newest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(device: &str, bereich: &str, dir: Direction) -> BereichGrant {
        BereichGrant {
            id: format!("g-{device}-{bereich}"),
            device: device.into(),
            bereich: bereich.into(),
            direction: dir,
            reason: "Schichtuebergabe innerhalb der Abteilung".into(),
            granted_by: "admin-1".into(),
            created_at: "2026-09-10T12:00:00Z".into(),
            revoked_at: None,
        }
    }

    /// The rule the whole cut exists for: rings 0 and 1 are refused *even when a grant says
    /// otherwise*. If this ever passes, a hub can rewrite what every agent treats as an
    /// invariant, and no amount of transport correctness makes up for it.
    #[test]
    fn no_grant_can_send_ring_0_or_1() {
        let generous = vec![
            grant("laptop-a", "disposition", Direction::Both),
            grant("laptop-a", "hr", Direction::Both),
        ];
        for ring in [Ring::Invariant, Ring::Protocol] {
            let r = may_move(
                "laptop-a",
                ring,
                Some("disposition"),
                Direction::Send,
                &generous,
            );
            assert_eq!(
                r,
                Err(SyncDenied::RingNeverLeaves { ring: ring.as_u8() }),
                "ring {ring:?} was allowed to leave"
            );
        }
        // And the rings that may travel still do.
        for ring in [Ring::Knowledge, Ring::Session, Ring::External] {
            assert!(
                may_move(
                    "laptop-a",
                    ring,
                    Some("disposition"),
                    Direction::Send,
                    &generous
                )
                .is_ok(),
                "ring {ring:?} should be eligible"
            );
        }
    }

    /// The operator's own example: within the department yes, across to HR no.
    #[test]
    fn disposition_reaches_disposition_but_not_hr() {
        let grants = vec![
            grant("mitarbeiter-1", "disposition", Direction::Both),
            grant("mitarbeiter-2", "disposition", Direction::Both),
            grant("mitarbeiter-3", "hr", Direction::Both),
        ];
        // 1 -> 2, both in disposition.
        assert!(
            may_move(
                "mitarbeiter-1",
                Ring::Knowledge,
                Some("disposition"),
                Direction::Send,
                &grants
            )
            .is_ok()
        );
        assert!(
            may_move(
                "mitarbeiter-2",
                Ring::Knowledge,
                Some("disposition"),
                Direction::Receive,
                &grants
            )
            .is_ok()
        );
        // 3 is HR and may not receive a disposition note.
        assert_eq!(
            may_move(
                "mitarbeiter-3",
                Ring::Knowledge,
                Some("disposition"),
                Direction::Receive,
                &grants
            ),
            Err(SyncDenied::NoGrant {
                device: "mitarbeiter-3".into(),
                bereich: "disposition".into(),
            })
        );
    }

    /// Nothing to check against is not permission.
    #[test]
    fn a_note_without_a_bereich_never_moves() {
        let grants = vec![grant("laptop-a", "disposition", Direction::Both)];
        assert_eq!(
            may_move("laptop-a", Ring::Knowledge, None, Direction::Send, &grants),
            Err(SyncDenied::NoteHasNoBereich)
        );
    }

    /// Sending and receiving are separate risks, so a one-way grant stays one-way.
    #[test]
    fn a_send_grant_does_not_let_anything_in() {
        let grants = vec![grant("laptop-a", "disposition", Direction::Send)];
        assert!(
            may_move(
                "laptop-a",
                Ring::Knowledge,
                Some("disposition"),
                Direction::Send,
                &grants
            )
            .is_ok()
        );
        assert_eq!(
            may_move(
                "laptop-a",
                Ring::Knowledge,
                Some("disposition"),
                Direction::Receive,
                &grants
            ),
            Err(SyncDenied::WrongDirection {
                device: "laptop-a".into(),
                bereich: "disposition".into(),
                granted: Direction::Send,
                wanted: Direction::Receive,
            })
        );
    }

    /// Withdrawn must not read as never-granted: one is an answer to "ask an admin", the
    /// other to "somebody decided against this".
    #[test]
    fn a_withdrawn_grant_says_so() {
        let mut g = grant("laptop-a", "disposition", Direction::Both);
        g.revoked_at = Some("2026-09-10T13:00:00Z".into());
        let r = may_move(
            "laptop-a",
            Ring::Knowledge,
            Some("disposition"),
            Direction::Send,
            &[g],
        );
        assert_eq!(
            r,
            Err(SyncDenied::Revoked {
                device: "laptop-a".into(),
                bereich: "disposition".into(),
                at: "2026-09-10T13:00:00Z".into(),
            })
        );
        assert!(r.unwrap_err().line().contains("withdrawn"));
    }

    /// A revoked grant next to a live one must not shadow it.
    #[test]
    fn a_live_grant_wins_over_a_revoked_one() {
        let mut old = grant("laptop-a", "disposition", Direction::Both);
        old.revoked_at = Some("2026-09-01T00:00:00Z".into());
        let new = grant("laptop-a", "disposition", Direction::Both);
        assert!(
            may_move(
                "laptop-a",
                Ring::Knowledge,
                Some("disposition"),
                Direction::Send,
                &[old, new]
            )
            .is_ok()
        );
    }

    /// Every refusal has to name what would fix it; a wall without a door gets worked
    /// around instead of understood.
    #[test]
    fn every_refusal_is_actionable() {
        let cases = [
            SyncDenied::RingNeverLeaves { ring: 0 },
            SyncDenied::NoteHasNoBereich,
            SyncDenied::NoGrant {
                device: "d".into(),
                bereich: "b".into(),
            },
            SyncDenied::WrongDirection {
                device: "d".into(),
                bereich: "b".into(),
                granted: Direction::Send,
                wanted: Direction::Receive,
            },
            SyncDenied::Revoked {
                device: "d".into(),
                bereich: "b".into(),
                at: "t".into(),
            },
        ];
        for c in cases {
            let l = c.line();
            assert!(l.len() > 30, "refusal too terse: {l}");
            assert!(!l.ends_with("refused."), "refusal says nothing useful: {l}");
        }
    }
}
