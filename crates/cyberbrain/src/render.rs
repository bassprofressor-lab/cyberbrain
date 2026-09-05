//! Human-readable output. Every report is also `Serialize`; `--json` bypasses this module
//! entirely so the two cannot disagree about the numbers, only about the prose.

use crate::app::{
    ConsentReport, DoctorReport, EmbedderSummary, Expanded, InitReport, NoteView, RetentionReport,
    ScanReport, StatusReport, WrittenNote,
};
use cyberbrain_core::RecallResult;
use cyberbrain_policy::{EgressEntry, ModelCard};
use std::fmt::Write as _;

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

pub fn init(r: &InitReport) -> String {
    let mut s = format!(
        "Created a store at {}\n  config: {}\n  audit record: {}\n  index cache: {}\n\nNext steps:\n",
        r.store.display(),
        r.config.display(),
        r.audit_db.display(),
        r.index_db.display()
    );
    for step in &r.next_steps {
        let _ = writeln!(s, "  - {step}");
    }
    s
}

fn embedder_line(e: &EmbedderSummary) -> String {
    match (&e.profile_id, &e.reason) {
        (Some(id), _) => format!("embedding model loaded: {id} (dim {})", e.dim.unwrap_or(0)),
        (None, Some(r)) => format!("no embedding model: {r}"),
        (None, None) => "no embedding model".to_string(),
    }
}

pub fn scan(r: &ScanReport) -> String {
    let mut s = String::new();
    if r.dry_run {
        s.push_str(
            "[dry run] nothing was written; the counts below are what a real scan would do\n",
        );
    }
    if let Some(c) = &r.cleared {
        let _ = writeln!(
            s,
            "--full: cleared the index first ({} notes, {} blocks, {} vectors, {} links)",
            c.notes, c.blocks, c.vectors, c.links_out
        );
    }
    let mut parts = vec![
        plural(
            r.indexed_new,
            "note indexed for the first time",
            "notes indexed for the first time",
        ),
        plural(
            r.reindexed_changed,
            "note reindexed (content changed)",
            "notes reindexed (content changed)",
        ),
        plural(r.unchanged, "unchanged", "unchanged"),
    ];
    if r.revectorised > 0 {
        parts.push(format!(
            "{} re-embedded (content unchanged, vectors were missing)",
            r.revectorised
        ));
    }
    if r.touched_only > 0 {
        parts.push(format!(
            "{} touched (mtime moved, content identical)",
            r.touched_only
        ));
    }
    if !r.dropped_missing_file.is_empty() {
        parts.push(format!(
            "{} dropped from the index (file gone: {})",
            r.dropped_missing_file.len(),
            r.dropped_missing_file.join(", ")
        ));
    }
    if !r.skipped.is_empty() {
        parts.push(format!("{} skipped", r.skipped.len()));
    }
    let _ = writeln!(
        s,
        "{} of {} files listed: {}",
        r.files_listed - r.skipped.len(),
        r.files_listed,
        parts.join(", ")
    );
    for sk in &r.skipped {
        let _ = writeln!(s, "  skipped {}: {}", sk.path.display(), sk.reason);
    }
    if r.links_written_back > 0 {
        let _ = writeln!(
            s,
            "links written back into frontmatter: {}",
            r.links_written_back
        );
    }
    for f in &r.link_writeback_failed {
        let _ = writeln!(s, "  links NOT written back: {f}");
    }
    for o in &r.oversized_blocks {
        let _ = writeln!(
            s,
            "  oversized block: {} #{} is ~{} tokens ({}); kept whole",
            o.note, o.block_idx, o.approx_tokens, o.reason
        );
    }
    let _ = writeln!(s, "{}", embedder_line(&r.embedder));
    if let Some(p) = &r.profile_change
        && p.changed
    {
        let _ = writeln!(
            s,
            "embedding profile changed to {} (was {}); {} stored vectors wiped and rebuilt",
            p.current.id,
            p.previous.as_ref().map(|x| x.id.as_str()).unwrap_or("none"),
            p.vectors_wiped
        );
    }
    let _ = writeln!(
        s,
        "index now: {} notes, {} blocks, {} vectors, {} links ({} dangling); {} ms",
        r.index.notes,
        r.index.blocks,
        r.index.vectors,
        r.index.links,
        r.index.dangling_links,
        r.elapsed_ms
    );
    if r.dry_run && !r.audit_preview.is_empty() {
        let _ = writeln!(
            s,
            "audit rows a real run would append: {}",
            r.audit_preview.join(", ")
        );
    }
    s
}

pub fn recall(r: &RecallResult) -> String {
    let mut s = String::new();
    if r.hits.is_empty() {
        s.push_str("no hits\n");
    }
    let top = r
        .hits
        .first()
        .map(|h| h.score)
        .unwrap_or(1.0)
        .max(f32::EPSILON);
    for (i, h) in r.hits.iter().enumerate() {
        let _ = writeln!(
            s,
            "{}. {}  {}  {}  ({:.0}% of top)",
            i + 1,
            h.citation,
            h.ring,
            h.note_name,
            h.score / top * 100.0
        );
        for line in h.text.lines() {
            let _ = writeln!(s, "     {line}");
        }
    }
    for c in &r.conflicts {
        let _ = writeln!(
            s,
            "CONFLICT: {} wins over {}: {}",
            c.winner, c.loser, c.reason
        );
    }
    for c in &r.caveats {
        let _ = writeln!(s, "caveat: {c}");
    }
    s
}

pub fn note(n: &NoteView) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}", n.path.display());
    let _ = writeln!(
        s,
        "id: {}  name: {}  ring: {}  kind: {}  pii: {:?}",
        n.front.id, n.front.name, n.front.ring, n.kind, n.front.pii
    );
    if !n.blocks.is_empty() {
        let _ = writeln!(s, "blocks: {}", n.blocks.join(" "));
    }
    s.push('\n');
    s.push_str(&n.body);
    if !n.body.ends_with('\n') {
        s.push('\n');
    }
    s
}

pub fn expanded(e: &Expanded) -> String {
    let mut s = format!(
        "{} is block #{} of {} ({}), ~{} tokens:\n\n",
        e.citation, e.block.idx, e.note.front.name, e.note.front.ring, e.block.token_count
    );
    s.push_str(&e.block.text);
    s.push_str("\n\n--- full note ---\n");
    s.push_str(&note(&e.note));
    s
}

pub fn written(w: &WrittenNote) -> String {
    let mut s = String::new();
    if w.dry_run {
        s.push_str("[dry run] ");
    }
    let _ = writeln!(
        s,
        "{} {} in ring {} as {} ({} bytes, {} blocks, {} vectors, {} links, pii: {:?})",
        if w.dry_run {
            "would write"
        } else if w.created {
            "wrote"
        } else {
            "updated"
        },
        w.name,
        w.ring,
        w.path.display(),
        w.bytes,
        w.blocks,
        w.vectors,
        w.links,
        w.pii
    );
    if w.redacted > 0 {
        let _ = writeln!(s, "{} finding(s) redacted", w.redacted);
    }
    let _ = writeln!(s, "id: {}", w.id);
    if let Some(r) = &w.embedder_reason {
        let _ = writeln!(s, "no vectors: {r}");
    }
    if w.dry_run && !w.audit_preview.is_empty() {
        let _ = writeln!(
            s,
            "audit rows a real run would append: {}",
            w.audit_preview.join(", ")
        );
    }
    s
}

pub fn doctor(r: &DoctorReport) -> String {
    let mut s = String::new();
    if r.findings.is_empty() {
        let _ = writeln!(s, "clean: {} checks, nothing to report", r.checks_run.len());
    } else {
        let errors = r.findings.iter().filter(|f| f.severity == "error").count();
        let _ = writeln!(
            s,
            "{} finding(s) from {} checks ({} errors, {} warnings)",
            r.findings.len(),
            r.checks_run.len(),
            errors,
            r.findings.len() - errors
        );
    }
    for f in &r.findings {
        let _ = writeln!(s, "  [{}] {}: {}", f.severity, f.check, f.detail);
    }
    let _ = writeln!(s, "checks: {}", r.checks_run.join(", "));
    s
}

pub fn status(r: &StatusReport) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "store: {}", r.store.display());
    let _ = writeln!(
        s,
        "notes on disk: {} (r0 {}, r1 {}, r2 {}, r3 {}, r4 {}); {} files skipped",
        r.notes_on_disk,
        r.notes_per_ring[0],
        r.notes_per_ring[1],
        r.notes_per_ring[2],
        r.notes_per_ring[3],
        r.notes_per_ring[4],
        r.files_skipped
    );
    let _ = writeln!(
        s,
        "resident rings 0+1: ~{} of {} tokens",
        r.resident_tokens, r.resident_cap
    );
    let _ = writeln!(
        s,
        "index: schema v{}, {} notes, {} blocks, {} vectors, {} links ({} dangling){}",
        r.index.schema_version,
        r.index.notes,
        r.index.blocks,
        r.index.vectors,
        r.index.links,
        r.index.dangling_links,
        if r.index_stale {
            "  STALE: run `cyberbrain scan`"
        } else {
            ""
        }
    );
    let _ = writeln!(
        s,
        "audit: {} rows in {}, chain {}",
        r.audit.rows,
        r.audit.path.display(),
        match &r.audit.chain {
            Ok(n) => format!("verified over {n} rows"),
            Err(e) => format!("BROKEN: {e}"),
        }
    );
    let _ = writeln!(s, "embedding: {}", embedder_line(&r.embedding.embedder));
    let _ = writeln!(s, "  model dir: {}", r.embedding.model_dir.display());
    match (&r.embedding.index_profile, r.embedding.matches_index) {
        (Some(p), Some(true)) => {
            let _ = writeln!(
                s,
                "  index vectors from {} (matches the loaded model)",
                p.id
            );
        }
        (Some(p), Some(false)) => {
            let _ = writeln!(
                s,
                "  index vectors from {} which is NOT the loaded model; semantic search is disabled until `cyberbrain scan --full`",
                p.id
            );
        }
        (Some(p), None) => {
            let _ = writeln!(
                s,
                "  index vectors from {} (no model loaded to compare)",
                p.id
            );
        }
        (None, _) => {
            let _ = writeln!(s, "  index holds no vectors");
        }
    }
    let _ = writeln!(
        s,
        "inference: {} model {}: {}",
        r.inference.endpoint,
        r.inference.model.as_deref().unwrap_or("(none)"),
        r.inference.state
    );
    if let Some(p) = &r.inference.probe {
        let _ = writeln!(
            s,
            "  backend {} ({} models listed, {} ms){}",
            p.backend,
            p.models.len(),
            p.latency.as_millis(),
            p.caveat
                .as_ref()
                .map(|c| format!("; {c}"))
                .unwrap_or_default()
        );
    }
    let _ = writeln!(
        s,
        "policy: profile {} ({}), PII scan {}",
        r.policy.profile.as_str(),
        r.policy.law,
        if r.policy.pii_scan_active {
            "active"
        } else {
            "off"
        }
    );
    for e in &r.policy.egress {
        let _ = writeln!(s, "  egress {:?}: {}", e.purpose, e.state);
    }
    s
}

pub fn egress(entries: &[EgressEntry]) -> String {
    let mut s = String::from("Every path by which bytes may leave this machine:\n\n");
    for e in entries {
        let _ = writeln!(
            s,
            "{:?}\n  destination: {}\n  data: {}\n  carries note content: {}\n  requires: {}\n  enabled: {}\n  state: {}\n",
            e.purpose,
            e.destination,
            e.data,
            if e.carries_note_content { "yes" } else { "no" },
            e.requires,
            if e.enabled { "yes" } else { "no" },
            e.state
        );
    }
    s.push_str("That is the entire list. Telemetry does not exist.\n");
    s
}

pub fn retention(r: &RetentionReport) -> String {
    let mut s = cyberbrain_policy::retention::render(&r.queue);
    for u in &r.unreadable {
        let _ = writeln!(s, "  not evaluated: {u}");
    }
    if r.applied_run {
        let _ = writeln!(
            s,
            "\n{}applied to {} due note(s):",
            if r.dry_run { "[dry run] " } else { "" },
            r.applied.len()
        );
        for a in &r.applied {
            match &a.result {
                Ok(rep) => s.push_str(&cyberbrain_policy::erasure::render(rep)),
                Err(e) => {
                    let _ = writeln!(s, "  {}: FAILED: {e}", a.name);
                }
            }
        }
    }
    s
}

pub fn model_cards(cards: &[ModelCard], absent: &[String]) -> String {
    let mut s = cyberbrain_policy::model_card::render_markdown(cards);
    for a in absent {
        let _ = writeln!(s, "\nNot in use: {a}");
    }
    s
}

pub fn consent(r: &ConsentReport) -> String {
    let mut s = format!(
        "model_download_consent = {} written to {}\n",
        r.consent,
        r.path.display()
    );
    for w in &r.warnings {
        let _ = writeln!(s, "  note: {w}");
    }
    s
}
