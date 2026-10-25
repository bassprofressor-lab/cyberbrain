//! Erasure, policy side (SPEC §12.2).
//!
//! `cyberbrain forget` removes the note file, its blocks, its vectors, its FTS entries, its
//! outbound link rows and any cached derivative, in one transaction, and prints what it
//! removed. The storage side is `cyberbrain_index::Index::delete_note` plus removing the
//! file; this module defines the seam ([`Eraser`]), wraps every call in audit rows, and
//! renders the result.
//!
//! Two rules from the spec are encoded here so nobody "fixes" them later:
//!
//! - **Inbound links are unresolved, not deleted.** Other notes still say `[[name]]` on
//!   disk; deleting those rows would put the index at odds with the files. The report
//!   counts them as `links_in_unresolved`.
//! - **Audit rows are exempt from erasure.** The row saying a note was erased is what makes
//!   the erasure demonstrable. Nothing here offers to remove audit rows, and the erase
//!   request carries the note's *id*, not its content, so the row is not itself a copy.
//!
//! The spec's named silent failure — the file gone, the vector still in the index — is a
//! storage-side property the index crate must test. This side checks the report for the
//! shape of that failure and says so when it sees it.

use crate::audit::{Actor, AuditAction, AuditLog};
use crate::profile::{Profile, ProfileExt};
use cyberbrain_core::{NoteId, Result, Ring};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EraseReason {
    /// `cyberbrain forget` by the operator.
    OperatorForget,
    /// `cyberbrain policy retention --apply`.
    Retention,
    /// A data subject asked (Art. 17 GDPR / Art. 32 FADP).
    SubjectRequest,
}

impl EraseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            EraseReason::OperatorForget => "operator-forget",
            EraseReason::Retention => "retention",
            EraseReason::SubjectRequest => "subject-request",
        }
    }
}

/// What to erase. Identity only; never the body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EraseRequest {
    pub note_id: NoteId,
    pub name: String,
    pub ring: Ring,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub reason: EraseReason,
    /// SPEC §8: the real path with a no-op writer. An eraser given `dry_run` reports what
    /// it *would* remove and removes nothing; the audit rows still go to whatever sink the
    /// caller wired, which for a dry run should be an in-memory one.
    pub dry_run: bool,
}

/// What was removed, per store. Mirrors `cyberbrain_index::Erased` plus the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErasureReport {
    pub note_id: String,
    pub name: String,
    pub dry_run: bool,
    pub file_removed: bool,
    pub blocks: usize,
    pub fts_rows: usize,
    pub vectors: usize,
    pub links_out: usize,
    /// Inbound link rows that now point nowhere. Kept, per SPEC §12.2.
    pub links_in_unresolved: usize,
    /// Anything cached beyond the index rows (summaries, embeddings on disk, ...).
    pub derivatives: usize,
    /// Whether this store embeds at all, filled by the eraser from the index's recorded
    /// profile. It decides whether "0 vectors removed" is a finding or the normal state:
    /// on a lexical-only store every note has none, and reporting that as a possible
    /// silent failure trains the reader to ignore the line that matters.
    #[serde(default)]
    pub vectors_expected: bool,
    /// What the eraser could not confirm, in words. Never empty out of politeness.
    pub notes: Vec<String>,
    /// What the clean-up after the erasure found in the database file: whether the WAL was
    /// emptied, or whether pre-erasure pages may still sit in it. `None` on a dry run, which
    /// erased nothing and scrubbed nothing, and in reports written before this field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrub: Option<ScrubStatus>,
}

/// The index's state after the scrub that follows every erasure (B5, F9).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScrubStatus {
    /// Pages written before the erasure may still be on disk, in `cyberbrain.db-wal`. They
    /// go at the next checkpoint nobody blocks; until then a copy of the store carries them.
    pub residue: bool,
    /// SQLite's answer to `wal_checkpoint(TRUNCATE)`. `None` where the index has no WAL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal: Option<WalStatus>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalStatus {
    pub busy: bool,
    pub log_frames: i64,
    pub checkpointed_frames: i64,
}

impl ScrubStatus {
    /// The sentence a reader needs when there is residue, and `None` when there is none.
    pub fn residue_note(&self) -> Option<String> {
        if !self.residue {
            return None;
        }
        let detail = match self.wal {
            Some(w) if w.busy => format!(
                "another process held the index open, and {} of {} WAL frame(s) reached the main file",
                w.checkpointed_frames.max(0),
                w.log_frames.max(0)
            ),
            Some(w) => format!(
                "{} of {} WAL frame(s) reached the main file",
                w.checkpointed_frames.max(0),
                w.log_frames.max(0)
            ),
            None => "the WAL could not be checked".into(),
        };
        Some(format!(
            "the WAL was not emptied ({detail}): pre-erasure pages may remain in cyberbrain.db-wal \
             until the next complete checkpoint. SQLite runs one, and removes the WAL, when the last \
             connection closes: stop `serve`, `mcp` and the daemon to get there"
        ))
    }
}

/// The storage seam. The binary implements it as: remove the file, then
/// `Index::delete_note`, in that order inside one logical operation, and fill the report
/// from `Erased`. A `dry_run` request must run the same code with writes disabled.
pub trait Eraser {
    fn erase(&mut self, req: &EraseRequest) -> Result<ErasureReport>;
}

/// Erase with audit rows around it: `note.erase.requested` before, `note.erase.completed`
/// or `note.erase.failed` after. The basis cited depends on the profile.
pub fn forget(
    audit: &AuditLog,
    actor: &Actor,
    profile: Profile,
    eraser: &mut dyn Eraser,
    req: &EraseRequest,
) -> Result<ErasureReport> {
    let subject = format!("note:{}", req.note_id);
    audit.record(
        actor,
        AuditAction::NoteEraseRequested,
        &subject,
        json!({
            "name": req.name,
            "ring": req.ring,
            "reason": req.reason,
            "basis": profile.erasure_basis(),
            "profile": profile,
            "dry_run": req.dry_run,
        }),
    )?;
    match eraser.erase(req) {
        Ok(mut report) => {
            report.note_id = req.note_id.to_string();
            report.name = req.name.clone();
            report.dry_run = req.dry_run;
            sanity_notes(&mut report);
            audit.record(
                actor,
                AuditAction::NoteEraseCompleted,
                &subject,
                json!({
                    "name": req.name,
                    "reason": req.reason,
                    "dry_run": req.dry_run,
                    "file_removed": report.file_removed,
                    "blocks": report.blocks,
                    "fts_rows": report.fts_rows,
                    "vectors": report.vectors,
                    "links_out": report.links_out,
                    "links_in_unresolved": report.links_in_unresolved,
                    "derivatives": report.derivatives,
                    "notes": report.notes,
                }),
            )?;
            Ok(report)
        }
        Err(e) => {
            audit.record(
                actor,
                AuditAction::NoteEraseFailed,
                &subject,
                json!({ "name": req.name, "reason": req.reason, "dry_run": req.dry_run, "error": e.to_string() }),
            )?;
            Err(e)
        }
    }
}

/// The shapes of silent failure the spec names. These are not errors: the eraser did what
/// it did, and the report says what looks wrong.
fn sanity_notes(r: &mut ErasureReport) {
    if r.dry_run {
        return;
    }
    if !r.file_removed {
        r.notes.push("the note file was not removed (already gone, or the eraser skipped it); the index rows were".into());
    }
    if r.vectors_expected && r.blocks > 0 && r.vectors == 0 {
        r.notes.push(format!("{} block(s) removed but 0 vectors, on a store that embeds: either this note was never indexed or vectors were left behind; run `cyberbrain doctor`", r.blocks));
    }
    if r.blocks > 0 && r.fts_rows == 0 {
        r.notes.push(format!("{} block(s) removed but 0 FTS rows: lexical search may still find this note; run `cyberbrain doctor`", r.blocks));
    }
}

pub fn render(r: &ErasureReport) -> String {
    let verb = if r.dry_run { "would remove" } else { "removed" };
    let mut s = format!(
        "{} {} ({}):\n  file: {}\n  blocks: {}\n  fts rows: {}\n  vectors: {}\n  outbound links: {}\n  inbound links now dangling: {}\n  cached derivatives: {}\n",
        if r.dry_run { "[dry run] " } else { "" },
        r.name,
        r.note_id,
        if r.file_removed { verb } else { "not removed" },
        r.blocks,
        r.fts_rows,
        r.vectors,
        r.links_out,
        r.links_in_unresolved,
        r.derivatives,
    );
    if let Some(sc) = &r.scrub {
        s.push_str(if sc.residue {
            "  wal: NOT emptied, pre-erasure pages may remain\n"
        } else {
            "  wal: emptied, nothing of the note left in the database files\n"
        });
    }
    for n in &r.notes {
        s.push_str(&format!("  note: {n}\n"));
    }
    s.push_str("  audit rows: kept (SPEC §12.2: the erasure record is the evidence)\n");
    s
}

/// A report of one erasure that somebody else can check (F9): what went, whether the WAL
/// was emptied, which audit rows record it, and — when the store has an audit key — a
/// signature over all of it.
///
/// # Format
///
/// One JSON object. The signature is Ed25519 over the domain tag
/// `cyberbrain.erasure.report.v1\n` followed by the object **without** its `sig` key,
/// serialised as compact JSON with sorted keys — the same canonical form the audit hash uses
/// for `detail`, so a checker in another language needs one rule, not two.
pub mod receipt {
    use super::ErasureReport;
    use crate::audit_key::{
        AuditKey, ERASURE_DOMAIN, fingerprint_of, key_matches, verify_signature,
    };
    use cyberbrain_core::{Error, Result};
    use serde::{Deserialize, Serialize};

    pub const KIND: &str = "cyberbrain.erasure.report";
    pub const VERSION: u32 = 1;

    /// An audit row the erasure left, named by its chain hash so the report can be tied to
    /// the log (and to any audit bundle that holds the row).
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AuditRef {
        pub action: String,
        pub hash: String,
        pub at: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Receipt {
        pub kind: String,
        pub version: u32,
        pub tool: String,
        pub written_at: String,
        pub erasure: ErasureReport,
        /// Pre-erasure pages may still be on disk. The same answer as `erasure.scrub`,
        /// lifted to the top because it is the one line a reader must not miss. `true`
        /// also when the scrub could not be checked at all.
        pub residue: bool,
        pub audit: Vec<AuditRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub key: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub sig: Option<String>,
    }

    impl Receipt {
        pub fn new(erasure: ErasureReport, audit: Vec<AuditRef>, tool: &str, now: &str) -> Self {
            // No scrub on a real erasure means nobody looked, and "nothing left" would be a
            // claim nobody checked. A dry run erased nothing and leaves nothing.
            let residue = match erasure.scrub {
                Some(s) => s.residue,
                None => !erasure.dry_run,
            };
            Receipt {
                kind: KIND.to_string(),
                version: VERSION,
                tool: tool.to_string(),
                written_at: now.to_string(),
                erasure,
                residue,
                audit,
                key: None,
                sig: None,
            }
        }

        /// Sign with the store's key. The public key goes into the signed content, so it
        /// cannot be swapped without breaking the signature.
        pub fn sign(&mut self, key: &AuditKey) -> Result<()> {
            self.sig = None;
            self.key = Some(key.public_hex());
            let body = canonical(self)?;
            self.sig = Some(key.sign(ERASURE_DOMAIN, body.as_bytes()));
            Ok(())
        }
    }

    fn canonical(r: &Receipt) -> Result<String> {
        let mut v = serde_json::to_value(r)
            .map_err(|e| Error::Index(format!("erasure report does not serialise: {e}")))?;
        if let serde_json::Value::Object(m) = &mut v {
            m.remove("sig");
        }
        Ok(v.to_string())
    }

    /// What a check of a report found.
    #[derive(Debug, Clone, Serialize, PartialEq, Eq)]
    pub struct Checked {
        pub receipt: Receipt,
        /// Fingerprint of the key the signature holds for; `None` for an unsigned report.
        pub signed_by: Option<String>,
    }

    /// Check a report on its own: shape, version, and the signature when there is one.
    pub fn verify(text: &str) -> Result<Checked> {
        let bad = |m: String| Error::Index(format!("erasure report: {m}"));
        let r: Receipt =
            serde_json::from_str(text).map_err(|e| bad(format!("not an erasure report: {e}")))?;
        if r.kind != KIND {
            return Err(bad(format!("kind is {:?}, expected {KIND:?}", r.kind)));
        }
        if r.version != VERSION {
            return Err(bad(format!(
                "format version {} is not {VERSION}; use a reader that knows it",
                r.version
            )));
        }
        // The top-level line must agree with the detail it summarises; otherwise an edit to
        // the one a reader looks at first would go unnoticed in an unsigned report.
        if let Some(s) = r.erasure.scrub
            && s.residue != r.residue
        {
            return Err(bad("`residue` contradicts `erasure.scrub.residue`".into()));
        }
        let signed_by = match (&r.key, &r.sig) {
            (None, None) => None,
            (Some(key), Some(sig)) => {
                verify_signature(key, ERASURE_DOMAIN, canonical(&r)?.as_bytes(), sig)
                    .map_err(|e| bad(format!("{e}; the report was changed after it was signed")))?;
                Some(fingerprint_of(key)?)
            }
            (Some(_), None) => {
                return Err(bad(
                    "it names a signing key but carries no signature; it was removed".into(),
                ));
            }
            (None, Some(_)) => {
                return Err(bad("it carries a signature but names no key".into()));
            }
        };
        Ok(Checked {
            receipt: r,
            signed_by,
        })
    }

    /// [`verify`], and the report must be signed with the key `expected` names.
    pub fn verify_expecting(text: &str, expected: &str) -> Result<Checked> {
        let c = verify(text)?;
        match &c.receipt.key {
            Some(k) if key_matches(expected, k) => Ok(c),
            Some(_) => Err(Error::Index(format!(
                "erasure report: signed with key {}, not with the expected {expected}",
                c.signed_by.as_deref().unwrap_or("?")
            ))),
            None => Err(Error::Index(format!(
                "erasure report: not signed, and key {expected} was expected"
            ))),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use cyberbrain_core::Error;

    /// A test eraser that returns a scripted report and remembers what it was asked.
    pub(crate) struct ScriptedEraser {
        pub(crate) result: std::result::Result<ErasureReport, String>,
        pub(crate) calls: Vec<EraseRequest>,
    }

    impl ScriptedEraser {
        pub(crate) fn ok(report: ErasureReport) -> Self {
            Self {
                result: Ok(report),
                calls: Vec::new(),
            }
        }
    }

    impl Eraser for ScriptedEraser {
        fn erase(&mut self, req: &EraseRequest) -> Result<ErasureReport> {
            self.calls.push(req.clone());
            match &self.result {
                Ok(r) => Ok(ErasureReport {
                    dry_run: req.dry_run,
                    ..r.clone()
                }),
                Err(msg) => Err(Error::Index(msg.clone())),
            }
        }
    }

    pub(crate) fn req(dry_run: bool) -> EraseRequest {
        EraseRequest {
            note_id: NoteId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
            name: "pg18-moves-pgdata".into(),
            ring: Ring::Knowledge,
            path: PathBuf::from("notes/r2/pg18-moves-pgdata.md"),
            reason: EraseReason::OperatorForget,
            dry_run,
        }
    }

    fn full() -> ErasureReport {
        ErasureReport {
            file_removed: true,
            blocks: 3,
            fts_rows: 3,
            vectors: 3,
            links_out: 2,
            links_in_unresolved: 1,
            ..Default::default()
        }
    }

    #[test]
    fn forget_wraps_the_eraser_in_two_rows_and_cites_the_profile() {
        let (log, sink) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(full());
        let r = forget(&log, &Actor::Operator, Profile::Ch, &mut e, &req(false)).unwrap();
        assert_eq!(e.calls.len(), 1);
        assert_eq!(r.name, "pg18-moves-pgdata");
        assert!(r.notes.is_empty(), "{:?}", r.notes);
        assert_eq!(
            sink.actions(),
            ["note.erase.requested", "note.erase.completed"]
        );
        let rows = sink.rows();
        assert_eq!(rows[0].subject, "note:01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert!(rows[0].detail["basis"].as_str().unwrap().contains("FADP"));
        assert_eq!(rows[1].detail["vectors"], 3);
        assert_eq!(rows[1].detail["links_in_unresolved"], 1);
        // The audit row names the note, never its content.
        assert!(!serde_json::to_string(&rows).unwrap().contains("body"));
    }

    #[test]
    fn eu_cites_article_17() {
        let (log, sink) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(full());
        forget(&log, &Actor::Operator, Profile::Eu, &mut e, &req(false)).unwrap();
        assert!(
            sink.rows()[0].detail["basis"]
                .as_str()
                .unwrap()
                .contains("Art. 17")
        );
    }

    #[test]
    fn a_failing_eraser_is_recorded_as_failed() {
        let (log, sink) = AuditLog::in_memory();
        let mut e = ScriptedEraser {
            result: Err("disk on fire".into()),
            calls: Vec::new(),
        };
        assert!(forget(&log, &Actor::Cli, Profile::Eu, &mut e, &req(false)).is_err());
        assert_eq!(
            sink.actions(),
            ["note.erase.requested", "note.erase.failed"]
        );
        assert_eq!(sink.rows()[1].detail["error"], "index: disk on fire");
    }

    /// The same report on a store that does not embed. Calibrated against the state that
    /// used to produce the warning: only the flag differs from the test above.
    #[test]
    fn a_lexical_only_store_is_not_accused_of_losing_vectors() {
        let (log, _) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(ErasureReport {
            file_removed: true,
            blocks: 3,
            fts_rows: 3,
            vectors: 0,
            vectors_expected: false,
            ..Default::default()
        });
        let r = forget(&log, &Actor::Operator, Profile::Eu, &mut e, &req(false)).unwrap();
        assert!(
            !r.notes.iter().any(|n| n.contains("0 vectors")),
            "a store with no embedding profile has no vectors to lose: {:?}",
            r.notes
        );
    }

    #[test]
    fn the_spec_named_silent_failure_is_called_out() {
        let (log, _) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(ErasureReport {
            file_removed: true,
            blocks: 3,
            fts_rows: 3,
            vectors: 0,
            vectors_expected: true,
            ..Default::default()
        });
        let r = forget(&log, &Actor::Operator, Profile::Eu, &mut e, &req(false)).unwrap();
        assert!(
            r.notes.iter().any(|n| n.contains("0 vectors")),
            "{:?}",
            r.notes
        );
        let mut e = ScriptedEraser::ok(ErasureReport {
            file_removed: false,
            blocks: 1,
            fts_rows: 1,
            vectors: 1,
            ..Default::default()
        });
        let r = forget(&log, &Actor::Operator, Profile::Eu, &mut e, &req(false)).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("not removed")));
    }

    #[test]
    fn dry_run_takes_the_same_path_and_says_so() {
        let (log, sink) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(full());
        let r = forget(&log, &Actor::Operator, Profile::Eu, &mut e, &req(true)).unwrap();
        assert!(r.dry_run);
        assert!(
            e.calls[0].dry_run,
            "the eraser itself runs, with writes disabled"
        );
        assert!(r.notes.is_empty(), "no sanity complaints about a dry run");
        assert_eq!(sink.rows()[0].detail["dry_run"], true);
        assert!(render(&r).starts_with("[dry run] "));
        assert!(render(&r).contains("audit rows: kept"));
    }

    // ---- the checkable report (F9) ----

    fn receipt_of(report: ErasureReport) -> receipt::Receipt {
        receipt::Receipt::new(
            report,
            vec![receipt::AuditRef {
                action: "note.erase.completed".into(),
                hash: "ab".repeat(32),
                at: "2026-10-25T00:00:00Z".into(),
            }],
            "test",
            "2026-10-25T00:00:01Z",
        )
    }

    #[test]
    fn a_signed_erasure_report_verifies_and_an_edit_does_not() {
        let key = crate::AuditKey::generate().unwrap();
        let mut r = receipt_of(ErasureReport {
            scrub: Some(ScrubStatus {
                residue: true,
                wal: Some(WalStatus {
                    busy: true,
                    log_frames: 9,
                    checkpointed_frames: 3,
                }),
            }),
            ..full()
        });
        assert!(r.residue);
        r.sign(&key).unwrap();
        let text = serde_json::to_string_pretty(&r).unwrap();
        let c = receipt::verify_expecting(&text, &key.fingerprint()).unwrap();
        assert_eq!(c.signed_by.as_deref(), Some(key.fingerprint().as_str()));

        // The line somebody would most like to change.
        let edited = text
            .replace("\"residue\": true", "\"residue\": false")
            .replace("\"busy\": true", "\"busy\": false");
        let err = receipt::verify(&edited).unwrap_err().to_string();
        assert!(err.contains("signature"), "{err}");
        let other = crate::AuditKey::generate().unwrap();
        assert!(receipt::verify_expecting(&text, &other.fingerprint()).is_err());
    }

    #[test]
    fn an_unsigned_report_verifies_but_not_against_a_key() {
        let r = receipt_of(full());
        assert!(
            r.residue,
            "no scrub recorded on a real erasure is not a clean bill"
        );
        let text = serde_json::to_string(&r).unwrap();
        let c = receipt::verify(&text).unwrap();
        assert_eq!(c.signed_by, None);
        assert!(receipt::verify_expecting(&text, "00").is_err());
        let contradicted = serde_json::to_string(&receipt::Receipt {
            residue: false,
            erasure: ErasureReport {
                scrub: Some(ScrubStatus {
                    residue: true,
                    wal: None,
                }),
                ..full()
            },
            ..r
        })
        .unwrap();
        assert!(receipt::verify(&contradicted).is_err());
    }
}
