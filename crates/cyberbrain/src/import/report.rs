//! The reconciliation. Two ledgers, both closed explicitly:
//!
//! - **files**: `on_disk` (independent count) = `listed` = mapped + skipped + unmapped
//! - **bytes**: per mapped file, bytes read = bytes in items (the split is lossless)
//! - **items**: `found` = written + unchanged + updated + held + duplicate + collided +
//!   exists + failed
//!
//! Every item that did not become a note is listed by name with its reason. A total is
//! never printed as the difference of two other totals (SPEC §14.3, §14.4).

use cyberbrain_core::{PiiState, Slash};
use serde::Serialize;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct ImportReport {
    pub dry_run: bool,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub root: PathBuf,
    pub plan: String,
    pub accept_pii: bool,
    pub files: FileLedger,
    pub items: ItemLedger,
    /// Every item, in source order.
    pub records: Vec<ItemRecord>,
    /// Mapped files that could not be read or decoded. Their items are unknown, which
    /// is why this list is separate from the item ledger and fails the run.
    pub file_failures: Vec<FileFailure>,
    /// Notes imported earlier from a mapped file that no current item produced. Not
    /// erased (erasure is the operator's act); listed so the operator knows.
    pub stale: Vec<StaleNote>,
    /// Dry run only: the audit rows a real run would append.
    pub audit_preview: Vec<String>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FileLedger {
    /// Second, independent walk of the tree.
    pub on_disk: usize,
    /// What the importer's own walk listed.
    pub listed: usize,
    pub mapped: usize,
    pub skipped: Vec<SkippedFile>,
    pub unmapped: Vec<String>,
    /// Mapped files that were blank and so produced no item. Named, because a file
    /// that yields nothing is the shape of a silent drop even when it is legitimate.
    pub empty: Vec<String>,
    /// Bytes read from a file versus bytes handed on as items. The split is lossless by
    /// construction, so any entry here is an importer bug and fails the accounting.
    pub byte_loss: Vec<ByteLoss>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ByteLoss {
    pub path: String,
    pub bytes_read: usize,
    pub bytes_in_items: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedFile {
    pub path: String,
    pub rule: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ItemLedger {
    pub found: usize,
    pub written: usize,
    pub unchanged: usize,
    pub updated: usize,
    pub held: usize,
    /// Byte-identical text of another item of this run under the same name; the other
    /// item is the note. Nothing is lost, and it is still named.
    pub duplicate: usize,
    pub collided: usize,
    pub exists: usize,
    pub failed: usize,
}

impl ItemLedger {
    pub fn accounted(&self) -> usize {
        self.written
            + self.unchanged
            + self.updated
            + self.held
            + self.duplicate
            + self.collided
            + self.exists
            + self.failed
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FileFailure {
    pub path: String,
    pub group: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleNote {
    pub name: String,
    pub ring: u8,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemRecord {
    pub file: String,
    pub index: usize,
    pub line: usize,
    pub group: String,
    /// Heading or first line, shortened; what a human would call this item.
    pub label: String,
    pub name: String,
    pub ring: u8,
    pub kind: String,
    pub bytes: usize,
    pub outcome: Outcome,
    /// Things worth knowing that are not failures: a fallback name, reused frontmatter.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum Outcome {
    Written {
        path: PathBuf,
        pii: PiiState,
    },
    /// Already in the store with this content, ring and kind.
    Unchanged,
    /// Replaced an earlier import of the same item.
    Updated {
        #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
        path: PathBuf,
    },
    /// The PII gate held it (SPEC §12.4). `findings` summarises what was found.
    Held {
        findings: String,
    },
    /// Same name and byte-identical text as `of`, which is the note.
    Duplicate {
        of: String,
    },
    /// Another item of this run derived the same name with different text.
    Collision {
        with: Vec<String>,
    },
    /// A note of this name exists and is not the same item.
    Exists {
        why: String,
    },
    Failed {
        error: String,
    },
}

impl ImportReport {
    /// Both ledgers close. False means the importer lost track of something itself.
    pub fn is_balanced(&self) -> bool {
        let f = &self.files;
        f.on_disk == f.listed
            && f.listed == f.mapped + f.skipped.len() + f.unmapped.len()
            && f.byte_loss.is_empty()
            && self.items.found == self.items.accounted()
            && self.items.found == self.records.len()
    }

    /// Items that did not become notes and no rule said to skip.
    pub fn drops(&self) -> usize {
        self.items.collided + self.items.exists + self.items.failed + self.files.unmapped.len()
    }

    /// SPEC §8: 0 clean, 1 something did not make it, 2 the importer's own accounting
    /// failed, 3 only policy holds stand between the source and the store.
    pub fn exit_code(&self) -> i32 {
        if !self.is_balanced() {
            return 2;
        }
        if self.drops() > 0 || !self.file_failures.is_empty() {
            return 1;
        }
        if self.items.held > 0 {
            return 3;
        }
        0
    }

    pub fn render(&self) -> String {
        let mut s = String::new();
        if self.dry_run {
            s.push_str(
                "[dry run] nothing was written; every number below is what a real run would do\n",
            );
        }
        let _ = writeln!(s, "source: {}  (plan: {})", Slash(&self.root), self.plan);

        // ---- files
        let f = &self.files;
        let _ = writeln!(
            s,
            "files: {} on disk, {} listed = {} mapped + {} skipped by rule + {} unmapped",
            f.on_disk,
            f.listed,
            f.mapped,
            f.skipped.len(),
            f.unmapped.len()
        );
        if f.on_disk != f.listed {
            let _ = writeln!(
                s,
                "  !! the two walks disagree ({} vs {}): the importer's listing is not trustworthy",
                f.on_disk, f.listed
            );
        }
        if !f.unmapped.is_empty() {
            s.push_str(
                "  UNMAPPED (no rule matches; add a [[group]] or a [[skip]] with a reason):\n",
            );
            for p in &f.unmapped {
                let _ = writeln!(s, "    {p}");
            }
        }
        // Skipped, grouped by rule; each file named.
        let mut rule = 0usize;
        let mut first = true;
        for sk in &f.skipped {
            if first || sk.rule != rule {
                let _ = writeln!(s, "  skipped by rule #{} ({}):", sk.rule + 1, sk.reason);
                rule = sk.rule;
                first = false;
            }
            let _ = writeln!(s, "    {}", sk.path);
        }
        for bl in &f.byte_loss {
            let _ = writeln!(
                s,
                "  !! {}: {} bytes read but {} bytes reached items; the split lost content (importer bug)",
                bl.path, bl.bytes_read, bl.bytes_in_items
            );
        }
        for e in &f.empty {
            let _ = writeln!(s, "  empty (blank file, no note made): {e}");
        }
        for ff in &self.file_failures {
            let _ = writeln!(
                s,
                "  FILE NOT IMPORTED {} (group {}): {}",
                ff.path, ff.group, ff.reason
            );
        }

        // ---- items
        let it = &self.items;
        let _ = writeln!(
            s,
            "items: {} found = {} written + {} already imported + {} updated + {} held (PII) + {} duplicates + {} collisions + {} exist + {} failed",
            it.found,
            it.written,
            it.unchanged,
            it.updated,
            it.held,
            it.duplicate,
            it.collided,
            it.exists,
            it.failed
        );
        if it.found != it.accounted() {
            let _ = writeln!(
                s,
                "  !! UNBALANCED: {} item(s) found but {} accounted for",
                it.found,
                it.accounted()
            );
        }
        let mut section = |title: &str, pick: &dyn Fn(&ItemRecord) -> Option<String>| {
            let rows: Vec<String> = self
                .records
                .iter()
                .filter_map(|r| {
                    pick(r).map(|why| {
                        format!(
                            "    {} <- {} #{} line {} {:?}: {why}",
                            r.name, r.file, r.index, r.line, r.label
                        )
                    })
                })
                .collect();
            if !rows.is_empty() {
                let _ = writeln!(s, "  {title} ({}):", rows.len());
                for row in rows {
                    s.push_str(&row);
                    s.push('\n');
                }
            }
        };
        section(
            "held by the PII gate; re-run with --accept-pii to write them marked reviewed",
            &|r| match &r.outcome {
                Outcome::Held { findings } => Some(findings.clone()),
                _ => None,
            },
        );
        section(
            "duplicates: identical text under the same name as another item of this run; that item is the note",
            &|r| match &r.outcome {
                Outcome::Duplicate { of } => Some(format!("same bytes as {of}")),
                _ => None,
            },
        );
        section(
            "collisions: more than one item derived this name, nothing written for them; change note_name or set collisions = \"hash\"",
            &|r| match &r.outcome {
                Outcome::Collision { with } => Some(format!("also from {}", with.join(", "))),
                _ => None,
            },
        );
        section(
            "exist: a different note already has this name",
            &|r| match &r.outcome {
                Outcome::Exists { why } => Some(why.clone()),
                _ => None,
            },
        );
        section("failed", &|r| match &r.outcome {
            Outcome::Failed { error } => Some(error.clone()),
            _ => None,
        });
        section("updated", &|r| match &r.outcome {
            Outcome::Updated { path } => Some(cyberbrain_core::slash(path)),
            _ => None,
        });
        let noted: Vec<&ItemRecord> = self
            .records
            .iter()
            .filter(|r| !r.notes.is_empty())
            .collect();
        if !noted.is_empty() {
            let _ = writeln!(s, "  notes ({}):", noted.len());
            for r in noted {
                let _ = writeln!(
                    s,
                    "    {} <- {} #{}: {}",
                    r.name,
                    r.file,
                    r.index,
                    r.notes.join("; ")
                );
            }
        }

        // ---- per file
        s.push_str("per file:\n");
        let mut cur: Option<&str> = None;
        let mut counts = ItemLedger::default();
        let flush = |s: &mut String, file: &str, c: &ItemLedger| {
            let mut parts = vec![format!("{} written", c.written)];
            for (n, what) in [
                (c.unchanged, "already imported"),
                (c.updated, "updated"),
                (c.held, "held"),
                (c.duplicate, "duplicates"),
                (c.collided, "collisions"),
                (c.exists, "exist"),
                (c.failed, "failed"),
            ] {
                if n > 0 {
                    parts.push(format!("{n} {what}"));
                }
            }
            let _ = writeln!(s, "  {file}: {} items -> {}", c.found, parts.join(", "));
        };
        for r in &self.records {
            if cur != Some(r.file.as_str()) {
                if let Some(file) = cur {
                    flush(&mut s, file, &counts);
                }
                cur = Some(r.file.as_str());
                counts = ItemLedger::default();
            }
            counts.found += 1;
            match &r.outcome {
                Outcome::Written { .. } => counts.written += 1,
                Outcome::Unchanged => counts.unchanged += 1,
                Outcome::Updated { .. } => counts.updated += 1,
                Outcome::Held { .. } => counts.held += 1,
                Outcome::Duplicate { .. } => counts.duplicate += 1,
                Outcome::Collision { .. } => counts.collided += 1,
                Outcome::Exists { .. } => counts.exists += 1,
                Outcome::Failed { .. } => counts.failed += 1,
            }
        }
        if let Some(file) = cur {
            flush(&mut s, file, &counts);
        }

        if !self.stale.is_empty() {
            let _ = writeln!(
                s,
                "stale ({}): imported earlier from these files, no longer produced by any item; not erased",
                self.stale.len()
            );
            for st in &self.stale {
                let _ = writeln!(s, "    r{} {} <- {}", st.ring, st.name, st.source);
            }
        }
        if self.dry_run && !self.audit_preview.is_empty() {
            let _ = writeln!(
                s,
                "audit: a real run would append {} row(s)",
                self.audit_preview.len()
            );
        }
        let _ = writeln!(s, "elapsed: {} ms", self.elapsed_ms);

        let code = self.exit_code();
        let verdict = match code {
            0 => "clean: every source item is a note".to_string(),
            1 => format!(
                "NOT CLEAN: {} item(s)/file(s) did not make it and no rule said to skip them (listed above)",
                self.drops() + self.file_failures.len()
            ),
            2 => "ACCOUNTING FAILED: the importer's own ledgers do not close; trust nothing above"
                .to_string(),
            _ => format!(
                "held: {} item(s) wait on the PII gate and nothing else is wrong",
                self.items.held
            ),
        };
        let _ = writeln!(s, "result: {verdict}; exit {code}");
        s
    }
}
