//! What retrieval returned, and what reading the whole thing would have cost.
//!
//! This is the ledger behind the one claim the tool makes for itself: that an agent which
//! recalls a slice spends fewer context tokens than one that opens files. It is a record,
//! not a cache, so it lives beside `audit.db` rather than inside `cyberbrain.db`, which may
//! be deleted and rebuilt at any time. It is deliberately not the audit log: routine reads
//! are not compliance events, and appending thousands of them would drown the hash chain.
//!
//! Both sides of every row are measured, never estimated. A `recall` is counted in tokens,
//! because the index stores an exact count per block. A `find` is counted in lines, because
//! that is what the agent is told to read. Mixing the two into one number would be a
//! rounder figure and a worse one, so the summary keeps them apart.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One retrieval, as it happened.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRow {
    pub at: String,
    /// `recall` or `find`.
    pub op: String,
    /// `tokens` or `lines`.
    pub unit: String,
    /// What the caller was handed.
    pub returned: u64,
    /// What the sources it came from hold in full: the price of reading them instead.
    pub full: u64,
    /// Hits returned.
    pub hits: u64,
    /// Distinct notes or files behind those hits.
    pub sources: u64,
}

/// Totals for one op, over the rows that are still on disk.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageTotals {
    pub ops: u64,
    pub returned: u64,
    pub full: u64,
    pub hits: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageSummary {
    /// Oldest row still on disk, so a percentage is never read as all-time.
    pub since: Option<String>,
    pub rows: u64,
    pub recall: UsageTotals,
    pub find: UsageTotals,
    /// Rows that could not be parsed, counted rather than hidden.
    pub unreadable_rows: u64,
}

/// Append-only JSONL. A failed append is reported to stderr and never fails the retrieval
/// that produced it: a bookkeeping error must not cost the caller their answer.
pub struct UsageLog {
    path: PathBuf,
}

impl UsageLog {
    pub fn new(root: &Path) -> Self {
        Self {
            path: root.join("usage.jsonl"),
        }
    }

    pub fn append(&self, row: &UsageRow) {
        let line = match serde_json::to_string(row) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("cyberbrain: usage row could not be serialised: {e}");
                return;
            }
        };
        let write = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = write {
            eprintln!("cyberbrain: usage log append failed: {e}");
        }
    }

    pub fn summary(&self) -> UsageSummary {
        let mut s = UsageSummary::default();
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return s;
        };
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(r) = serde_json::from_str::<UsageRow>(line) else {
                s.unreadable_rows += 1;
                continue;
            };
            s.rows += 1;
            if s.since.is_none() {
                s.since = Some(r.at.clone());
            }
            let t = match r.op.as_str() {
                "find" => &mut s.find,
                _ => &mut s.recall,
            };
            t.ops += 1;
            t.returned += r.returned;
            t.full += r.full;
            t.hits += r.hits;
        }
        s
    }
}

pub fn now() -> String {
    let t = jiff::Timestamp::now();
    t.round(jiff::Unit::Second).unwrap_or(t).to_string()
}

/// What the local model cost, per task, as the audit log recorded it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TaskUsage {
    pub calls: u64,
    /// Calls that did not end in `ok`. They still cost time, and often tokens.
    pub failed: u64,
    pub prompt_tokens: u64,
    /// Part of `prompt_tokens` the server answered from its prompt cache.
    pub cached_prompt_tokens: u64,
    pub completion_tokens: u64,
    pub elapsed_ms: u64,
    /// Calls where the endpoint reported no counts at all.
    pub calls_without_counts: u64,
    /// Calls that reported counts but said nothing about cache hits. Their prompts are in
    /// `prompt_tokens` and cannot be in `cached_prompt_tokens`, so a cache share computed
    /// over all calls would be too low. This number says by how many calls.
    pub calls_without_cache_report: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct InferenceUsage {
    pub tasks: std::collections::BTreeMap<String, TaskUsage>,
    /// First and last chat call in the log.
    pub first: Option<String>,
    pub last: Option<String>,
}

/// One calendar day (UTC), so a 30-day chart has a continuous axis. Days where nothing
/// happened are present and zero; a gap in a time series is not the same as a zero, and the
/// axis has to show which one it is looking at.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DayBucket {
    /// `YYYY-MM-DD`, UTC, because every timestamp in the store is.
    pub date: String,
    pub recall: UsageTotals,
    pub find: UsageTotals,
    /// Chat completions that day.
    pub calls: u64,
    pub prompt_tokens: u64,
    pub cached_prompt_tokens: u64,
    pub completion_tokens: u64,
    /// Time this binary spent waiting for the model.
    pub wall_ms: u64,
    /// Average cores the endpoint burned, over the calls that could be attributed.
    pub endpoint_cores: Option<f64>,
    pub machine_cores: Option<f64>,
}

/// The days from `first` to `last` inclusive, as `YYYY-MM-DD`.
pub fn day_axis(days: usize) -> Vec<String> {
    let today = jiff::Timestamp::now()
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date();
    (0..days)
        .rev()
        .filter_map(|i| today.checked_sub(jiff::Span::new().days(i as i64)).ok())
        .map(|d| d.to_string())
        .collect()
}

/// `2026-09-05T21:38:53Z` -> `2026-09-05`. Anything shorter is not a date and is dropped.
pub fn day_of(ts: &str) -> Option<&str> {
    (ts.len() >= 10).then(|| &ts[..10])
}
