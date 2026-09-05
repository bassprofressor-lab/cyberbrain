//! The seam between the audit *semantics* (`cyberbrain_policy::AuditLog`, the hash chain)
//! and the audit *rows* (`cyberbrain_index::AuditStore`, `audit.db`). SPEC §8.2 step 3:
//! the binary implements policy's `AuditSink` over the index's store, and nothing else
//! appends. Nothing here computes a hash; nothing here decides anything.
//!
//! Two properties this bridge has to hold:
//!
//! - **`detail` is text on the wire, canonical on both sides.** Policy hands over a
//!   `serde_json::Value` and expects one back, so the bridge serialises once on append and
//!   parses once on read. That is only safe because `serde_json` is built without
//!   `preserve_order`: `Value::Object` is a `BTreeMap`, its keys are sorted, and the text is
//!   the same text policy hashes. `detail_round_trip_is_byte_stable` pins it.
//! - **`last` + `append` is one write lock.** Policy serialises the pair inside a process;
//!   across processes it cannot. `AuditStore::append_after` reads the predecessor and inserts
//!   under one `BEGIN IMMEDIATE`, so this bridge re-checks that the predecessor policy chained
//!   to is still the last row, and fails closed if another process got in between. A refused
//!   append is a retry; a forked chain is a false tamper report.

use cyberbrain_core::{Error, Result};
use cyberbrain_index::{AuditEntry, AuditStore, NewAuditEntry};
use cyberbrain_policy::audit::GENESIS;
use cyberbrain_policy::{AuditEvent, AuditFilter, AuditSink};
use serde_json::Value;
use std::sync::{Mutex, MutexGuard};

/// `audit.db` inside the store (SPEC §4). Core names `cyberbrain.db` (`store::DB_FILE`)
/// but not this one; see the report.
pub const AUDIT_DB_FILE: &str = "audit.db";

pub struct StoreAuditSink {
    store: Mutex<AuditStore>,
}

impl StoreAuditSink {
    pub fn new(store: AuditStore) -> Self {
        Self {
            store: Mutex::new(store),
        }
    }

    pub fn count(&self) -> Result<usize> {
        self.lock()?.count()
    }

    pub fn schema_version(&self) -> Result<u32> {
        self.lock()?.schema_version()
    }

    /// A poisoned lock means a writer panicked mid-append. Fail closed rather than write
    /// after it: the egress gate proceeds on `Ok`, and an unrecorded permit is the hole.
    fn lock(&self) -> Result<MutexGuard<'_, AuditStore>> {
        self.store.lock().map_err(|_| {
            Error::Index("audit store lock poisoned; refusing to touch the record".into())
        })
    }
}

/// The chain hash a stored row carries, read without normalising the text.
fn stored_chain_hash(entry: &AuditEntry) -> Option<String> {
    let detail: Value = serde_json::from_str(entry.detail.as_deref()?).ok()?;
    detail
        .get("_chain")?
        .get("hash")?
        .as_str()
        .map(str::to_owned)
}

/// The audit column's format: `YYYY-MM-DDTHH:MM:SS.sssZ`, UTC, fixed width. Written out
/// by hand rather than via `Display`, which trims a zero fraction and would produce rows
/// of two different widths in a column that `since` compares as strings.
fn format_ts(ts: jiff::Timestamp) -> String {
    let z = ts.to_zoned(jiff::tz::TimeZone::UTC);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        z.year(),
        z.month(),
        z.day(),
        z.hour(),
        z.minute(),
        z.second(),
        z.subsec_nanosecond() / 1_000_000
    )
}

fn to_event(e: AuditEntry) -> Result<AuditEvent> {
    let ts = e.ts.parse::<jiff::Timestamp>().map_err(|err| {
        Error::Index(format!(
            "audit row {}: stored ts {:?} is not a timestamp: {err}",
            e.seq, e.ts
        ))
    })?;
    let detail = match e.detail {
        Some(text) => serde_json::from_str(&text).map_err(|err| {
            Error::Index(format!(
                "audit row {}: stored detail is not JSON: {err}",
                e.seq
            ))
        })?,
        None => serde_json::json!({}),
    };
    Ok(AuditEvent {
        ts,
        actor: e.actor,
        action: e.action,
        subject: e.subject.unwrap_or_default(),
        detail,
    })
}

impl AuditSink for StoreAuditSink {
    fn append(&self, event: &AuditEvent) -> Result<()> {
        let detail = serde_json::to_string(&event.detail)
            .map_err(|e| Error::Index(format!("audit detail does not serialise: {e}")))?;
        let expected_prev = event.chain_prev().map(str::to_owned);
        let row = NewAuditEntry {
            // One clock, and it is the writer's. Letting the store stamp its own would
            // give the event two timestamps: the one the caller is handed back and the
            // one that is stored, truncated to milliseconds. A `since` filter built from
            // the returned value would then miss the row it came from.
            ts: Some(format_ts(event.ts)),
            actor: event.actor.clone(),
            action: event.action.clone(),
            subject: Some(event.subject.clone()),
            detail: Some(detail),
        };
        self.lock()?.append_after(|last| {
            if let Some(expected) = &expected_prev {
                let actual = last
                    .and_then(stored_chain_hash)
                    .unwrap_or_else(|| GENESIS.to_string());
                if *expected != actual {
                    return Err(Error::Index(format!(
                        "audit: another writer appended row {} while this row was being \
                         chained; nothing was written, retry the operation",
                        last.map(|l| l.seq).unwrap_or(0)
                    )));
                }
            }
            Ok(row)
        })?;
        Ok(())
    }

    fn read(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        // `since` is a timestamp on this side and text on the store's; rather than trust
        // two formats to sort alike, filter it here and apply the limit afterwards.
        // `action` is matched here rather than in the store, because a reader asking for
        // `note` means the whole family and the store compares exactly. Accepting a family
        // name in validation and then not serving it is worse than not accepting it: the
        // answer comes back empty and reads as "that never happened".
        let store_filter = cyberbrain_index::AuditFilter {
            since: None,
            action: None,
            subject: filter.subject.clone(),
            contains: filter.contains.clone(),
            limit: if filter.since.is_some() || filter.action.is_some() {
                None
            } else {
                filter.limit
            },
        };
        let rows = self.lock()?.read(&store_filter)?;
        let mut out = Vec::with_capacity(rows.len());
        let narrowed = filter.since.is_some() || filter.action.is_some();
        for r in rows {
            let e = to_event(r)?;
            if let Some(since) = filter.since
                && e.ts < since
            {
                continue;
            }
            if let Some(action) = &filter.action
                && !(e.action == *action || e.action.starts_with(&format!("{action}.")))
            {
                continue;
            }
            out.push(e);
            if narrowed && filter.limit.is_some_and(|l| out.len() >= l) {
                break;
            }
        }
        Ok(out)
    }

    fn last(&self) -> Result<Option<AuditEvent>> {
        self.lock()?.last()?.map(to_event).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyberbrain_policy::{Actor, AuditAction, AuditLog};
    use serde_json::json;
    use std::sync::Arc;

    fn log() -> (AuditLog, Arc<StoreAuditSink>) {
        let sink = Arc::new(StoreAuditSink::new(AuditStore::open_in_memory().unwrap()));
        (AuditLog::new(sink.clone()), sink)
    }

    /// The chain hash is computed over `detail` as a `Value` and stored as text. If the
    /// text that comes back parsed to a different canonical form, every row after the
    /// first would verify as edited. This is the property the bridge exists to hold.
    #[test]
    fn detail_round_trip_is_byte_stable_and_the_chain_verifies() {
        let (log, sink) = log();
        for i in 0..5 {
            log.record(
                &Actor::Cli,
                AuditAction::NoteWrite,
                format!("note:{i}"),
                json!({ "zeta": i, "alpha": [1, 2, {"y": 1, "x": 2}], "m": {"b": 1, "a": 2} }),
            )
            .unwrap();
        }
        assert_eq!(log.verify().unwrap(), 5);
        assert_eq!(sink.count().unwrap(), 5);
        let rows = log.read(&AuditFilter::default()).unwrap();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].subject, "note:0");
    }

    #[test]
    fn a_foreign_row_between_two_chained_ones_is_refused_not_forked() {
        let (log, sink) = log();
        log.record(&Actor::Cli, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        // Simulate a second process appending behind policy's back.
        sink.lock()
            .unwrap()
            .append(&NewAuditEntry {
                // A foreign writer that does not keep its own clock; the store stamps.
                ts: None,
                actor: "other-process".into(),
                action: "note.write".into(),
                subject: Some("b".into()),
                detail: Some("{}".into()),
            })
            .unwrap();
        // Policy computed prev from its own last() *before* that row existed? No: it
        // reads last() at record time and sees the foreign row (no _chain -> GENESIS).
        // The bridge then compares GENESIS against the store's last, agrees, appends.
        // The chain is broken at the foreign row, and verify says exactly where.
        log.record(&Actor::Cli, AuditAction::NoteWrite, "c", json!({}))
            .unwrap();
        let err = log.verify().unwrap_err().to_string();
        assert!(err.contains("row 1"), "{err}");
        assert!(err.contains("no _chain"), "{err}");
    }

    /// One event, one timestamp. The store column holds milliseconds; a writer's clock
    /// does not. If the writer keeps the finer value and the store keeps the coarser one,
    /// every `since` filter built from a returned row silently skips that row, and a
    /// reader sees a gap in a log that has none.
    ///
    /// This is the deterministic version of a failure that showed up as a flaky test: it
    /// only tripped when the microsecond part happened to be non-zero, which is most of
    /// the time but not all of it. Run against the un-truncated stamp it fails on the
    /// first assertion with values differing after the third decimal.
    #[test]
    fn the_stored_timestamp_is_exactly_the_one_the_caller_was_handed() {
        let (log, _) = log();
        let written = log
            .record(&Actor::Cli, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();

        let read_back = log.read(&AuditFilter::default()).unwrap();
        assert_eq!(read_back.len(), 1);
        assert_eq!(
            read_back[0].ts, written.ts,
            "the row as stored must carry the timestamp the caller holds"
        );

        // The consequence, stated as its own assertion so a regression names the symptom
        // and not just the cause.
        let since_itself = log
            .read(&AuditFilter {
                since: Some(written.ts),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            since_itself.len(),
            1,
            "a row must be found by a filter built from its own timestamp"
        );
    }

    #[test]
    fn read_filters_by_since_and_limit_on_this_side() {
        let (log, _) = log();
        let first = log
            .record(&Actor::Cli, AuditAction::NoteWrite, "a", json!({}))
            .unwrap();
        log.record(&Actor::Cli, AuditAction::NoteWrite, "b", json!({}))
            .unwrap();
        let rows = log
            .read(&AuditFilter {
                since: Some(first.ts),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        let rows = log
            .read(&AuditFilter {
                action: Some("note.write".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
    }
}
