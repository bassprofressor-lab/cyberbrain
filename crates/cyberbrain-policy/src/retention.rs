//! Retention (SPEC §12.5).
//!
//! A note may carry `retention: P2Y` (an ISO 8601 duration). [`evaluate`] lists what is
//! due, pending or malformed; [`apply`] erases what is due **through the same path as
//! `forget`**, one audited erasure per note.
//!
//! **Nothing here runs on a timer.** There is no background task, no hook-triggered sweep,
//! no "apply on scan". The only way a note expires is the operator running
//! `cyberbrain policy retention --apply`, and every expiry writes `retention.expired` plus
//! the erasure rows. A memory that disappears without a record is indistinguishable from a
//! bug, so the record is the feature.
//!
//! The clock starts at `created`, not `updated`. `scan` rewrites `links` (and may bump
//! `updated`) on every pass; counting from `updated` would let routine maintenance extend
//! retention indefinitely without anyone deciding it. The spec does not say which; this is
//! the conservative reading and it is reported.
//!
//! A malformed duration is listed as `Invalid`, not skipped: a note the operator meant to
//! expire and that never will is exactly the silent failure to surface.

use crate::audit::{Actor, AuditAction, AuditLog};
use crate::erasure::{EraseReason, EraseRequest, Eraser, ErasureReport, forget};
use crate::profile::Profile;
use cyberbrain_core::{Error, Frontmatter, NoteId, Result, Ring};
use jiff::{Span, Timestamp, tz::TimeZone};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Parse an ISO 8601 duration such as `P2Y`, `P6M`, `P90D`, `P1Y6M`, `PT12H`, `P2W`.
/// Must be positive; the friendly form (`2 years`) is not accepted in frontmatter.
pub fn parse_duration(s: &str) -> Result<Span> {
    let t = s.trim();
    if !t.starts_with('P') {
        return Err(Error::Config(format!(
            "retention {s:?} is not an ISO 8601 duration; expected e.g. P2Y, P90D, PT12H"
        )));
    }
    let span: Span = t
        .parse()
        .map_err(|e| Error::Config(format!("retention {s:?}: {e}")))?;
    if span.is_negative() {
        return Err(Error::Config(format!("retention {s:?} is negative")));
    }
    if span.is_zero() {
        return Err(Error::Config(format!(
            "retention {s:?} is zero; omit the field to keep a note indefinitely"
        )));
    }
    Ok(span)
}

/// `created + retention`, in UTC. Calendar units (years, months) need a zoned date.
pub fn expires_at(created: Timestamp, retention: Span) -> Result<Timestamp> {
    created
        .to_zoned(TimeZone::UTC)
        .checked_add(retention)
        .map(|z| z.timestamp())
        .map_err(|e| Error::Config(format!("retention overflows the calendar: {e}")))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum RetentionStatus {
    Due { expired_at: Timestamp },
    Pending { expires_at: Timestamp },
    Invalid { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RetentionItem {
    pub note_id: NoteId,
    pub name: String,
    pub ring: Ring,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub path: PathBuf,
    pub created: Timestamp,
    pub retention: String,
    pub status: RetentionStatus,
}

impl RetentionItem {
    pub fn is_due(&self) -> bool {
        matches!(self.status, RetentionStatus::Due { .. })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RetentionQueue {
    pub evaluated_at: Timestamp,
    /// Every note with a `retention` field, due first, then pending by expiry, then invalid.
    pub items: Vec<RetentionItem>,
    pub due: usize,
    pub pending: usize,
    pub invalid: usize,
    /// Notes without a `retention` field. The denominator, so "3 due" has a population.
    pub indefinite: usize,
}

impl RetentionQueue {
    pub fn due_items(&self) -> impl Iterator<Item = &RetentionItem> {
        self.items.iter().filter(|i| i.is_due())
    }
}

/// Evaluate every note. Takes frontmatter and path so the caller need not load bodies.
pub fn evaluate<'a>(
    notes: impl IntoIterator<Item = (&'a Frontmatter, &'a Path)>,
    now: Timestamp,
) -> RetentionQueue {
    let mut items = Vec::new();
    let mut indefinite = 0;
    for (front, path) in notes {
        let Some(ret) = &front.retention else {
            indefinite += 1;
            continue;
        };
        let status = match parse_duration(ret).and_then(|span| expires_at(front.created, span)) {
            Ok(at) if at <= now => RetentionStatus::Due { expired_at: at },
            Ok(at) => RetentionStatus::Pending { expires_at: at },
            Err(e) => RetentionStatus::Invalid {
                reason: e.to_string(),
            },
        };
        items.push(RetentionItem {
            note_id: front.id,
            name: front.name.clone(),
            ring: front.ring,
            path: path.to_path_buf(),
            created: front.created,
            retention: ret.clone(),
            status,
        });
    }
    items.sort_by_key(|i| match &i.status {
        RetentionStatus::Due { expired_at } => (0, *expired_at),
        RetentionStatus::Pending { expires_at } => (1, *expires_at),
        RetentionStatus::Invalid { .. } => (2, now),
    });
    let due = items
        .iter()
        .filter(|i| matches!(i.status, RetentionStatus::Due { .. }))
        .count();
    let invalid = items
        .iter()
        .filter(|i| matches!(i.status, RetentionStatus::Invalid { .. }))
        .count();
    RetentionQueue {
        evaluated_at: now,
        pending: items.len() - due - invalid,
        items,
        due,
        invalid,
        indefinite,
    }
}

/// `--apply`: erase every due note through [`forget`]. Runs only when called; never from a
/// timer. Returns one result per due note, in queue order; a failure on one note does not
/// stop the others, and every outcome has its rows.
pub fn apply(
    audit: &AuditLog,
    actor: &Actor,
    profile: Profile,
    eraser: &mut dyn Eraser,
    queue: &RetentionQueue,
    dry_run: bool,
) -> Vec<(RetentionItem, Result<ErasureReport>)> {
    let mut out = Vec::new();
    for item in queue.due_items() {
        let RetentionStatus::Due { expired_at } = &item.status else {
            unreachable!()
        };
        let recorded = audit.record(
            actor,
            AuditAction::RetentionExpired,
            format!("note:{}", item.note_id),
            json!({
                "name": item.name,
                "ring": item.ring,
                "retention": item.retention,
                "created": item.created,
                "expired_at": expired_at,
                "evaluated_at": queue.evaluated_at,
                "dry_run": dry_run,
            }),
        );
        let result = match recorded {
            Err(e) => Err(e), // no record, no erasure
            Ok(_) => forget(
                audit,
                actor,
                profile,
                eraser,
                &EraseRequest {
                    note_id: item.note_id,
                    name: item.name.clone(),
                    ring: item.ring,
                    path: item.path.clone(),
                    reason: EraseReason::Retention,
                    dry_run,
                },
            ),
        };
        out.push((item.clone(), result));
    }
    out
}

pub fn render(q: &RetentionQueue) -> String {
    let mut s = format!(
        "Retention at {}: {} due, {} pending, {} invalid; {} note(s) kept indefinitely.\n",
        q.evaluated_at, q.due, q.pending, q.invalid, q.indefinite
    );
    for i in &q.items {
        let line = match &i.status {
            RetentionStatus::Due { expired_at } => format!("DUE      expired {expired_at}"),
            RetentionStatus::Pending { expires_at } => format!("pending  expires {expires_at}"),
            RetentionStatus::Invalid { reason } => format!("INVALID  {reason}"),
        };
        s.push_str(&format!(
            "  {line}  {} ({}, created {}, retention {})\n",
            i.name, i.ring, i.created, i.retention
        ));
    }
    if q.due > 0 {
        s.push_str("Nothing has been erased. Run `cyberbrain policy retention --apply` to erase what is due; each erasure is recorded.\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::erasure::tests::ScriptedEraser;
    use cyberbrain_core::{NoteKind, PiiState};

    fn ts(s: &str) -> Timestamp {
        s.parse().unwrap()
    }

    fn front(name: &str, created: &str, retention: Option<&str>) -> Frontmatter {
        Frontmatter {
            id: NoteId::generate(),
            name: name.into(),
            ring: Ring::Session,
            kind: NoteKind::Session,
            created: ts(created),
            updated: ts("2026-09-01T00:00:00Z"),
            tags: vec![],
            links: vec![],
            retention: retention.map(str::to_owned),
            pii: PiiState::None,
        }
    }

    #[test]
    fn parses_iso_durations_and_rejects_the_rest() {
        for ok in [
            "P2Y", "P6M", "P90D", "P1Y6M", "PT12H", "P2W", "P1DT12H", " P30D ",
        ] {
            parse_duration(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for bad in [
            "2 years", "90d", "", "P", "-P1D", "PT0S", "P0D", "forever", "1Y",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn calendar_arithmetic_is_calendar_arithmetic() {
        let created = ts("2024-02-29T12:00:00Z");
        assert_eq!(
            expires_at(created, parse_duration("P1Y").unwrap()).unwrap(),
            ts("2025-02-28T12:00:00Z")
        );
        assert_eq!(
            expires_at(ts("2026-01-31T00:00:00Z"), parse_duration("P1M").unwrap()).unwrap(),
            ts("2026-02-28T00:00:00Z")
        );
        assert_eq!(
            expires_at(ts("2026-01-01T00:00:00Z"), parse_duration("P2W").unwrap()).unwrap(),
            ts("2026-01-15T00:00:00Z")
        );
    }

    #[test]
    fn evaluate_sorts_due_pending_invalid_and_counts_the_denominator() {
        let now = ts("2026-09-05T00:00:00Z");
        let notes = [
            (
                front("keep", "2020-01-01T00:00:00Z", None),
                PathBuf::from("a"),
            ),
            (
                front("pending", "2026-08-01T00:00:00Z", Some("P90D")),
                PathBuf::from("b"),
            ),
            (
                front("due-old", "2020-01-01T00:00:00Z", Some("P1Y")),
                PathBuf::from("c"),
            ),
            (
                front("due-new", "2026-06-01T00:00:00Z", Some("P30D")),
                PathBuf::from("d"),
            ),
            (
                front("broken", "2026-01-01T00:00:00Z", Some("90 days")),
                PathBuf::from("e"),
            ),
        ];
        let q = evaluate(notes.iter().map(|(f, p)| (f, p.as_path())), now);
        let names: Vec<&str> = q.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["due-old", "due-new", "pending", "broken"]);
        assert_eq!((q.due, q.pending, q.invalid, q.indefinite), (2, 1, 1, 1));
        assert!(
            matches!(&q.items[3].status, RetentionStatus::Invalid { reason } if reason.contains("ISO 8601"))
        );
        let out = render(&q);
        assert!(
            out.contains("2 due, 1 pending, 1 invalid; 1 note(s) kept indefinitely"),
            "{out}"
        );
        assert!(out.contains("Nothing has been erased"), "{out}");
    }

    #[test]
    fn the_clock_starts_at_created_not_updated() {
        // updated is 2026-09-01 in the fixture; created + P30D is long past.
        let now = ts("2026-09-05T00:00:00Z");
        let f = front("n", "2026-01-01T00:00:00Z", Some("P30D"));
        let p = PathBuf::from("x");
        let q = evaluate([(&f, p.as_path())], now);
        assert!(
            q.items[0].is_due(),
            "a scan bumping `updated` must not extend retention"
        );
    }

    #[test]
    fn apply_erases_only_what_is_due_with_rows_for_each() {
        let now = ts("2026-09-05T00:00:00Z");
        let notes = [
            (
                front("due", "2020-01-01T00:00:00Z", Some("P1Y")),
                PathBuf::from("c"),
            ),
            (
                front("pending", "2026-08-01T00:00:00Z", Some("P90D")),
                PathBuf::from("b"),
            ),
        ];
        let q = evaluate(notes.iter().map(|(f, p)| (f, p.as_path())), now);
        let (log, sink) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(ErasureReport {
            file_removed: true,
            blocks: 1,
            fts_rows: 1,
            vectors: 1,
            ..Default::default()
        });
        let results = apply(&log, &Actor::Retention, Profile::Eu, &mut e, &q, false);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.name, "due");
        assert!(results[0].1.is_ok());
        assert_eq!(e.calls.len(), 1, "the pending note is untouched");
        assert_eq!(e.calls[0].reason, EraseReason::Retention);
        assert_eq!(
            sink.actions(),
            [
                "retention.expired",
                "note.erase.requested",
                "note.erase.completed"
            ]
        );
        assert_eq!(sink.rows()[0].actor, "retention");
        assert_eq!(sink.rows()[0].detail["retention"], "P1Y");
    }

    #[test]
    fn apply_without_a_record_does_not_erase() {
        let now = ts("2026-09-05T00:00:00Z");
        let f = front("due", "2020-01-01T00:00:00Z", Some("P1Y"));
        let p = PathBuf::from("c");
        let q = evaluate([(&f, p.as_path())], now);
        let (log, sink) = AuditLog::in_memory();
        sink.fail_next_append();
        let mut e = ScriptedEraser::ok(ErasureReport::default());
        let results = apply(&log, &Actor::Retention, Profile::Eu, &mut e, &q, false);
        assert!(results[0].1.is_err());
        assert!(e.calls.is_empty(), "no record, no erasure");
    }

    #[test]
    fn apply_dry_run_runs_the_real_path() {
        let now = ts("2026-09-05T00:00:00Z");
        let f = front("due", "2020-01-01T00:00:00Z", Some("P1Y"));
        let p = PathBuf::from("c");
        let q = evaluate([(&f, p.as_path())], now);
        let (log, sink) = AuditLog::in_memory();
        let mut e = ScriptedEraser::ok(ErasureReport {
            file_removed: true,
            blocks: 1,
            fts_rows: 1,
            vectors: 1,
            ..Default::default()
        });
        let results = apply(&log, &Actor::Retention, Profile::Eu, &mut e, &q, true);
        assert!(results[0].1.as_ref().unwrap().dry_run);
        assert!(e.calls[0].dry_run);
        assert_eq!(sink.rows()[0].detail["dry_run"], true);
    }
}
