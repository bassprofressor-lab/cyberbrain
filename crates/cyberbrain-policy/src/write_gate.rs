//! The write-time hold (SPEC §12.4).
//!
//! Before a note is written, [`check_write`] scans the body. Under `eu` and `ch` a body with
//! findings is **held**: the caller shows the findings to the operator and comes back with
//! an [`OperatorChoice`], which [`resolve_hold`] turns into the body to write and the
//! [`PiiState`] to stamp. Under `off` the scan does not run and the note is stamped
//! [`PiiState::Unscanned`], which is the truth: nobody looked.
//!
//! The scan is heuristic (see [`crate::pii`]). A `Clean` verdict means the detectors found
//! nothing, not that there is nothing. Callers must not turn `PiiState::None` into a claim.

use crate::pii::{self, Finding};
use crate::profile::{Profile, ProfileExt};
use cyberbrain_core::PiiState;
use serde::{Deserialize, Serialize};

/// What the scan did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ScanStatus {
    /// The scan did not run. `reason` says why; today only "profile is off".
    Skipped { reason: String },
    /// The detectors found nothing. Not a guarantee.
    Clean,
    /// The detectors found `count` things and the write is held.
    Findings { count: usize },
}

/// The gate's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum WriteVerdict {
    /// Write the body as given and stamp `pii`.
    Proceed { status: ScanStatus, pii: PiiState },
    /// Do not write yet. Show `findings` and call [`resolve_hold`].
    Held { findings: Vec<Finding> },
}

/// Scan `body` under `profile`. One enum comparison under `off`; no regex is compiled.
pub fn check_write(profile: Profile, body: &str) -> WriteVerdict {
    if profile.is_off() {
        return WriteVerdict::Proceed {
            status: ScanStatus::Skipped {
                reason: "profile is off; PII scan disabled".into(),
            },
            pii: PiiState::Unscanned,
        };
    }
    let findings = pii::scan(body);
    if findings.is_empty() {
        return WriteVerdict::Proceed {
            status: ScanStatus::Clean,
            pii: PiiState::None,
        };
    }
    debug_assert!(profile.holds_pii_writes());
    WriteVerdict::Held { findings }
}

/// The three options SPEC §12.4 gives the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperatorChoice {
    /// Replace every finding with a placeholder, then write.
    Redact,
    /// Write as is; the findings were seen and accepted.
    MarkReviewed,
    /// Write as is; the findings stand unaddressed and the note says so.
    ProceedFlagged,
}

impl OperatorChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            OperatorChoice::Redact => "redact",
            OperatorChoice::MarkReviewed => "mark-reviewed",
            OperatorChoice::ProceedFlagged => "proceed-flagged",
        }
    }
}

/// What to write after the operator decided.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedWrite {
    pub body: String,
    pub pii: PiiState,
    pub choice: OperatorChoice,
    /// Findings replaced by placeholders. Zero unless `Redact`.
    pub redacted: usize,
    /// Findings still present in `body` after the choice was applied. For `Redact` this is
    /// a rescan of the redacted text and should be empty; for the other two it is the
    /// original list.
    pub remaining: Vec<Finding>,
}

/// Apply the operator's choice to a held write.
pub fn resolve_hold(body: &str, findings: &[Finding], choice: OperatorChoice) -> ResolvedWrite {
    match choice {
        OperatorChoice::Redact => {
            let (redacted_body, n) = pii::redact(body, findings);
            let remaining = pii::scan(&redacted_body);
            // Redacted and re-scanned clean: the operator saw the findings and they are
            // gone. Reviewed, not None, because the history matters.
            let pii = if remaining.is_empty() {
                PiiState::Reviewed
            } else {
                PiiState::Flagged
            };
            ResolvedWrite {
                body: redacted_body,
                pii,
                choice,
                redacted: n,
                remaining,
            }
        }
        OperatorChoice::MarkReviewed => ResolvedWrite {
            body: body.to_string(),
            pii: PiiState::Reviewed,
            choice,
            redacted: 0,
            remaining: findings.to_vec(),
        },
        OperatorChoice::ProceedFlagged => ResolvedWrite {
            body: body.to_string(),
            pii: PiiState::Flagged,
            choice,
            redacted: 0,
            remaining: findings.to_vec(),
        },
    }
}

/// Render a hold for a terminal, with the disclaimer.
pub fn render_hold(body: &str, findings: &[Finding]) -> String {
    let mut s = format!(
        "Write held: {} possible personal data item(s) found.\n",
        findings.len()
    );
    for f in findings {
        let line = body[..f.start].matches('\n').count() + 1;
        s.push_str(&format!(
            "  line {line}, bytes {}..{}: {} [{}] {}\n    {}\n",
            f.start, f.end, f.kind, f.confidence, f.why, f.matched
        ));
    }
    s.push_str("Options: redact, mark reviewed, proceed flagged.\n");
    s.push_str(pii::DISCLAIMER);
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pii::PiiKind;

    const DIRTY: &str = "Customer bob@corp.example.org called from 203.0.113.9 about IBAN DE89 3704 0044 0532 0130 00.";

    #[test]
    fn off_skips_and_stamps_unscanned() {
        let v = check_write(Profile::Off, DIRTY);
        match v {
            WriteVerdict::Proceed {
                status: ScanStatus::Skipped { reason },
                pii,
            } => {
                assert!(reason.contains("off"));
                assert_eq!(
                    pii,
                    PiiState::Unscanned,
                    "nobody looked, and the note must say so"
                );
                assert!(!pii.was_scanned());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn eu_and_ch_hold_dirty_writes() {
        for p in [Profile::Eu, Profile::Ch] {
            match check_write(p, DIRTY) {
                WriteVerdict::Held { findings } => {
                    let kinds: Vec<PiiKind> = findings.iter().map(|f| f.kind).collect();
                    assert_eq!(kinds, [PiiKind::Email, PiiKind::Ipv4, PiiKind::Iban]);
                }
                other => panic!("{}: {other:?}", p.as_str()),
            }
        }
    }

    #[test]
    fn clean_text_proceeds_as_none_which_is_scanned() {
        match check_write(Profile::Eu, "Postgres 18 moved PGDATA.") {
            WriteVerdict::Proceed {
                status: ScanStatus::Clean,
                pii,
            } => {
                assert_eq!(pii, PiiState::None);
                assert!(pii.was_scanned());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn redact_replaces_and_rescans_clean() {
        let WriteVerdict::Held { findings } = check_write(Profile::Eu, DIRTY) else {
            panic!()
        };
        let r = resolve_hold(DIRTY, &findings, OperatorChoice::Redact);
        assert_eq!(r.redacted, 3);
        assert!(r.remaining.is_empty());
        assert_eq!(r.pii, PiiState::Reviewed);
        assert_eq!(
            r.body,
            "Customer [redacted:email] called from [redacted:ipv4] about IBAN [redacted:iban]."
        );
    }

    #[test]
    fn reviewed_and_flagged_keep_the_body() {
        let WriteVerdict::Held { findings } = check_write(Profile::Ch, DIRTY) else {
            panic!()
        };
        let r = resolve_hold(DIRTY, &findings, OperatorChoice::MarkReviewed);
        assert_eq!(
            (r.body.as_str(), r.pii, r.redacted),
            (DIRTY, PiiState::Reviewed, 0)
        );
        assert_eq!(r.remaining.len(), 3);
        let r = resolve_hold(DIRTY, &findings, OperatorChoice::ProceedFlagged);
        assert_eq!((r.body.as_str(), r.pii), (DIRTY, PiiState::Flagged));
    }

    #[test]
    fn render_carries_the_disclaimer_and_line_numbers() {
        let body = "line one\nmail bob@corp.example.org";
        let WriteVerdict::Held { findings } = check_write(Profile::Eu, body) else {
            panic!()
        };
        let out = render_hold(body, &findings);
        assert!(out.contains("line 2,"), "{out}");
        assert!(out.contains("seatbelt, not a guarantee"), "{out}");
    }
}
