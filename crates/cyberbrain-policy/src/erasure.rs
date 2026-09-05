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
    /// What the eraser could not confirm, in words. Never empty out of politeness.
    pub notes: Vec<String>,
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
    if r.blocks > 0 && r.vectors == 0 {
        r.notes.push(format!("{} block(s) removed but 0 vectors: either the note was never embedded or vectors were left behind; run `cyberbrain doctor`", r.blocks));
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
    for n in &r.notes {
        s.push_str(&format!("  note: {n}\n"));
    }
    s.push_str("  audit rows: kept (SPEC §12.2: the erasure record is the evidence)\n");
    s
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

    #[test]
    fn the_spec_named_silent_failure_is_called_out() {
        let (log, _) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(ErasureReport {
            file_removed: true,
            blocks: 3,
            fts_rows: 3,
            vectors: 0,
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
}
