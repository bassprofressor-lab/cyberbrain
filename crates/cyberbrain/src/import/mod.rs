//! `cyberbrain import`: move a tree of Markdown into a store without losing anything.
//!
//! Generic on purpose. The code knows Markdown, headings, fences, frontmatter and names;
//! which files to take, how to cut them and where they land is entirely the plan's
//! business ([`plan`]). Nothing here is named after any tool that wrote the corpus.
//!
//! Every note goes through [`App::write`], so the PII gate, the audit row, the resident
//! cap and the reindex apply to an imported note exactly as to a typed one, and
//! `--dry-run` is the same code path with the no-op writers (SPEC §8.2) — this module
//! never touches the notes tree or the index itself.
//!
//! The one requirement above all others is the reconciliation in [`report`]: what was
//! found, what became a note, and every item that did not, by name and with a reason.

pub mod plan;
pub mod suggest;
pub mod report;
mod slug;
mod split;
mod walk;

#[cfg(test)]
mod tests;

use crate::app::{App, WriteOutcome, WriteRequest};
use cyberbrain_core::blocks::approx_tokens;
use cyberbrain_core::{Error, NoteKind, Result, Ring, Slash};
use cyberbrain_policy::{Finding, OperatorChoice};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

pub use plan::{ImportPlan, load_plan};
pub use report::{ImportReport, Outcome};

use plan::{Collisions, Existing, Group, Split};
use report::{ByteLoss, FileFailure, FileLedger, ItemLedger, ItemRecord, SkippedFile, StaleNote};
use split::{Head, Item, Mode};
use walk::{Class, Found};

/// Tag prefix that ties a note to the file it was cut from. Re-import recognition and the
/// stale check both key on it.
pub const SOURCE_TAG_PREFIX: &str = "src:";

/// Run the plan against the store. `dry_run` swaps the writers, not the path.
pub fn import(app: &App, plan: &ImportPlan, dry_run: bool) -> Result<ImportReport> {
    let started = Instant::now();
    let root = plan.root()?.to_path_buf();
    if !root.is_dir() {
        return Err(Error::Config(format!(
            "import root {} is not a directory",
            Slash(&root)
        )));
    }

    // ---- files: two walks, one classification --------------------------------------
    let on_disk = walk::count_files_independently(&root)?;
    let listed = walk::list_files(&root)?;
    let mut files = FileLedger {
        on_disk,
        listed: listed.len(),
        ..FileLedger::default()
    };
    let mut mapped: Vec<(Found, usize)> = Vec::new();
    for f in listed {
        match walk::classify(plan, &f.rel) {
            Class::Skip(i) => files.skipped.push(SkippedFile {
                path: f.rel,
                rule: i,
                reason: plan.skip[i].reason.clone(),
            }),
            Class::Group(g) => mapped.push((f, g)),
            Class::Unmapped => files.unmapped.push(f.rel),
        }
    }
    files.mapped = mapped.len();
    // Report order: by rule, then path, so a rule's files sit together.
    files
        .skipped
        .sort_by(|a, b| a.rule.cmp(&b.rule).then(a.path.cmp(&b.path)));

    // ---- items: cut every mapped file into candidates -------------------------------
    let mut file_failures = Vec::new();
    let mut cands: Vec<Candidate> = Vec::new();
    for (f, gi) in &mapped {
        let group = &plan.groups[*gi];
        let text = match std::fs::read(&f.abs) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(t) => t,
                Err(e) => {
                    file_failures.push(FileFailure {
                        path: f.rel.clone(),
                        group: group.name.clone(),
                        reason: format!(
                            "not valid UTF-8 (byte {}); fix the encoding or skip it by rule",
                            e.utf8_error().valid_up_to()
                        ),
                    });
                    continue;
                }
            },
            Err(e) => {
                file_failures.push(FileFailure {
                    path: f.rel.clone(),
                    group: group.name.clone(),
                    reason: format!("cannot read: {e}"),
                });
                continue;
            }
        };
        let (bytes_read, bytes_in_items) = cut(plan, *gi, f, &text, &mut cands);
        if bytes_read == 0 {
            files.empty.push(f.rel.clone());
        } else if bytes_read != bytes_in_items {
            files.byte_loss.push(ByteLoss {
                path: f.rel.clone(),
                bytes_read,
                bytes_in_items,
            });
        }
    }

    // ---- names: collisions within this run ------------------------------------------
    resolve_collisions(plan, &mut cands);

    // ---- resident cap: rings 0/1 are checked up front so dry and real runs agree ------
    resident_cap_check(app, &mut cands)?;

    // ---- write, one by one, through App::write --------------------------------------
    let mut records = Vec::with_capacity(cands.len());
    let mut ledger = ItemLedger {
        found: cands.len(),
        ..ItemLedger::default()
    };
    let mut audit_preview = Vec::new();
    let mut produced: HashSet<String> = HashSet::new();
    for c in cands {
        let outcome = match c.pre {
            Some(o) => o,
            None => match write_one(app, plan, &c, dry_run, &mut audit_preview) {
                Ok(o) => o,
                Err(e) => Outcome::Failed {
                    error: e.to_string(),
                },
            },
        };
        match &outcome {
            Outcome::Written { .. } => ledger.written += 1,
            Outcome::Unchanged => ledger.unchanged += 1,
            Outcome::Updated { .. } => ledger.updated += 1,
            Outcome::Held { .. } => ledger.held += 1,
            Outcome::Duplicate { .. } => ledger.duplicate += 1,
            Outcome::Collision { .. } => ledger.collided += 1,
            Outcome::Exists { .. } => ledger.exists += 1,
            Outcome::Failed { .. } => ledger.failed += 1,
        }
        if !matches!(outcome, Outcome::Collision { .. } | Outcome::Failed { .. }) {
            produced.insert(c.name.clone());
        }
        records.push(ItemRecord {
            file: c.file.clone(),
            index: c.item.index,
            line: c.item.line,
            group: c.group.clone(),
            label: c.label.clone(),
            name: c.name.clone(),
            ring: c.ring.as_u8(),
            kind: kind_name(c.kind),
            bytes: c.body.len(),
            outcome,
            notes: c.notes.clone(),
        });
    }

    // ---- stale: earlier imports of these files that nothing produced now --------------
    let stale = stale_notes(app, &mapped, &produced)?;

    let mut report = ImportReport {
        dry_run,
        root: root.clone(),
        plan: plan.origin.clone(),
        accept_pii: plan.accept_pii,
        files,
        items: ledger,
        records,
        file_failures,
        stale,
        audit_preview,
        elapsed_ms: 0,
    };

    // One row for the run as a whole, with the counts, so the audit log carries the
    // reconciliation and not only the per-note writes.
    let summary = json!({
        "root": root,
        "plan": plan.origin,
        "files": { "on_disk": report.files.on_disk, "listed": report.files.listed, "mapped": report.files.mapped,
                   "skipped": report.files.skipped.len(), "unmapped": report.files.unmapped.clone() },
        "items": report.items,
        "file_failures": report.file_failures.len(),
        "balanced": report.is_balanced(),
        "dry_run": dry_run,
    });
    if dry_run {
        report
            .audit_preview
            .push(format!("import.completed {summary}"));
    } else {
        app.policy().audit().record_raw(
            &app.actor().to_string(),
            "import.completed",
            format!("import:{}", Slash(&root)),
            summary,
        )?;
    }
    report.elapsed_ms = started.elapsed().as_millis();
    Ok(report)
}

// ---------------------------------------------------------------------------------------

/// An item with everything decided except whether the store takes it.
struct Candidate {
    file: String,
    group: String,
    group_idx: usize,
    item: Item,
    label: String,
    name: String,
    ring: Ring,
    kind: NoteKind,
    tags: Vec<String>,
    bereich: Option<String>,
    retention: Option<String>,
    body: String,
    notes: Vec<String>,
    /// Decided before the write pass (collision, cap).
    pre: Option<Outcome>,
}

fn kind_name(k: NoteKind) -> String {
    match serde_json::to_value(k) {
        Ok(serde_json::Value::String(s)) => s,
        _ => format!("{k:?}").to_lowercase(),
    }
}

fn label_of(item: &Item) -> String {
    let text = item
        .heading
        .clone()
        .or_else(|| split::first_text_line(&item.body))
        .unwrap_or_default();
    let mut out: String = text.chars().take(60).collect();
    if out.len() < text.len() {
        out.push('…');
    }
    out
}

/// Cut one file into candidates according to its group. Returns the bytes it split and
/// the bytes that reached items, which must agree: the split is lossless by construction
/// and the caller checks that it stayed so. A blank file returns `(0, 0)`.
fn cut(
    plan: &ImportPlan,
    group_idx: usize,
    f: &Found,
    text: &str,
    out: &mut Vec<Candidate>,
) -> (usize, usize) {
    let group: &Group = &plan.groups[group_idx];
    let path = std::path::Path::new(&f.rel);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dir = path
        .parent()
        .map(|p| p.to_string_lossy().replace('/', "-"))
        .unwrap_or_default();
    let src_tag = format!("{SOURCE_TAG_PREFIX}{}", f.rel);
    let base_tags: Vec<String> = plan
        .tags
        .iter()
        .chain(group.tags.iter())
        .cloned()
        .chain([src_tag])
        .collect();

    let (mode, default_template) = match group.split {
        Split::File => (Mode::File, "{stem}"),
        Split::Heading => (
            Mode::Heading {
                level: group.heading_level,
            },
            "{stem}-{heading}",
        ),
        Split::Delimiter => (
            Mode::Delimiter {
                delimiter: group.delimiter.as_deref().unwrap_or("---"),
            },
            "{stem}-{heading}",
        ),
    };
    let template = group.note_name.as_deref().unwrap_or(default_template);
    let preamble_template = group.preamble_name.as_deref().unwrap_or("{stem}");

    // Frontmatter is only meaningful for a whole file; in split modes a head is text of
    // the preamble and stays there verbatim.
    let mut head_front = None;
    let mut head_notes = Vec::new();
    let mut body_text = text;
    if group.split == Split::File {
        match split::detect_head(&f.abs, text) {
            Head::None => {}
            Head::Cyberbrain { front, body } => {
                head_notes.push(format!(
                    "frontmatter reused (name, kind, tags, retention); id {} and its timestamps are not preserved because App::write assigns them",
                    front.id
                ));
                if front.ring != group.ring {
                    head_notes.push(format!(
                        "file says ring {}, plan says ring {}; the plan wins",
                        front.ring, group.ring
                    ));
                }
                head_front = Some((*front, body));
            }
            Head::Foreign { reason } => head_notes.push(format!(
                "leading `---` block is not a cyberbrain head ({reason}); kept in the body verbatim"
            )),
        }
    }
    let owned_body;
    if let Some((_, b)) = &head_front {
        owned_body = b.clone();
        body_text = &owned_body;
    }
    if body_text.trim().is_empty() {
        return (0, 0);
    }
    let bytes_read = body_text.len();
    let mut bytes_in_items = 0usize;

    for item in split::split(body_text, mode) {
        bytes_in_items += item.body.len();
        let mut notes = head_notes.clone();
        let heading_text = item
            .heading
            .clone()
            .or_else(|| split::first_text_line(&item.body))
            .unwrap_or_default();
        let parts = slug::NameParts {
            stem: stem.clone(),
            dir: dir.clone(),
            heading: heading_text,
            index: item.index,
            hash: slug::short_hash(&item.body),
        };
        let t = if item.is_preamble {
            preamble_template
        } else {
            template
        };
        let mut name = slug::render(t, &parts);
        let mut kind = group.kind;
        let mut tags = base_tags.clone();
        let mut retention = group.retention.clone();
        if let Some((front, _)) = &head_front {
            if group.note_name.is_none() {
                name = front.name.clone();
            }
            kind = front.kind;
            for tg in &front.tags {
                if !tags.contains(tg) {
                    tags.push(tg.clone());
                }
            }
            if front.retention.is_some() {
                retention = front.retention.clone();
            }
        }
        // A template that asks for the heading must get one: an item whose heading
        // slugs to nothing (emoji only, `./`) would otherwise silently take the file's
        // own name.
        let heading_missing = t.contains("{heading}") && slug::slugify(&parts.heading).is_empty();
        if heading_missing || !slug::is_valid(&name) {
            let fallback = slug::render("{stem}-{index}", &parts);
            notes.push(if heading_missing {
                format!(
                    "template `{t}` needs a heading and {:?} gives none; used `{fallback}`",
                    parts.heading
                )
            } else {
                format!("template `{t}` gave `{name}` which is not a note name; used `{fallback}`")
            });
            name = fallback;
        }
        if item.is_preamble {
            notes.push("text before the first boundary".into());
        }
        let label = label_of(&item);
        out.push(Candidate {
            file: f.rel.clone(),
            group: group.name.clone(),
            group_idx,
            body: item.body.clone(),
            item,
            label,
            name,
            ring: group.ring,
            kind,
            tags,
            bereich: group.bereich.clone(),
            retention,
            notes,
            pre: None,
        });
    }
    (bytes_read, bytes_in_items)
}

/// Items of one run that derive the same name. `hash` groups get `-{hash}` appended and
/// are re-checked; whatever still collides is reported and not written.
fn resolve_collisions(plan: &ImportPlan, cands: &mut [Candidate]) {
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        by_name.entry(c.name.clone()).or_default().push(i);
    }
    for (name, idxs) in &by_name {
        if idxs.len() < 2 {
            continue;
        }
        for &i in idxs {
            if plan.groups[cands[i].group_idx].collisions == Collisions::Hash {
                let hash = slug::short_hash(&cands[i].body);
                let suffixed = slug::slugify(&format!("{name}-{hash}"));
                cands[i].notes.push(format!(
                    "name `{name}` collided with {} other item(s); suffixed with the content hash",
                    idxs.len() - 1
                ));
                cands[i].name = suffixed;
            }
        }
    }
    // Second pass: what still shares a name. Byte-identical text is the same item twice
    // (a log that recorded a session twice): the first occurrence is the note and the
    // rest are duplicates of it, named. Different text is a collision and nothing is
    // written for any of them.
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        by_name.entry(c.name.clone()).or_default().push(i);
    }
    for idxs in by_name.values() {
        if idxs.len() < 2 {
            continue;
        }
        let first = idxs[0];
        let all_same = idxs.iter().all(|&j| cands[j].body == cands[first].body);
        for &i in idxs {
            if all_same {
                if i != first {
                    cands[i].pre = Some(Outcome::Duplicate {
                        of: format!(
                            "{} #{} line {}",
                            cands[first].file, cands[first].item.index, cands[first].item.line
                        ),
                    });
                }
                continue;
            }
            let with: Vec<String> = idxs
                .iter()
                .filter(|&&j| j != i)
                .map(|&j| {
                    format!(
                        "{} #{} line {}",
                        cands[j].file, cands[j].item.index, cands[j].item.line
                    )
                })
                .collect();
            cands[i].pre = Some(Outcome::Collision { with });
        }
    }
}

/// Rings 0 and 1 share a token cap that the real writer enforces per note and the no-op
/// writer does not check at all. Projecting it here keeps the two runs identical and
/// names the item that would cross the line instead of failing mid-way.
fn resident_cap_check(app: &App, cands: &mut [Candidate]) -> Result<()> {
    let store = app.store();
    let cap = store.resident_cap();
    let mut projected = store.resident_tokens()?;
    for c in cands.iter_mut() {
        if !c.ring.is_resident() || c.pre.is_some() {
            continue;
        }
        let new_tokens = approx_tokens(&c.body) as usize;
        // Replacing an existing resident note counts its new size, not old plus new.
        let old_tokens = match store.read(&c.name) {
            Ok(n) if n.front.ring.is_resident() => approx_tokens(&n.body) as usize,
            _ => 0,
        };
        let next = projected + new_tokens - old_tokens.min(projected);
        if next > cap {
            c.pre = Some(Outcome::Failed {
                error: format!(
                    "~{new_tokens} tokens would put rings 0+1 at ~{next}, over the cap of {cap} (SPEC §3.2); move this group to ring 2 or raise rings.resident_cap_tokens"
                ),
            });
            continue;
        }
        projected = next;
    }
    Ok(())
}

fn summarise(findings: &[Finding]) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for f in findings {
        *counts.entry(f.kind.to_string()).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(k, n)| format!("{k} ×{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One item into the store. Existence is decided here; the write itself, the scan and
/// the index are `App::write`'s.
fn write_one(
    app: &App,
    plan: &ImportPlan,
    c: &Candidate,
    dry_run: bool,
    audit_preview: &mut Vec<String>,
) -> Result<Outcome> {
    let group = &plan.groups[c.group_idx];
    let src_tag = c
        .tags
        .iter()
        .find(|t| t.starts_with(SOURCE_TAG_PREFIX))
        .cloned()
        .unwrap_or_default();
    let existing = match app.store().read(&c.name) {
        Ok(n) => Some(n),
        Err(Error::NoSuchNote(_)) => None,
        Err(e) => return Err(e),
    };
    let mut updating = false;
    if let Some(n) = &existing {
        if n.front.ring != c.ring {
            return Ok(Outcome::Exists {
                why: format!(
                    "already exists in ring {} (this item is bound for ring {}); a move must be explicit",
                    n.front.ring, c.ring
                ),
            });
        }
        if n.body == c.body && n.front.kind == c.kind {
            return Ok(Outcome::Unchanged);
        }
        let same_source = n.front.tags.contains(&src_tag);
        match (group.existing, same_source) {
            (Existing::Overwrite, true) => updating = true,
            (Existing::Overwrite, false) => {
                return Ok(Outcome::Exists {
                    why: "exists with different content and was not imported from this file (no matching src: tag); not overwritten".into(),
                });
            }
            (Existing::Keep, true) => {
                return Ok(Outcome::Exists {
                    why: "imported from this file earlier and differs now; set existing = \"overwrite\" on the group to replace it".into(),
                });
            }
            (Existing::Keep, false) => {
                return Ok(Outcome::Exists {
                    why: "a note of this name exists with different content".into(),
                });
            }
        }
    }
    let req = WriteRequest {
        ring: c.ring,
        kind: c.kind,
        name: c.name.clone(),
        body: c.body.clone(),
        tags: c.tags.clone(),
        bereich: c.bereich.clone().map(Some),
        retention: c.retention.clone(),
        force: false,
        choice: plan.accept_pii.then_some(OperatorChoice::MarkReviewed),
        expected_updated: None,
        dry_run,
    };
    Ok(match app.write(req)? {
        WriteOutcome::Written(w) => {
            audit_preview.extend(w.audit_preview);
            if updating {
                Outcome::Updated { path: w.path }
            } else {
                Outcome::Written {
                    path: w.path,
                    pii: w.pii,
                }
            }
        }
        WriteOutcome::Held { findings, .. } => Outcome::Held {
            findings: summarise(&findings),
        },
        WriteOutcome::Conflict { .. } => Outcome::Failed {
            error: "the store reported a concurrent change to a note this run never read".into(),
        },
    })
}

/// Notes tagged with a mapped file's `src:` tag whose name no current item produced.
fn stale_notes(
    app: &App,
    mapped: &[(Found, usize)],
    produced: &HashSet<String>,
) -> Result<Vec<StaleNote>> {
    let tags: HashMap<String, String> = mapped
        .iter()
        .map(|(f, _)| (format!("{SOURCE_TAG_PREFIX}{}", f.rel), f.rel.clone()))
        .collect();
    let store = app.store();
    let mut out = Vec::new();
    for e in store.list()?.entries {
        // Unreadable notes are `doctor`'s business; here they simply cannot be stale.
        let Ok(n) = store.read_path(&e.path) else {
            continue;
        };
        if produced.contains(&n.front.name) {
            continue;
        }
        if let Some(src) = n.front.tags.iter().find_map(|t| tags.get(t)) {
            out.push(StaleNote {
                name: n.front.name,
                ring: n.front.ring.as_u8(),
                source: src.clone(),
            });
        }
    }
    Ok(out)
}

/// Exit code for the CLI, so `main.rs` needs no knowledge of the ledger.
pub fn exit_code(report: &ImportReport) -> i32 {
    report.exit_code()
}

/// Human rendering, for `main.rs`.
pub fn render(report: &ImportReport) -> String {
    report.render()
}
