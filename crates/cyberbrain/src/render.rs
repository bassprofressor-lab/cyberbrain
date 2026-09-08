//! Human-readable output. Every report is also `Serialize`; `--json` bypasses this module
//! entirely so the two cannot disagree about the numbers, only about the prose.

use crate::app::{
    ConsentReport, DoctorReport, EmbedderSummary, Expanded, FindReport, InitReport, NoteView,
    RetentionReport, ScanReport, StatusReport, WrittenNote,
};
use cyberbrain_core::{RecallResult, Slash};
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
        Slash(&r.store),
        Slash(&r.config),
        Slash(&r.audit_db),
        Slash(&r.index_db)
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
        let _ = writeln!(s, "  skipped {}: {}", Slash(&sk.path), sk.reason);
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
    let _ = writeln!(s, "{}", Slash(&n.path));
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
        Slash(&w.path),
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
    let _ = writeln!(s, "store: {}", Slash(&r.store));
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
        Slash(&r.audit.path),
        match &r.audit.chain {
            Ok(n) => format!("verified over {n} rows"),
            Err(e) => format!("BROKEN: {e}"),
        }
    );
    let _ = writeln!(s, "embedding: {}", embedder_line(&r.embedding.embedder));
    let _ = writeln!(s, "  model dir: {}", Slash(&r.embedding.model_dir));
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

/// The obligation catalogue. Grouped by nothing and sorted by nothing: the order in the
/// profile is the order a reader gets, because it runs scope first and sanctions last, which
/// is how the law reads. Confidence is printed on every line, including `high`, so the label
/// is a fact about each line rather than a warning that only appears when something is
/// shaky.
pub fn obligations(v: &crate::app::ObligationsView) -> String {
    let mut s = format!(
        "Profile {}: {}\n{} obligation(s) this profile encodes.\n\n",
        v.profile.as_str(),
        v.law,
        v.obligations.len()
    );
    for o in &v.obligations {
        let _ = writeln!(
            s,
            "{:?}  [{}]\n  {}\n  basis: {}",
            o.topic,
            o.confidence.as_str(),
            o.summary,
            o.basis
        );
        if !o.note.is_empty() {
            let _ = writeln!(s, "  note: {}", o.note);
        }
        s.push('\n');
    }
    s.push_str(
        "Confidence is the author's, not counsel's: high means the rule was verified in the \
         primary text, low means do not repeat it to a regulator without checking. Nothing \
         below high drives behaviour in code.\n",
    );
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
    let _ = writeln!(s, "{}", cyberbrain_policy::egress::TLS_TRUST);
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
        Slash(&r.path)
    );
    for w in &r.warnings {
        let _ = writeln!(s, "  note: {w}");
    }
    s
}

/// `find` (SPEC §10). The path and line range come first on every line, because the whole
/// point of this command is that the caller reads a slice instead of a file, and the slice
/// coordinates are what they need to paste into a reader.
pub fn find(r: &FindReport) -> String {
    let mut o = String::new();
    if r.hits.is_empty() {
        o.push_str(&format!("no definition of `{}` found\n", r.symbol));
    }
    for h in &r.hits {
        o.push_str(&format!(
            "{}:{}-{}  {} {}{}\n",
            h.path,
            h.start_line,
            h.end_line,
            h.kind,
            h.scope
                .as_ref()
                .map(|s| format!("{s}::"))
                .unwrap_or_default(),
            h.name,
        ));
        for line in h.snippet.lines().take(3) {
            o.push_str(&format!("     {line}\n"));
        }
    }

    // Named by the side of the boundary they count (SPEC §14.3): what was read, and then
    // what was passed over and why. A bare total would hide the skips entirely, and the
    // skips are the interesting half — a vendored copy silently outranking live source is
    // the failure §10 exists to prevent.
    o.push_str(&format!(
        "{} of {} match(es) shown; {} files scanned, {} definitions indexed; {} ms\n",
        r.hits.len(),
        r.matched_total,
        r.files_scanned,
        r.definitions_indexed,
        r.elapsed_ms,
    ));

    let s = &r.skipped;
    let skips = [
        (s.ignored_entries, "by .cyberbrainignore"),
        (s.gitignored_entries, "by .gitignore"),
        (s.hidden_entries, "hidden"),
        (s.store_entries, "the store itself"),
        (s.symlinks, "symlinks, never followed"),
        (s.lockfiles, "lockfiles"),
        (s.too_large, "over the size cap"),
        (s.binary, "binary"),
    ];
    let listed: Vec<String> = skips
        .iter()
        .filter(|(n, _)| *n > 0)
        .map(|(n, why)| format!("{n} {why}"))
        .collect();
    if !listed.is_empty() {
        o.push_str(&format!("not entered: {}\n", listed.join(", ")));
    }
    for u in &s.unreadable {
        o.push_str(&format!("unreadable: {} ({})\n", Slash(&u.path), u.reason));
    }
    for c in &r.caveats {
        o.push_str(&format!("caveat: {c}\n"));
    }
    o
}

/// The verdict on an audit export, for a person who was handed a file.
///
/// Written to be readable by someone who did not make the file and may not know the tool:
/// what was checked, over which period, and the anchor it started from.
pub fn verify_export(r: &cyberbrain_policy::bundle::Report) -> String {
    let mut s = String::new();
    s.push_str(&format!("chain holds over {} row(s)\n", r.rows));
    let period = match (&r.from, &r.to) {
        (Some(f), Some(t)) => format!("{f} to {t}"),
        (Some(f), None) => format!("{f} onwards"),
        (None, Some(t)) => format!("up to {t}"),
        (None, None) => "the whole log".to_string(),
    };
    s.push_str(&format!("period:   {period}\n"));
    s.push_str(&format!("anchor:   {}\n", r.anchor));
    if let Some(h) = &r.last_hash {
        s.push_str(&format!("last row: {h}\n"));
    }
    s.push_str(&format!("written:  {} by {}\n", r.exported_at, r.tool));
    s.push_str(
        "\nThis says the file is internally intact and starts where it says it does.\n\
         Whether the anchor belongs to that machine's real history is a question only the\n\
         full log answers.\n",
    );
    s
}

/// `install`. The shape of it is one block per client, because the question a person has
/// afterwards is "did it do the thing for Claude Desktop", not "how many files were there".
///
/// Every file is named in full. Two of them are the normal case on Windows, and a person
/// who has been bitten by an entry that never loaded needs to see which one was written.
pub fn install(r: &crate::install::Report) -> String {
    use crate::install::Action;
    let mut s = String::new();
    let verb = if r.undo { "Removing" } else { "Setting up" };
    let _ = writeln!(s, "{verb} {}", Slash(&r.binary));
    let _ = writeln!(s, "  store: {}", Slash(&r.store));
    if r.dry_run {
        let _ = writeln!(s, "  --dry-run: nothing was written");
    }

    for c in &r.clients {
        let _ = writeln!(s);
        if !c.found {
            let _ = writeln!(s, "{} — not found", c.client);
            if let Some(note) = &c.note {
                let _ = writeln!(s, "  {note}");
            }
            continue;
        }
        let _ = writeln!(s, "{}", c.client);
        for change in &c.changes {
            let did = match change.action {
                Action::Added => "added",
                Action::Updated => "updated",
                Action::Unchanged => "already set",
                Action::Removed => "removed",
                Action::NothingToUndo => "nothing of ours",
            };
            let _ = writeln!(s, "  {did:<15} {}", Slash(&change.path));
            let _ = writeln!(s, "  {:<15} {}", "", change.why);
            if let Some(b) = &change.backup {
                let _ = writeln!(s, "  {:<15} previous file kept as {}", "", Slash(b));
            }
        }
        if let Some(note) = &c.note {
            let _ = writeln!(s, "  {note}");
        }
        if let Some(snippet) = &c.snippet {
            for line in snippet.lines() {
                // A blank line stays blank: four spaces of indent on an empty line is
                // whitespace somebody's editor will flag when they paste this.
                if line.is_empty() {
                    let _ = writeln!(s);
                } else {
                    let _ = writeln!(s, "    {line}");
                }
            }
        }
    }

    if !r.undo && !r.dry_run && r.clients.iter().any(|c| c.found && !c.changes.is_empty()) {
        let _ = writeln!(
            s,
            "\nRestart the client for it to read this. `cyberbrain install --undo` takes it \
             back out."
        );
    }
    s
}

/// `propose`. Says where it went and what has to happen next, because a proposal that
/// nobody is told to review is a file in a folder.
pub fn proposed(r: &crate::app::ProposeReport) -> String {
    let mut s = format!(
        "Proposed {} for ring {} as {}\n  by: {}\n  {} bytes",
        r.name,
        r.ring.as_u8(),
        Slash(&r.path),
        r.proposed_by,
        r.bytes
    );
    if r.redacted > 0 {
        let _ = write!(s, ", {} redaction(s)", r.redacted);
    }
    s.push('\n');
    if r.changes_existing {
        let _ = writeln!(
            s,
            "  a note named {} already exists: accepting this changes it",
            r.name
        );
    }
    if r.dry_run {
        s.push_str("  --dry-run: nothing was written\n");
    }
    s.push_str(
        "\nIt is not in the index, so recall cannot find it. Somebody else runs \
         `cyberbrain review` to see it.\n",
    );
    s
}

/// `review` with no name: what is waiting.
pub fn proposals(list: &[crate::app::ProposalSummary]) -> String {
    if list.is_empty() {
        return "Nothing is waiting.\n".to_string();
    }
    let mut s = format!("{}\n\n", plural(list.len(), "proposal", "proposals"));
    for p in list {
        let _ = writeln!(s, "  {}  r{}  {}", p.name, p.ring.as_u8(), p.kind);
        match &p.proposed_by {
            Some(by) => {
                let _ = writeln!(s, "      by {by}, {}", p.created);
            }
            // Not a missing detail: a file that arrived in proposals/ some other way.
            None => {
                let _ = writeln!(
                    s,
                    "      no `note.proposed` row in the audit log; this cannot be accepted"
                );
            }
        }
        if p.changes_existing {
            let _ = writeln!(s, "      changes the existing note of that name");
        }
    }
    s.push_str("\n`cyberbrain review <name> --accept`, or `--reject --reason \"…\"`.\n");
    s
}

/// `review --accept` / `--reject`.
pub fn reviewed(r: &crate::app::ReviewReport) -> String {
    let mut s = if r.accepted {
        format!(
            "Accepted {} (proposed by {}, accepted by {})\n",
            r.name, r.proposed_by, r.by
        )
    } else {
        format!(
            "Rejected {} (proposed by {}, rejected by {})\n",
            r.name, r.proposed_by, r.by
        )
    };
    if let Some(path) = &r.path {
        let _ = writeln!(
            s,
            "  {} — {} block(s), {} vector(s)",
            Slash(path),
            r.blocks,
            r.vectors
        );
    }
    if let Some(reason) = &r.reason {
        let _ = writeln!(s, "  reason: {reason}");
        s.push_str("  the proposal file is gone; the reason is in the audit log\n");
    }
    if r.dry_run {
        s.push_str("  --dry-run: nothing was written\n");
    }
    s
}

#[cfg(test)]
mod tests {
    //! The one property every renderer shares: a path reaches the reader with forward
    //! slashes, whatever separator the platform built it with (`cyberbrain_core::path`).
    //! Paths are built from components so the separator logic runs on every OS, and the
    //! assertions are on whole reports rather than one per call site.
    use super::*;
    use std::path::PathBuf;

    fn p(parts: &[&str]) -> PathBuf {
        parts.iter().collect()
    }

    fn init_report() -> InitReport {
        InitReport {
            store: p(&["proj", ".cyberbrain"]),
            config: p(&["proj", ".cyberbrain", "config.toml"]),
            audit_db: p(&["proj", ".cyberbrain", "audit.db"]),
            index_db: p(&["proj", ".cyberbrain", "index.db"]),
            next_steps: vec![],
        }
    }

    #[test]
    fn a_rendered_report_spells_its_paths_with_forward_slashes() {
        let text = init(&init_report());
        assert!(!text.contains('\\'), "{text}");
        assert!(
            text.contains("Created a store at proj/.cyberbrain\n"),
            "{text}"
        );
        assert!(
            text.contains("config: proj/.cyberbrain/config.toml\n"),
            "{text}"
        );

        let c = consent(&ConsentReport {
            path: p(&["proj", ".cyberbrain", "config.toml"]),
            consent: true,
            model_source: None,
            warnings: vec![],
        });
        assert!(
            c.contains("written to proj/.cyberbrain/config.toml\n"),
            "{c}"
        );
    }

    /// `--json` and the MCP `structuredContent` are the same serialisation; a `PathBuf`
    /// field must not reach either with the platform separator.
    #[test]
    fn a_json_report_spells_its_paths_with_forward_slashes() {
        let v = serde_json::to_value(init_report()).unwrap();
        assert_eq!(v["store"], "proj/.cyberbrain");
        assert_eq!(v["index_db"], "proj/.cyberbrain/index.db");
    }
}
