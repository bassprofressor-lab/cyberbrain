//! Fixtures for the importer: each split mode, collisions, re-import, unmapped items,
//! the PII gate, the resident cap, and the ledger's own accounting.

use super::plan::ImportPlan;
use super::report::{ImportReport, ItemLedger, Outcome};
use super::{SOURCE_TAG_PREFIX, import};
use crate::app::App;
use cyberbrain_core::{PiiState, Ring};
use cyberbrain_policy::Actor;
use std::fs;
use std::path::{Path, PathBuf};

struct Fx {
    _dir: tempfile::TempDir,
    src: PathBuf,
    app: App,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("store");
    App::init(&store, &Actor::Operator).unwrap();
    let app = App::open(Some(&store), Actor::Operator).unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    Fx {
        _dir: dir,
        src,
        app,
    }
}

fn plan(root: &Path, body: &str) -> ImportPlan {
    let mut p = ImportPlan::parse(&format!("version = 1\n{body}"), "test").unwrap();
    p.root = Some(root.to_path_buf());
    p
}

fn write(src: &Path, rel: &str, text: &str) {
    let p = src.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn notes_on_disk(app: &App) -> Vec<(Ring, String)> {
    let mut v: Vec<(Ring, String)> = app
        .store()
        .list()
        .unwrap()
        .entries
        .into_iter()
        .map(|e| (e.ring, e.name))
        .collect();
    v.sort();
    v
}

/// (found, written, unchanged, updated, held, duplicate, collided, exists, failed)
fn counts(
    r: &ImportReport,
) -> (
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
) {
    let i = &r.items;
    (
        i.found,
        i.written,
        i.unchanged,
        i.updated,
        i.held,
        i.duplicate,
        i.collided,
        i.exists,
        i.failed,
    )
}

const HEADED: &str = "# Cerebrum\n\n> intro line\n\n## User Preferences\n\n- pref one\n- pref two\n\n## Key Learnings\n\n### Postgres 18 moves PGDATA\n\nbody with [[docker-bind-mount-inode-drift]] link.\n\n```md\n### not a heading, inside a fence\n```\n\n### Ärger mit Umlauten\n\nzweiter Abschnitt\n\n## Decision Log\n\nfinal text\n";

#[test]
fn heading_split_imports_every_section_losslessly() {
    let fx = fixture();
    write(&fx.src, "cerebrum.md", HEADED);
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"curated\"\npaths = [\"cerebrum.md\"]\nring = 2\nkind = \"knowledge\"\nsplit = \"heading\"\nheading_level = 3\ntags = [\"cerebrum\"]\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert!(r.is_balanced(), "{}", r.render());
    assert_eq!(counts(&r), (6, 6, 0, 0, 0, 0, 0, 0, 0), "{}", r.render());
    assert_eq!(r.exit_code(), 0);
    assert_eq!(r.files.on_disk, 1);
    assert_eq!(r.files.mapped, 1);

    let names: Vec<&str> = r.records.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "cerebrum",
            "cerebrum-user-preferences",
            "cerebrum-key-learnings",
            "cerebrum-postgres-18-moves-pgdata",
            "cerebrum-aerger-mit-umlauten",
            "cerebrum-decision-log",
        ]
    );
    assert!(
        r.records[0]
            .notes
            .iter()
            .any(|n| n.contains("before the first boundary"))
    );

    // Lossless end to end: the bodies read back from the store concatenate to the file.
    let mut joined = String::new();
    for rec in &r.records {
        let n = fx.app.store().read(&rec.name).unwrap();
        assert_eq!(n.front.ring, Ring::Knowledge);
        assert!(n.front.tags.contains(&"cerebrum".to_string()));
        assert!(
            n.front
                .tags
                .contains(&format!("{SOURCE_TAG_PREFIX}cerebrum.md"))
        );
        joined.push_str(&n.body);
    }
    assert_eq!(joined, HEADED);
    // The wikilink survived as written and was picked up as a link.
    let n = fx
        .app
        .store()
        .read("cerebrum-postgres-18-moves-pgdata")
        .unwrap();
    assert_eq!(n.front.links, ["docker-bind-mount-inode-drift"]);
    assert!(n.body.contains("[[docker-bind-mount-inode-drift]]"));
    assert_eq!(notes_on_disk(&fx.app).len(), 6);
}

#[test]
fn delimiter_split_names_items_by_first_line() {
    let fx = fixture();
    let text = "first entry\nmore\n---\nsecond entry\n---\nthird\n";
    write(&fx.src, "log.md", text);
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"log\"\npaths = [\"log.md\"]\nring = 3\nkind = \"session\"\nsplit = \"delimiter\"\ndelimiter = \"---\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (3, 3, 0, 0, 0, 0, 0, 0, 0), "{}", r.render());
    let names: Vec<&str> = r.records.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["log-first-entry", "log-second-entry", "log-third"]);
    let joined: String = r
        .records
        .iter()
        .map(|rec| fx.app.store().read(&rec.name).unwrap().body)
        .collect();
    assert_eq!(joined, text);
}

#[test]
fn file_mode_reuses_a_cyberbrain_head_and_keeps_a_foreign_one() {
    let fx = fixture();
    write(
        &fx.src,
        "own.md",
        "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: given-name\nring: 3\nkind: lesson\ncreated: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\ntags:\n- keep-me\nretention: P1Y\npii: none\n---\n\nthe body\n",
    );
    write(
        &fx.src,
        "foreign.md",
        "---\ntitle: Foo\ndate: 2026-01-01\n---\n\ntext\n",
    );
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"*.md\"]\nring = 2\nkind = \"knowledge\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (2, 2, 0, 0, 0, 0, 0, 0, 0), "{}", r.render());

    let own = fx.app.store().read("given-name").unwrap();
    assert_eq!(own.front.ring, Ring::Knowledge, "plan ring wins");
    assert_eq!(own.front.kind, cyberbrain_core::NoteKind::Lesson);
    assert!(own.front.tags.contains(&"keep-me".to_string()));
    assert_eq!(own.front.retention.as_deref(), Some("P1Y"));
    assert_eq!(own.body, "the body\n");
    assert_ne!(own.front.id.to_string(), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let rec = r.records.iter().find(|x| x.name == "given-name").unwrap();
    assert!(
        rec.notes.iter().any(|n| n.contains("not preserved")),
        "{:?}",
        rec.notes
    );
    assert!(
        rec.notes.iter().any(|n| n.contains("plan wins")),
        "{:?}",
        rec.notes
    );

    let foreign = fx.app.store().read("foreign").unwrap();
    assert!(
        foreign.body.starts_with("---\ntitle: Foo\n"),
        "{}",
        foreign.body
    );
    let rec = r.records.iter().find(|x| x.name == "foreign").unwrap();
    assert!(
        rec.notes.iter().any(|n| n.contains("kept in the body")),
        "{:?}",
        rec.notes
    );
}

#[test]
fn collisions_are_reported_not_suffixed_unless_asked() {
    let fx = fixture();
    write(
        &fx.src,
        "a.md",
        "## Same\none\n## Same\ntwo\n## Other\nthree\n",
    );
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"a.md\"]\nring = 2\nkind = \"knowledge\"\nsplit = \"heading\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (3, 1, 0, 0, 0, 0, 2, 0, 0), "{}", r.render());
    assert_eq!(r.exit_code(), 1);
    assert_eq!(
        notes_on_disk(&fx.app),
        [(Ring::Knowledge, "a-other".to_string())]
    );
    let text = r.render();
    assert!(text.contains("a-same <- a.md #1 line 1"), "{text}");
    assert!(text.contains("a-same <- a.md #2 line 3"), "{text}");
    assert!(text.contains("NOT CLEAN: 2"), "{text}");

    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"a.md\"]\nring = 2\nkind = \"knowledge\"\nsplit = \"heading\"\ncollisions = \"hash\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (3, 2, 1, 0, 0, 0, 0, 0, 0), "{}", r.render());
    let names: Vec<&str> = r.records.iter().map(|x| x.name.as_str()).collect();
    assert!(
        names[0].starts_with("a-same-") && names[1].starts_with("a-same-") && names[0] != names[1],
        "{names:?}"
    );
    assert!(
        r.records[0]
            .notes
            .iter()
            .any(|n| n.contains("content hash"))
    );
    // Identical bodies under one name are one note plus named duplicates, not a loss.
    write(&fx.src, "a.md", "## Same\nsame\n## Same\nsame\n");
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (2, 1, 0, 0, 0, 1, 0, 0, 0), "{}", r.render());
    assert!(matches!(&r.records[1].outcome, Outcome::Duplicate { of } if of.contains("#1 line 1")));
    assert_eq!(r.exit_code(), 0);
    assert!(
        r.render().contains("same bytes as a.md #1 line 1"),
        "{}",
        r.render()
    );
}

#[test]
fn unmapped_files_fail_the_run_and_skipped_ones_are_counted() {
    let fx = fixture();
    write(&fx.src, "a.md", "# A\n");
    write(&fx.src, "b.md", "# B\n");
    write(&fx.src, "state.json", "{}");
    write(&fx.src, "backups/old.md", "# old\n");
    let p = plan(
        &fx.src,
        "[[skip]]\npaths = [\"*.json\", \"backups/**\"]\nreason = \"state and backups\"\n[[group]]\nname = \"g\"\npaths = [\"a.md\"]\nring = 2\nkind = \"knowledge\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert!(r.is_balanced());
    assert_eq!(r.files.on_disk, 4);
    assert_eq!(r.files.mapped, 1);
    assert_eq!(r.files.unmapped, ["b.md"]);
    let skipped: Vec<&str> = r.files.skipped.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(skipped, ["backups/old.md", "state.json"]);
    assert_eq!(r.exit_code(), 1);
    let text = r.render();
    assert!(text.contains("UNMAPPED"), "{text}");
    assert!(text.contains("    b.md"), "{text}");
    assert!(text.contains("state and backups"), "{text}");
}

#[test]
fn reimport_recognises_items_and_never_clobbers_an_edit() {
    let fx = fixture();
    write(&fx.src, "m.md", "## S1\none\n## S2\ntwo\n");
    let keep = "[[group]]\nname = \"g\"\npaths = [\"m.md\"]\nring = 3\nkind = \"session\"\nsplit = \"heading\"\n";
    let p = plan(&fx.src, keep);
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (2, 2, 0, 0, 0, 0, 0, 0, 0));
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(
        counts(&r),
        (2, 0, 2, 0, 0, 0, 0, 0, 0),
        "second run: {}",
        r.render()
    );
    assert_eq!(r.exit_code(), 0);
    assert_eq!(notes_on_disk(&fx.app).len(), 2, "no duplicates");

    // The source changed: `keep` reports, `overwrite` replaces.
    write(&fx.src, "m.md", "## S1\none changed\n## S2\ntwo\n");
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (2, 0, 1, 0, 0, 0, 0, 1, 0), "{}", r.render());
    assert!(matches!(&r.records[0].outcome, Outcome::Exists { why } if why.contains("overwrite")));
    assert_eq!(fx.app.store().read("m-s1").unwrap().body, "## S1\none\n");
    let p_over = plan(&fx.src, &format!("{keep}existing = \"overwrite\"\n"));
    let r = import(&fx.app, &p_over, false).unwrap();
    assert_eq!(counts(&r), (2, 0, 1, 1, 0, 0, 0, 0, 0), "{}", r.render());
    assert_eq!(
        fx.app.store().read("m-s1").unwrap().body,
        "## S1\none changed\n"
    );

    // A note that was not imported from this file is never overwritten, even with `overwrite`.
    let mut n = fx.app.store().read("m-s2").unwrap();
    n.front.tags.retain(|t| !t.starts_with(SOURCE_TAG_PREFIX));
    n.body = "hand edited\n".into();
    fx.app.store().write(&n).unwrap();
    let r = import(&fx.app, &p_over, false).unwrap();
    assert!(
        matches!(&r.records[1].outcome, Outcome::Exists { why } if why.contains("no matching src: tag")),
        "{}",
        r.render()
    );
    assert_eq!(fx.app.store().read("m-s2").unwrap().body, "hand edited\n");

    // A section removed from the source leaves a stale note, reported and not erased.
    write(&fx.src, "m.md", "## S1\none changed\n");
    let r = import(&fx.app, &p_over, false).unwrap();
    assert_eq!(r.items.found, 1);
    assert!(
        r.stale.is_empty(),
        "m-s2 lost its src tag above, so it is not stale: {:?}",
        r.stale
    );
    write(&fx.src, "m.md", "## S1\none changed\n## S3\nthree\n");
    import(&fx.app, &p_over, false).unwrap();
    write(&fx.src, "m.md", "## S1\none changed\n");
    let r = import(&fx.app, &p_over, false).unwrap();
    assert_eq!(r.stale.len(), 1, "{}", r.render());
    assert_eq!(r.stale[0].name, "m-s3");
    assert!(
        fx.app.store().read("m-s3").is_ok(),
        "stale notes are not erased"
    );
}

#[test]
fn dry_run_reports_the_same_numbers_and_writes_nothing() {
    let fx = fixture();
    write(&fx.src, "a.md", "## One\nx\n## Two\ny\n## One\nz\n");
    write(&fx.src, "junk.txt", "");
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"a.md\"]\nring = 2\nkind = \"knowledge\"\nsplit = \"heading\"\n",
    );
    let dry = import(&fx.app, &p, true).unwrap();
    assert!(dry.dry_run);
    assert!(notes_on_disk(&fx.app).is_empty(), "dry run wrote a file");
    assert!(!dry.audit_preview.is_empty());
    assert_eq!(
        fx.app
            .policy()
            .audit()
            .read(&Default::default())
            .unwrap()
            .len(),
        1,
        "only store.init in the log after a dry run"
    );
    let real = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&dry), counts(&real));
    assert_eq!(dry.files.unmapped, real.files.unmapped);
    assert_eq!(dry.exit_code(), real.exit_code());
    // Every record, outcome and path included, is the same in both runs.
    assert_eq!(
        serde_json::to_value(&dry.records).unwrap(),
        serde_json::to_value(&real.records).unwrap()
    );
    assert_eq!(notes_on_disk(&fx.app).len(), 1);
}

#[test]
fn pii_is_held_unless_accepted_in_bulk() {
    let fx = fixture();
    write(&fx.src, "p.md", "contact bob@corp.example.org about it\n");
    write(&fx.src, "clean.md", "nothing personal here\n");
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"*.md\"]\nring = 2\nkind = \"knowledge\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (2, 1, 0, 0, 1, 0, 0, 0, 0), "{}", r.render());
    assert_eq!(r.exit_code(), 3);
    assert!(fx.app.store().read("p").is_err());
    assert!(
        matches!(&r.records[1].outcome, Outcome::Held { findings } if findings.contains("email"))
    );
    assert!(r.render().contains("--accept-pii"));

    let mut p = p;
    p.accept_pii = true;
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (2, 1, 1, 0, 0, 0, 0, 0, 0), "{}", r.render());
    let n = fx.app.store().read("p").unwrap();
    assert_eq!(n.front.pii, PiiState::Reviewed);
    assert!(
        n.body.contains("bob@corp.example.org"),
        "accepting means keeping the bytes"
    );
    assert_eq!(
        fx.app.store().read("clean").unwrap().front.pii,
        PiiState::None
    );
}

#[test]
fn resident_cap_is_projected_identically_in_dry_and_real_runs() {
    let fx = fixture();
    let cap = fx.app.store().resident_cap();
    write(&fx.src, "small.md", "# rule\nnever do the thing\n");
    write(
        &fx.src,
        "huge.md",
        &format!("# big\n{}", "word ".repeat(cap + 10)),
    );
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"inv\"\npaths = [\"*.md\"]\nring = 0\nkind = \"knowledge\"\n",
    );
    let dry = import(&fx.app, &p, true).unwrap();
    let real = import(&fx.app, &p, false).unwrap();
    for r in [&dry, &real] {
        assert_eq!(counts(r), (2, 1, 0, 0, 0, 0, 0, 0, 1), "{}", r.render());
        assert!(
            matches!(&r.records[0].outcome, Outcome::Failed { error } if error.contains("cap")),
            "{}",
            r.render()
        );
        assert_eq!(r.records[1].name, "small");
    }
    assert_eq!(
        notes_on_disk(&fx.app),
        [(Ring::Invariant, "small".to_string())]
    );
}

#[test]
fn an_unreadable_mapped_file_is_a_named_file_failure() {
    let fx = fixture();
    fs::write(fx.src.join("bad.md"), [0xff, 0xfe, b'x']).unwrap();
    write(&fx.src, "ok.md", "fine\n");
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"*.md\"]\nring = 2\nkind = \"knowledge\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert!(r.is_balanced());
    assert_eq!(r.file_failures.len(), 1);
    assert_eq!(r.file_failures[0].path, "bad.md");
    assert!(r.file_failures[0].reason.contains("UTF-8"));
    assert_eq!(r.exit_code(), 1);
    assert!(r.render().contains("FILE NOT IMPORTED bad.md"));
}

#[test]
fn a_heading_without_letters_falls_back_to_an_index_name_and_says_so() {
    let fx = fixture();
    write(&fx.src, "x.md", "## first\na\n## 🚀\nb\n");
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"x.md\"]\nring = 2\nkind = \"knowledge\"\nsplit = \"heading\"\n",
    );
    let r = import(&fx.app, &p, false).unwrap();
    assert_eq!(counts(&r), (2, 2, 0, 0, 0, 0, 0, 0, 0), "{}", r.render());
    assert_eq!(r.records[1].name, "x-2");
    assert!(
        r.records[1].notes.iter().any(|n| n.contains("gives none")),
        "{:?}",
        r.records[1].notes
    );
}

#[test]
fn ring_conflict_with_an_existing_note_is_reported_not_moved() {
    let fx = fixture();
    write(&fx.src, "n.md", "text\n");
    let p2 = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"n.md\"]\nring = 2\nkind = \"knowledge\"\n",
    );
    import(&fx.app, &p2, false).unwrap();
    let p3 = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"n.md\"]\nring = 3\nkind = \"knowledge\"\n",
    );
    let r = import(&fx.app, &p3, false).unwrap();
    assert!(
        matches!(&r.records[0].outcome, Outcome::Exists { why } if why.contains("ring r2")),
        "{}",
        r.render()
    );
    assert_eq!(notes_on_disk(&fx.app), [(Ring::Knowledge, "n".to_string())]);
}

#[test]
fn the_ledger_refuses_to_balance_over_a_lost_item() {
    // A report whose records do not add up to what was found is exit 2, whatever else
    // it says. This is the check that catches the importer losing track of itself.
    let fx = fixture();
    write(&fx.src, "a.md", "## A\nx\n## B\ny\n");
    let p = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"a.md\"]\nring = 2\nkind = \"knowledge\"\nsplit = \"heading\"\n",
    );
    let mut r = import(&fx.app, &p, true).unwrap();
    assert!(r.is_balanced());
    r.records.pop();
    assert!(!r.is_balanced());
    assert_eq!(r.exit_code(), 2);
    assert!(r.render().contains("ACCOUNTING FAILED"));
    let mut r2 = import(&fx.app, &p, true).unwrap();
    r2.items = ItemLedger {
        found: 3,
        ..r2.items
    };
    assert!(!r2.is_balanced());
    let mut r3 = import(&fx.app, &p, true).unwrap();
    r3.files.on_disk += 1;
    assert!(
        !r3.is_balanced(),
        "the independent count must be able to veto the listing"
    );
    let mut r4 = import(&fx.app, &p, true).unwrap();
    r4.files.byte_loss.push(super::report::ByteLoss {
        path: "a.md".into(),
        bytes_read: 10,
        bytes_in_items: 9,
    });
    assert!(
        !r4.is_balanced(),
        "a byte lost between file and items is an accounting failure"
    );
    assert!(r4.render().contains("the split lost content"));
    // A blank mapped file is named, not silently nothing.
    write(&fx.src, "blank.md", "\n\n");
    let p2 = plan(
        &fx.src,
        "[[group]]\nname = \"g\"\npaths = [\"*.md\"]\nring = 2\nkind = \"knowledge\"\nsplit = \"heading\"\n",
    );
    let r5 = import(&fx.app, &p2, true).unwrap();
    assert_eq!(r5.files.empty, ["blank.md"]);
    assert!(r5.is_balanced() && r5.exit_code() == 0, "{}", r5.render());
    assert!(
        r5.render()
            .contains("empty (blank file, no note made): blank.md")
    );
}

#[test]
fn the_default_plan_maps_a_tree_of_the_expected_shape_with_nothing_unmapped() {
    let fx = fixture();
    for (rel, text) in [
        ("identity.md", "# Identity\n- Never delete files\n"),
        ("OPENWOLF.md", "# protocol\n"),
        ("ENGINE.md", "# engine\n"),
        ("cerebrum.md", "# C\n## Key Learnings\n### one\nx\n"),
        ("DO-NOT-REPEAT.md", "# D\n### rules\n- a\n"),
        (
            "memory.md",
            "# Memory\n| t | a |\n## Session: 2026-04-16 15:53\nx\n",
        ),
        ("STATUS.md", "# S\n## Next\nx\n"),
        ("PREREG_va_2026-08-25.md", "# prereg\nhash\n"),
        ("PREREG_hashes.txt", "abc  PREREG_va_2026-08-25.md\n"),
        ("anatomy.md", "# anatomy\n## ./\nx\n"),
        ("reframe-frameworks.md", "# r\n"),
        ("cfetch-AGENT.draft.md", "# draft\n"),
        ("cfetch-umzug-checkliste.md", "# list\n"),
        ("proposals/bughunt-2026-07-14.md", "# p\n"),
        ("buglog.json", "{}"),
        ("STATUS.md.bak-20260903-2025", "old"),
        ("backups/cerebrum.md", "old"),
        ("hooks/shared.js", "code"),
        ("daemon.log", "log"),
        ("daemon.pid", "1"),
        (".gitignore", "x"),
        ("dashboard-token", "t"),
        ("designqc-captures/x.jpg", "img"),
    ] {
        write(&fx.src, rel, text);
    }
    let p = ImportPlan::default_for(&fx.src).unwrap();
    let r = import(&fx.app, &p, false).unwrap();
    assert!(r.is_balanced());
    assert!(r.files.unmapped.is_empty(), "{}", r.render());
    assert_eq!(r.files.mapped, 14, "{}", r.render());
    assert_eq!(r.files.skipped.len(), 9, "{}", r.render());
    assert_eq!(r.exit_code(), 0, "{}", r.render());
    let on_disk = notes_on_disk(&fx.app);
    assert!(on_disk.contains(&(Ring::Invariant, "identity".into())));
    assert!(on_disk.contains(&(Ring::Protocol, "openwolf".into())));
    assert!(on_disk.contains(&(Ring::Knowledge, "prereg-va-2026-08-25".into())));
    assert!(on_disk.contains(&(Ring::Knowledge, "prereg-hashes".into())));
    assert!(on_disk.contains(&(Ring::Session, "memory-session-2026-04-16-15-53".into())));
    assert!(on_disk.contains(&(Ring::External, "proposals-bughunt-2026-07-14".into())));
    // The evidence file's bytes are the note's bytes.
    assert_eq!(
        fx.app.store().read("prereg-hashes").unwrap().body,
        "abc  PREREG_va_2026-08-25.md\n"
    );
}
