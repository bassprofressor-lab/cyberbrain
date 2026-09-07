//! End-to-end tests over the real binary and a store it creates itself. Every test takes
//! the user's route (SPEC §14.9): the CLI, `--json`, and the files and databases on disk.
//!
//! The regression tests in here were each shown to fail against the defect they cover
//! before the implementation was finished (SPEC §14.2); the report says what was seen.

use cyberbrain_embed::synthetic::write_synthetic_model;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const VOCAB: &[&str] = &[
    "postgres",
    "pgdata",
    "moved",
    "docker",
    "bind",
    "mount",
    "inode",
    "drift",
    "redis",
    "maxmemory",
    "config",
    "copy",
    "never",
    "server",
    "two",
    "the",
    "a",
    "is",
    "in",
    "to",
    "unique",
    "marker",
    "alpha",
    "beta",
    "gamma",
];

struct Cb {
    _dir: tempfile::TempDir,
    store: PathBuf,
}

impl Cb {
    fn bin() -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cyberbrain"));
        c.env_remove("CYBERBRAIN_STORE");
        c
    }

    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        let out = Self::bin()
            .args(["init", "--path"])
            .arg(&store)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        Self { _dir: dir, store }
    }

    fn run(&self, args: &[&str]) -> Output {
        Self::bin()
            .arg("--store")
            .arg(&self.store)
            .args(args)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> (Value, i32, String) {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let out = self.run(&full);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let v = if stdout.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
        };
        (v, out.status.code().unwrap_or(-1), stderr)
    }

    /// Like [`json`](Self::json), with the note body on stdin instead of `--body`.
    ///
    /// Windows caps a command line at 32767 characters, so a body of any size has to
    /// arrive this way — which is what the CLI documents anyway. Passing a large body as
    /// an argument works on Linux, where the limit is megabytes, and fails on Windows with
    /// `os error 206`.
    fn json_stdin(&self, args: &[&str], body: &str) -> (Value, i32, String) {
        use std::io::Write;
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let mut child = Self::bin()
            .arg("--store")
            .arg(&self.store)
            .args(&full)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(body.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let v = if stdout.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("{e}: {stdout}"))
        };
        (v, out.status.code().unwrap_or(-1), stderr)
    }

    fn ok(&self, args: &[&str]) -> Value {
        let (v, code, err) = self.json(args);
        assert_eq!(code, 0, "{args:?}: {err}");
        v
    }

    fn write(&self, ring: &str, name: &str, body: &str) -> Value {
        self.ok(&[
            "write",
            "--ring",
            ring,
            "--kind",
            "knowledge",
            "--name",
            name,
            "--body",
            body,
        ])
    }

    fn note_path(&self, ring: &str, name: &str) -> PathBuf {
        self.store
            .join("notes")
            .join(format!("r{ring}"))
            .join(format!("{name}.md"))
    }

    fn install_model(&self, seed: u64) {
        let dir = self.store.join("models").join("model2vec");
        std::fs::create_dir_all(&dir).unwrap();
        let (_, manifest) = write_synthetic_model(&dir, VOCAB, 16, seed).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::to_string(&manifest).unwrap(),
        )
        .unwrap();
    }

    /// Every file under the store, by bytes. SQLite's `-wal`/`-shm` side files come and
    /// go with connections and are excluded; the databases themselves are compared.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    let name = p.file_name().unwrap().to_string_lossy();
                    if name.ends_with("-wal") || name.ends_with("-shm") {
                        continue;
                    }
                    out.insert(p.clone(), std::fs::read(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.store, &mut out);
        out
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.store.join("cyberbrain.db")).unwrap()
    }

    fn count(&self, sql: &str) -> i64 {
        self.db().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn audit_rows(&self) -> Vec<Value> {
        let v = self.ok(&["policy", "audit", "--limit", "1000"]);
        serde_json::from_str(v["rendered"].as_str().unwrap()).unwrap()
    }
}

// ---------------------------------------------------------------------------------------

#[test]
fn init_creates_a_store_once_and_commands_outside_one_say_so() {
    let cb = Cb::new();
    assert!(cb.store.join("cyberbrain.toml").is_file());
    assert!(cb.store.join("audit.db").is_file());
    assert!(cb.store.join("cyberbrain.db").is_file());
    for r in ["r0", "r1", "r2", "r3", "r4"] {
        assert!(cb.store.join("notes").join(r).is_dir());
    }
    let (_, code, err) = cb.json(&["init", "--path", cb.store.to_str().unwrap()]);
    assert_eq!(code, 1);
    assert!(err.contains("already a cyberbrain store"), "{err}");

    // Outside a store: say so, suggest init, create nothing.
    let empty = tempfile::tempdir().unwrap();
    let out = Cb::bin()
        .current_dir(empty.path())
        .args(["status"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cyberbrain init"), "{err}");
    assert!(
        std::fs::read_dir(empty.path()).unwrap().next().is_none(),
        "a command outside a store must not create one"
    );

    // Discovery walks up from a subdirectory.
    let project = tempfile::tempdir().unwrap();
    let store = project.path().join(".cyberbrain");
    let out = Cb::bin()
        .args(["init", "--path"])
        .arg(&store)
        .output()
        .unwrap();
    assert!(out.status.success());
    let deep = project.path().join("src").join("deep");
    std::fs::create_dir_all(&deep).unwrap();
    let out = Cb::bin()
        .current_dir(&deep)
        .args(["--json", "status"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    // Both sides canonicalised. The tool resolves the store path, which is right — it is
    // what makes a store's identity stable — and on macOS `/var` is a symlink to
    // `/private/var`, so a temporary directory comes back under a different prefix than
    // the one `tempfile` handed out. Comparing the raw strings passes on Linux and fails
    // on macOS for a difference that is not a difference.
    assert_eq!(
        std::fs::canonicalize(v["store"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&store).unwrap()
    );
}

#[test]
fn write_indexes_immediately_and_scan_counts_name_their_side_of_the_boundary() {
    let cb = Cb::new();
    let w = cb.write(
        "2",
        "pg18-moves-pgdata",
        "Postgres 18 moved PGDATA. See [[docker-bind-mount-inode-drift]].",
    );
    assert_eq!(w["outcome"], "written");
    assert_eq!(w["blocks"], 1);
    assert_eq!(w["links"], 1);
    assert_eq!(w["created"], true);
    cb.write("0", "never-copy-config", "Never copy config to server two.");

    let s = cb.ok(&["scan"]);
    assert_eq!(s["unchanged"], 2, "write already indexed both: {s}");
    assert_eq!(s["indexed_new"], 0);
    assert_eq!(s["index"]["dangling_links"], 1);

    // Hand-edit one note (the human's route), drop a broken file in, add one by hand.
    let p = cb.note_path("2", "pg18-moves-pgdata");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(
        &p,
        text.replace("moved PGDATA", "moved PGDATA into a subdirectory"),
    )
    .unwrap();
    std::fs::write(
        cb.note_path("3", "broken"),
        "---\nname: broken\n---\nno ring\n",
    )
    .unwrap();
    let good = cb.note_path("3", "by-hand");
    std::fs::write(
        &good,
        "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: by-hand\nring: 3\nkind: session\ncreated: 2026-09-01T00:00:00Z\nupdated: 2026-09-01T00:00:00Z\n---\n\nWritten by hand, links to [[pg18-moves-pgdata]].\n",
    )
    .unwrap();

    let out = cb.run(&["scan"]);
    let human = String::from_utf8_lossy(&out.stdout);
    let s = cb.ok(&["scan"]); // second scan: everything now unchanged, the broken one still skipped
    assert_eq!(s["unchanged"], 3, "{s}");
    assert_eq!(s["skipped"].as_array().unwrap().len(), 1);
    assert!(
        s["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("frontmatter"),
        "{s}"
    );
    assert!(
        human.contains("1 note reindexed (content changed)"),
        "{human}"
    );
    assert!(
        human.contains("1 note indexed for the first time"),
        "{human}"
    );
    assert!(human.contains("1 skipped"), "{human}");
    assert!(human.contains("unreadable frontmatter"), "{human}");
    // The hand-written note carried no links: field; scan wrote it back.
    assert!(
        human.contains("links written back into frontmatter: 1"),
        "{human}"
    );
    let text = std::fs::read_to_string(&good).unwrap();
    assert!(text.contains("links:\n- pg18-moves-pgdata"), "{text}");
    assert_eq!(
        s["index"]["dangling_links"], 1,
        "by-hand's link resolves; pg18's still dangles"
    );

    // Remove a file by hand: the next scan drops the row and records why.
    std::fs::remove_file(&good).unwrap();
    let s = cb.ok(&["scan"]);
    assert_eq!(s["dropped_missing_file"], serde_json::json!(["by-hand"]));
    let rows = cb.audit_rows();
    let dropped = rows
        .iter()
        .find(|r| r["action"] == "index.note-dropped")
        .expect("a dropped row is recorded");
    assert_eq!(dropped["detail"]["name"], "by-hand");
    assert_eq!(dropped["detail"]["reason"], "file missing at scan");
}

#[test]
fn recall_hits_carry_citations_that_resolve_and_caveats_say_what_was_skipped() {
    let cb = Cb::new();
    cb.write(
        "2",
        "redis-limits",
        "Redis maxmemory must be set or the box swaps.",
    );
    cb.write("3", "unrelated", "Alpha beta gamma.");
    let r = cb.ok(&["recall", "redis maxmemory"]);
    let hits = r["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{r}");
    let cit = hits[0]["citation"].as_str().unwrap();
    assert!(cit.starts_with("r2-") && cit.len() == 15, "{cit}");
    let caveats = r["caveats"].as_array().unwrap();
    assert!(
        caveats
            .iter()
            .any(|c| c.as_str().unwrap().contains("lexical only")),
        "{r}"
    );
    assert!(
        caveats
            .iter()
            .any(|c| c.as_str().unwrap().contains("contradiction check skipped")),
        "{r}"
    );

    let e = cb.ok(&["recall", "--id", cit]);
    assert_eq!(e["note"]["front"]["name"], "redis-limits");
    assert_eq!(e["block"]["citation"], cit);
    assert!(e["note"]["body"].as_str().unwrap().contains("maxmemory"));

    let (_, code, err) = cb.json(&["recall", "--id", "not-a-citation"]);
    assert_eq!(code, 1, "a bad citation is a user error: {err}");
    assert!(err.contains("bad-citation"), "{err}");
    let (_, code, err) = cb.json(&["recall", "--id", "r9-a91f2c33e1bd"]);
    assert_eq!(code, 1, "core reports the ring before the shape: {err}");
    assert!(err.contains("bad-ring"), "{err}");

    let r = cb.ok(&["recall", "alpha", "--ring", "2"]);
    assert!(r["hits"].as_array().unwrap().is_empty(), "ring filter: {r}");
}

#[test]
fn a_model_artefact_gives_vectors_and_semantic_search_with_a_verified_profile() {
    let cb = Cb::new();
    cb.write("2", "pg", "postgres pgdata moved");
    cb.write("2", "dock", "docker bind mount inode drift");
    let before = cb.ok(&["status"]);
    assert_eq!(before["index"]["vectors"], 0);
    assert_eq!(before["embedding"]["embedder"]["loaded"], false);

    cb.install_model(1);
    let s = cb.ok(&["scan"]);
    assert_eq!(s["embedder"]["loaded"], true, "{s}");
    assert_eq!(
        s["revectorised"], 2,
        "content unchanged, vectors were missing: {s}"
    );
    assert_eq!(s["index"]["vectors"], s["index"]["blocks"]);
    assert_eq!(s["profile_change"]["changed"], true);
    assert_eq!(s["profile_change"]["previous"], Value::Null);

    let r = cb.ok(&["recall", "postgres"]);
    let caveats = r["caveats"].to_string();
    assert!(!caveats.contains("lexical only"), "{caveats}");
    assert_eq!(r["hits"][0]["note_name"], "pg");

    let st = cb.ok(&["status"]);
    assert_eq!(st["embedding"]["matches_index"], true);
    assert_eq!(st["embedding"]["manifest_present"], true);
    let d = cb.ok(&["doctor"]);
    assert_eq!(d["clean"], true, "{d}");

    // A write with a model present embeds in the same request.
    let w = cb.write("2", "third", "redis maxmemory");
    assert_eq!(w["vectors"], 1, "{w}");

    // A tampered weights file is refused, never warned about.
    let weights = cb.store.join("models/model2vec/model.safetensors");
    let mut bytes = std::fs::read(&weights).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&weights, bytes).unwrap();
    let s = cb.ok(&["scan"]);
    assert_eq!(s["embedder"]["loaded"], false);
    assert!(
        s["embedder"]["reason"]
            .as_str()
            .unwrap()
            .contains("does not match its manifest"),
        "{s}"
    );
}

/// Regression, SPEC §8 / §8.2: `--dry-run` swaps the writers, not the path. Shown to fail
/// against a `writers()` that ignored `dry_run` (the store changed and the audit rows were
/// real) before the no-op writers were wired in.
#[test]
fn dry_run_leaves_the_store_byte_identical_and_still_runs_the_real_path() {
    let cb = Cb::new();
    cb.install_model(1);
    cb.write("2", "keep", "postgres pgdata is unique");
    cb.write("2", "gone-soon", "docker inode drift");
    cb.ok(&["scan"]);
    // Real work for a scan to find: a hand edit and a hand deletion.
    let p = cb.note_path("2", "keep");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.replace("is unique", "is unique marker")).unwrap();
    std::fs::remove_file(cb.note_path("2", "gone-soon")).unwrap();
    let audit_before = cb.count_audit();
    let before = cb.snapshot();

    let s = cb.ok(&["scan", "--dry-run"]);
    assert_eq!(s["dry_run"], true);
    assert_eq!(s["reindexed_changed"], 1, "the real comparison ran: {s}");
    assert_eq!(s["dropped_missing_file"], serde_json::json!(["gone-soon"]));
    assert_eq!(
        s["audit_preview"],
        serde_json::json!(["index.note-dropped"])
    );
    assert_eq!(before, cb.snapshot(), "dry-run scan changed the store");

    let f = cb.ok(&["forget", "keep", "--dry-run"]);
    assert_eq!(f["dry_run"], true);
    assert_eq!(f["blocks"], 1, "the real erasure logic counted: {f}");
    assert_eq!(f["vectors"], 1);
    assert_eq!(f["fts_rows"], 1);
    assert_eq!(f["file_removed"], true, "it *would* have removed the file");
    assert!(p.is_file(), "dry-run forget removed the file");
    assert_eq!(before, cb.snapshot(), "dry-run forget changed the store");

    let w = cb.ok(&[
        "write",
        "--ring",
        "2",
        "--kind",
        "lesson",
        "--name",
        "new-one",
        "--body",
        "beta gamma",
        "--dry-run",
    ]);
    assert_eq!(w["outcome"], "written");
    assert_eq!(w["dry_run"], true);
    assert_eq!(w["vectors"], 1, "the embedder ran on the real path: {w}");
    assert_eq!(w["audit_preview"], serde_json::json!(["note.write"]));
    assert!(!cb.note_path("2", "new-one").exists());
    assert_eq!(before, cb.snapshot(), "dry-run write changed the store");

    assert_eq!(
        cb.count_audit(),
        audit_before,
        "dry runs must not append to audit.db"
    );
    // And the real thing still works afterwards.
    let s = cb.ok(&["scan"]);
    assert_eq!(s["reindexed_changed"], 1);
    assert_ne!(before, cb.snapshot());
}

impl Cb {
    fn count_audit(&self) -> i64 {
        rusqlite::Connection::open(self.store.join("audit.db"))
            .unwrap()
            .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
            .unwrap()
    }
}

/// Regression, SPEC §12.2: erasure removes the vector and the FTS row, not just the file
/// and the notes row, and the erasure is recorded with what went. Shown to fail against an
/// eraser that removed the file and skipped `delete_note` (vector and FTS row survived),
/// and against a `forget` that bypassed `Policy::forget` (no erasure row).
#[test]
fn forget_erases_vector_and_fts_row_and_the_audit_log_names_what_went() {
    let cb = Cb::new();
    cb.install_model(1);
    cb.write("2", "doomed", "postgres pgdata unique marker");
    cb.write("2", "linker", "see [[doomed]] and [[nowhere]]");
    cb.ok(&["scan"]);
    let id = cb.ok(&["export", "doomed"])["front"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(cb.count("SELECT count(*) FROM vectors"), 2);
    assert_eq!(
        cb.count("SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH 'marker'"),
        1
    );
    assert_eq!(
        cb.count(
            "SELECT count(*) FROM links WHERE to_name = 'doomed' AND resolved_note_id IS NOT NULL"
        ),
        1
    );

    let f = cb.ok(&["forget", "doomed"]);
    assert_eq!(f["dry_run"], false);
    assert_eq!(f["file_removed"], true);
    assert_eq!(f["blocks"], 1);
    assert_eq!(f["fts_rows"], 1);
    assert_eq!(f["vectors"], 1);
    assert_eq!(f["links_in_unresolved"], 1);
    assert!(!cb.note_path("2", "doomed").exists());

    // The spec's named silent failure: file gone, vector still in the index.
    assert_eq!(
        cb.count("SELECT count(*) FROM vectors"),
        1,
        "the vector must be gone"
    );
    assert_eq!(
        cb.count("SELECT count(*) FROM blocks_fts WHERE blocks_fts MATCH 'marker'"),
        0,
        "the FTS row must be gone"
    );
    assert_eq!(
        cb.count(&format!("SELECT count(*) FROM notes WHERE id = '{id}'")),
        0
    );
    assert_eq!(
        cb.count(&format!(
            "SELECT count(*) FROM blocks WHERE note_id = '{id}'"
        )),
        0
    );
    // Inbound links are unresolved, not deleted.
    assert_eq!(
        cb.count(
            "SELECT count(*) FROM links WHERE to_name = 'doomed' AND resolved_note_id IS NULL"
        ),
        1
    );
    let r = cb.ok(&["recall", "marker"]);
    assert!(
        r["hits"].as_array().unwrap().is_empty(),
        "lexical search still finds the erased note: {r}"
    );

    // The erasure record is the evidence (SPEC §12.2) and it names what went.
    let rows = cb.audit_rows();
    let completed = rows
        .iter()
        .find(|r| r["action"] == "note.erase.completed")
        .expect("an erasure row exists");
    assert_eq!(completed["subject"], format!("note:{id}"));
    assert_eq!(completed["detail"]["name"], "doomed");
    assert_eq!(completed["detail"]["vectors"], 1);
    assert_eq!(completed["detail"]["fts_rows"], 1);
    assert_eq!(completed["detail"]["file_removed"], true);
    assert!(rows.iter().any(|r| r["action"] == "note.erase.requested"));
    let v = cb.ok(&["policy", "audit", "--verify"]);
    assert!(v["verified"]["Ok"].is_number(), "{v}");

    let (_, code, err) = cb.json(&["forget", "doomed"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no-such-note"), "{err}");
}

/// Regression: the index wipes vectors from a previous profile itself and writes no audit
/// row; the binary must. Shown to fail with the `record_raw` in `declare_profile` removed
/// (the wipe happened, the log said nothing).
#[test]
fn a_profile_change_that_wipes_vectors_is_recorded_with_the_count() {
    let cb = Cb::new();
    cb.install_model(1);
    cb.write("2", "a", "postgres pgdata");
    cb.write("2", "b", "docker inode");
    let s = cb.ok(&["scan"]);
    assert_eq!(s["index"]["vectors"], 2);
    let old_profile = s["embedder"]["profile_id"].as_str().unwrap().to_string();

    cb.install_model(2);
    let s = cb.ok(&["scan"]);
    let new_profile = s["embedder"]["profile_id"].as_str().unwrap().to_string();
    assert_ne!(
        old_profile, new_profile,
        "different weights, different profile"
    );
    assert_eq!(s["profile_change"]["changed"], true, "{s}");
    assert_eq!(s["profile_change"]["vectors_wiped"], 2);
    assert_eq!(s["profile_change"]["previous"]["id"], old_profile);
    assert_eq!(
        s["revectorised"], 2,
        "every note re-embedded under the new profile"
    );
    assert_eq!(s["index"]["vectors"], 2);

    let rows = cb.audit_rows();
    let row = rows
        .iter()
        .find(|r| {
            r["action"] == "index.embedding-profile-changed" && r["detail"]["vectors_wiped"] == 2
        })
        .unwrap_or_else(|| panic!("no row records the wipe: {rows:?}"));
    assert_eq!(row["detail"]["previous"]["id"], old_profile);
    assert_eq!(row["detail"]["current"]["id"], new_profile);
    assert_eq!(
        cb.ok(&["policy", "audit", "--verify"])["verified"]["Ok"],
        rows.len() + 1
    );
}

#[test]
fn a_pii_hold_is_a_decision_with_exit_3_and_force_writes_it_flagged() {
    let cb = Cb::new();
    let (v, code, _) = cb.json(&[
        "write",
        "--ring",
        "3",
        "--kind",
        "session",
        "--name",
        "contact",
        "--body",
        "reach bob@corp.example.org",
    ]);
    assert_eq!(code, 3);
    assert_eq!(v["outcome"], "held");
    assert_eq!(v["findings"][0]["kind"], "email");
    assert!(
        !cb.note_path("3", "contact").exists(),
        "a held write writes nothing"
    );
    let rows = cb.audit_rows();
    let held = rows
        .iter()
        .find(|r| r["action"] == "note.write.held")
        .unwrap();
    assert!(
        !held.to_string().contains("bob@"),
        "the matched text never enters the log"
    );

    let w = cb.ok(&[
        "write",
        "--ring",
        "3",
        "--kind",
        "session",
        "--name",
        "contact",
        "--body",
        "reach bob@corp.example.org",
        "--force",
    ]);
    assert_eq!(w["pii"], "flagged");
    let text = std::fs::read_to_string(cb.note_path("3", "contact")).unwrap();
    assert!(text.contains("pii: flagged"), "{text}");

    // Ring cap and bad names are user errors (1); a policy refusal (3) is not a crash.
    let big = "word ".repeat(9000);
    let (_, code, err) = cb.json_stdin(
        &[
            "write", "--ring", "0", "--kind", "decision", "--name", "huge",
        ],
        &big,
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("ring-cap-exceeded"), "{err}");
    let (_, code, err) = cb.json(&[
        "write",
        "--ring",
        "2",
        "--kind",
        "knowledge",
        "--name",
        "Bad Name",
        "--body",
        "x",
    ]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("kebab-case"), "{err}");
}

#[test]
fn policy_subcommands_work_over_a_real_store() {
    let cb = Cb::new();
    cb.write("2", "team", "Bob Smith owns the pager.");
    cb.ok(&[
        "write",
        "--ring",
        "3",
        "--kind",
        "session",
        "--name",
        "old-session",
        "--body",
        "alpha beta",
        "--retention",
        "P1D",
    ]);
    cb.write("3", "fresh-session", "gamma");

    // Egress register: the three registered purposes, each enabled or disabled with a
    // reason. An unenrolled store lists audit sync as disabled rather than hiding it.
    let e = cb.ok(&["policy", "egress"]);
    let entries = e.as_array().unwrap();
    assert_eq!(entries.len(), 3);
    let sync = &entries[2];
    assert_eq!(sync["purpose"], "audit-sync");
    assert_eq!(sync["enabled"], false);
    assert!(
        sync["state"]
            .as_str()
            .unwrap()
            .contains("not enrolled with a hub"),
        "{sync}"
    );
    assert_eq!(entries[0]["purpose"], "model-download");
    assert_eq!(entries[0]["enabled"], false);
    assert!(
        entries[0]["state"]
            .as_str()
            .unwrap()
            .contains("no model_source")
    );
    assert_eq!(entries[1]["purpose"], "local-inference");
    assert_eq!(entries[1]["enabled"], true);

    // Subject access: finds the identifier with a citation, logs a hash not the name.
    let s = cb.ok(&["policy", "subject", "Bob Smith"]);
    assert_eq!(s["note_hits"].as_array().unwrap().len(), 1);
    assert_eq!(s["note_hits"][0]["note_name"], "team");
    assert!(
        s["note_hits"][0]["citation"]
            .as_str()
            .unwrap()
            .starts_with("r2-")
    );
    let rows = cb.audit_rows();
    let access = rows
        .iter()
        .find(|r| r["action"] == "subject.access")
        .unwrap();
    assert!(
        access["subject"]
            .as_str()
            .unwrap()
            .starts_with("id:blake3:")
    );
    assert!(!access.to_string().to_lowercase().contains("bob smith"));
    let (_, code, err) = cb.json(&["policy", "subject", "ab"]);
    assert_eq!(code, 1, "{err}");

    // Retention: back-date the note by hand (the human's route), rescan, sweep.
    let p = cb.note_path("3", "old-session");
    let text = std::fs::read_to_string(&p).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("created:"))
        .unwrap()
        .to_string();
    std::fs::write(&p, text.replace(&line, "created: 2020-01-01T00:00:00Z")).unwrap();
    cb.ok(&["scan"]);
    let r = cb.ok(&["policy", "retention"]);
    assert_eq!(r["queue"]["due"], 1, "{r}");
    assert_eq!(r["queue"]["indefinite"], 2);
    assert_eq!(r["applied_run"], false);
    assert!(p.is_file(), "listing never erases");
    let r = cb.ok(&["policy", "retention", "--apply", "--dry-run"]);
    assert_eq!(r["applied"].as_array().unwrap().len(), 1);
    assert_eq!(r["applied"][0]["result"]["Ok"]["dry_run"], true);
    assert!(p.is_file(), "a dry-run sweep erases nothing");
    assert_eq!(
        r["audit_preview"],
        serde_json::json!([
            "retention.expired",
            "note.erase.requested",
            "note.erase.completed"
        ])
    );
    let r = cb.ok(&["policy", "retention", "--apply"]);
    assert_eq!(r["applied"][0]["result"]["Ok"]["file_removed"], true);
    assert!(!p.exists());
    let rows = cb.audit_rows();
    let expired = rows
        .iter()
        .find(|r| r["action"] == "retention.expired")
        .unwrap();
    assert_eq!(expired["actor"], "retention");
    assert_eq!(expired["detail"]["name"], "old-session");
    assert!(
        rows.iter()
            .any(|r| r["action"] == "note.erase.completed" && r["detail"]["reason"] == "retention")
    );
    assert_eq!(cb.ok(&["status"])["index"]["notes"], 2);

    // Model card: nothing in use, and it says so rather than inventing a model.
    let m = cb.ok(&["policy", "model-card"]);
    assert!(m["cards"].as_array().unwrap().is_empty());
    assert_eq!(m["absent"].as_array().unwrap().len(), 2);
    cb.install_model(1);
    let m = cb.ok(&["policy", "model-card"]);
    assert_eq!(m["cards"][0]["role"], "embedding");
    assert_eq!(m["cards"][0]["hash_verified"], true);
    assert_eq!(m["cards"][0]["dimension"], 16);
    assert_eq!(
        m["cards"][0]["licence"],
        Value::Null,
        "not stated, never guessed"
    );

    // Consent lives in the file, survives comments, and is recorded.
    let c = cb.ok(&["policy", "consent"]);
    assert_eq!(c["consent"], true);
    let toml = std::fs::read_to_string(cb.store.join("cyberbrain.toml")).unwrap();
    assert!(toml.contains("\nmodel_download_consent = true\n"), "{toml}");
    assert!(
        toml.contains("# Cyberbrain configuration"),
        "comments survive"
    );
    assert_eq!(toml.matches("model_download_consent =").count(), 1);
    let c = cb.ok(&["policy", "consent", "--withdraw"]);
    assert_eq!(c["consent"], false);
    let toml = std::fs::read_to_string(cb.store.join("cyberbrain.toml")).unwrap();
    assert!(toml.contains("\nmodel_download_consent = false\n"));
    let e = cb.ok(&["policy", "egress"]);
    assert!(e[0]["state"].as_str().unwrap().contains("no model_source"));

    // Audit: the filter works, the export is itself recorded, and the chain holds.
    let a = cb.ok(&["policy", "audit", "--action", "note.write", "--limit", "2"]);
    assert_eq!(a["rows"], 2);
    let v = cb.ok(&["policy", "audit", "--verify"]);
    let n = v["verified"]["Ok"].as_u64().unwrap();
    assert!(n >= 13, "{v}");
    assert!(
        cb.audit_rows()
            .iter()
            .filter(|r| r["action"] == "audit.export")
            .count()
            >= 3
    );
}

#[test]
fn doctor_reports_dangling_links_stale_index_and_broken_files() {
    let cb = Cb::new();
    cb.write("2", "a", "see [[missing]]");
    let d = cb.ok(&["doctor"]);
    assert_eq!(d["clean"], false);
    let f = d["findings"].as_array().unwrap();
    assert_eq!(f.len(), 1, "{d}");
    assert_eq!(f[0]["check"], "dangling links");
    assert!(f[0]["detail"].as_str().unwrap().contains("[[missing]]"));

    let p = cb.note_path("2", "a");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.replace("[[missing]]", "nothing")).unwrap();
    std::fs::write(cb.note_path("2", "junk"), "not a note").unwrap();
    let d = cb.ok(&["doctor"]);
    let checks: Vec<&str> = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["check"].as_str().unwrap())
        .collect();
    assert!(checks.contains(&"stale index"), "{d}");
    assert!(checks.contains(&"unreadable note"), "{d}");
    assert!(d["checks_run"].as_array().unwrap().len() >= 8);
    let st = cb.ok(&["status"]);
    assert_eq!(st["index_stale"], true);

    std::fs::remove_file(cb.note_path("2", "junk")).unwrap();
    cb.ok(&["scan"]);
    let d = cb.ok(&["doctor"]);
    assert_eq!(d["clean"], true, "{d}");
    let out = cb.run(&["doctor"]);
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("clean:"));
}

#[test]
fn export_gives_the_file_back_or_a_json_view() {
    let cb = Cb::new();
    let w = cb.write("4", "imported", "External claim.");
    let id = w["id"].as_str().unwrap();
    let out = cb.run(&["export", "imported"]);
    let md = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        md.trim_end(),
        std::fs::read_to_string(cb.note_path("4", "imported"))
            .unwrap()
            .trim_end()
    );
    let out = cb.run(&["export", id, "--format", "json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["front"]["name"], "imported");
    assert_eq!(v["kind"], "knowledge");
    assert_eq!(v["blocks"].as_array().unwrap().len(), 1);
    let (_, code, err) = cb.json(&["export", "nope"]);
    assert_eq!(code, 1, "{err}");

    // --quiet prints nothing and still exits by the outcome.
    let out = cb.run(&["--quiet", "status"]);
    assert!(out.stdout.is_empty() && out.status.success());
    // A hook never fails the harness, whatever it is handed (SPEC §9.1). `serve` is not
    // exercised here: it binds and blocks by design, and it has its own tests.
    let out = cb.run(&["hook", "pre-tool-use"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

/// An `--action` filter that matches nothing must be refused, not answered with an empty
/// table. In an audit tool those are opposite statements: "there is no such action name"
/// and "nothing of that kind ever happened". Someone checking whether erasures occurred
/// types a plausible name, gets zero rows, and concludes the wrong thing.
///
/// Written against the broken state first: with the check removed, `gibtsnicht` returned
/// exit 0 and `0 rows shown`, and `note.erase` — a real family — returned nothing at all
/// because the store compares action names exactly.
#[test]
fn an_audit_filter_that_matches_nothing_says_so_and_families_are_served() {
    let cb = Cb::new();
    cb.run(&[
        "write",
        "--ring",
        "2",
        "--kind",
        "bug",
        "--name",
        "a-note",
        "--body",
        "some text",
    ]);
    cb.run(&["forget", "a-note"]);

    // A name that does not exist anywhere in the log: refused, exit 1, and the message
    // names what is actually there so the reader can correct themselves.
    let out = cb.run(&["policy", "audit", "--action", "no-such-action"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no audit action named"), "{err}");
    assert!(
        err.contains("note.erase.completed"),
        "must list what is present: {err}"
    );

    // A family prefix serves the whole family, because validation accepts it and
    // accepting a name without serving it is the empty-table failure again.
    let family = String::from_utf8_lossy(
        &cb.run(&["policy", "audit", "--action", "note.erase"])
            .stdout,
    )
    .to_string();
    assert!(family.contains("note.erase.requested"), "{family}");
    assert!(family.contains("note.erase.completed"), "{family}");

    // An exact name still selects exactly that one.
    let exact = String::from_utf8_lossy(
        &cb.run(&["policy", "audit", "--action", "note.erase.completed"])
            .stdout,
    )
    .to_string();
    assert!(exact.contains("note.erase.completed"), "{exact}");
    assert!(!exact.contains("note.erase.requested"), "{exact}");
}

/// A link to a name that could exist and a link to a name that never can are different
/// facts, and reporting both as "does not exist" tells the operator to wait for something
/// that is not coming. Written against the broken state first: with both cases sharing one
/// message, the second assertion below matched the first finding and passed vacuously.
#[test]
fn doctor_separates_links_that_can_never_resolve_from_ones_that_merely_do_not_exist() {
    let cb = Cb::new();
    // A valid target nobody has written yet, and a target from another tool's file naming.
    cb.run(&[
        "write",
        "--ring",
        "2",
        "--kind",
        "knowledge",
        "--name",
        "source-note",
        "--body",
        "See [[not-written-yet]] and [[lesson_messwerkzeug_eichen]].",
    ]);
    // The note the underscore link probably meant, under a name that is actually legal.
    cb.run(&[
        "write",
        "--ring",
        "2",
        "--kind",
        "lesson",
        "--name",
        "lesson-messwerkzeug-eichen",
        "--body",
        "calibrate the instrument first",
    ]);

    let out = String::from_utf8_lossy(&cb.run(&["doctor"]).stdout).to_string();

    assert!(
        out.contains("[[not-written-yet]] which does not exist yet"),
        "a legal name is intent, not an error: {out}"
    );
    assert!(
        out.contains("[[lesson_messwerkzeug_eichen]], which can never resolve"),
        "an illegal name must be reported as unresolvable: {out}"
    );
    assert!(
        out.contains("did you mean [[lesson-messwerkzeug-eichen]]?"),
        "when the intended note exists under a legal name, say so: {out}"
    );
    assert!(
        out.contains("unresolvable links"),
        "the two cases must be separate checks so their counts do not merge: {out}"
    );
}

/// A hook is told which project the session is in. That statement wins over the directory
/// the process happens to start in, and the two are not the same thing: the harness starts
/// the hook in the project directory today, so a hook that used the process directory
/// would look correct right up until that changed and it silently read another project's
/// memory.
///
/// Written against the broken state first: with discovery walking up from the process
/// directory, this resolved the *other* store and the assertion below saw its path.
#[test]
fn a_hook_resolves_the_store_the_session_names_not_the_one_it_was_started_in() {
    use std::io::Write;

    // Discovery looks for a directory literally named `.cyberbrain`, so both stores are
    // created under that name rather than through `Cb::new`, which puts its store at
    // `<tmp>/store` where the walk would never see it.
    let init = |path: &std::path::Path| {
        assert!(
            Cb::bin()
                .args(["init", "--path"])
                .arg(path)
                .output()
                .unwrap()
                .status
                .success()
        );
    };

    let session_dir = tempfile::tempdir().unwrap();
    let session_store = session_dir.path().join(".cyberbrain");
    init(&session_store);
    assert!(
        Cb::bin()
            .arg("--store")
            .arg(&session_store)
            .args([
                "write",
                "--ring",
                "0",
                "--kind",
                "decision",
                "--name",
                "the-right-store",
                "--body",
                "this note only exists in the session's store",
            ])
            .output()
            .unwrap()
            .status
            .success()
    );

    // A second store, and the hook is started from inside it.
    let other_dir = tempfile::tempdir().unwrap();
    let other_store = other_dir.path().join(".cyberbrain");
    init(&other_store);

    let payload = serde_json::json!({
        "session_id": "s",
        "source": "startup",
        "cwd": session_dir.path(),
    })
    .to_string();

    let mut child = Cb::bin()
        .current_dir(other_dir.path())
        .args(["hook", "session-start"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(out.status.code(), Some(0), "a hook never fails the harness");
    assert!(
        stdout.contains("the-right-store"),
        "the hook must read the store the session named: {stdout}"
    );
    assert!(
        !stdout.contains(&cyberbrain_core::slash(&other_store)),
        "it must not fall back to the directory it was started in: {stdout}"
    );
}

// ---------------------------------------------------------------------------------------
// Audit export (SPEC §12.6): the file a person hands to somebody else.

/// The claim the format makes is that the file stands on its own. This is the end-to-end
/// version of that: write rows, export a bundle, and check it with a *second* process that
/// is given the file and nothing else — no store, no configuration, no working directory
/// that means anything.
#[test]
fn an_exported_bundle_verifies_with_nothing_but_the_file() {
    let cb = Cb::new();
    for i in 0..3 {
        let out = cb.run(&[
            "write",
            "--ring",
            "2",
            "--kind",
            "knowledge",
            "--name",
            &format!("note-{i}"),
            "--body",
            "Body of the note.",
        ]);
        assert!(out.status.success());
    }

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("export.jsonl");
    let out = cb.run(&["policy", "audit", "--export", file.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(file.is_file());

    // A different process, a different directory, no --store.
    let check = Cb::bin()
        .current_dir(dir.path())
        .arg("verify-export")
        .arg(&file)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert!(check.status.success(), "{stdout}");
    assert!(stdout.contains("chain holds over"), "{stdout}");
}

#[test]
fn a_tampered_bundle_is_refused_with_a_non_zero_exit() {
    let cb = Cb::new();
    assert!(
        cb.run(&[
            "write",
            "--ring",
            "2",
            "--kind",
            "knowledge",
            "--name",
            "n",
            "--body",
            "b"
        ])
        .status
        .success()
    );

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("export.jsonl");
    assert!(
        cb.run(&["policy", "audit", "--export", file.to_str().unwrap()])
            .status
            .success()
    );

    // Edit one row's subject, the way somebody would who wanted an event to look like a
    // different one. Everything else about the file stays valid JSON and the right length.
    let text = std::fs::read_to_string(&file).unwrap();
    let tampered: String = text
        .lines()
        .map(|l| {
            if l.contains("\"action\":\"note.write\"") {
                l.replace("note.write", "note.reads")
            } else {
                l.to_string()
            }
        })
        .map(|l| format!("{l}\n"))
        .collect();
    let bad = dir.path().join("tampered.jsonl");
    std::fs::write(&bad, tampered).unwrap();

    let check = Cb::bin().arg("verify-export").arg(&bad).output().unwrap();
    assert!(!check.status.success(), "a tampered file verified");
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(stderr.contains("chain broken"), "{stderr}");
}

/// The format is meant to outlive this program, so the check has to be writable by someone
/// else. `scripts/verify-audit-export.py` is that second implementation, and this test runs
/// it against a real bundle: if the two ever disagree, one of them is wrong about the rule.
///
/// Skipped, loudly, where python or the blake3 module is missing — a silent skip would turn
/// "we cannot check this here" into "this passed".
#[test]
fn the_independent_python_checker_agrees() {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts/verify-audit-export.py")
        .canonicalize()
        .expect("the script is in the repository");

    let probe = Command::new("python3")
        .args(["-c", "import blake3"])
        .output();
    match probe {
        Ok(o) if o.status.success() => {}
        _ => {
            eprintln!(
                "SKIPPED the_independent_python_checker_agrees: python3 with the blake3 \
                 module is not available here (pip install blake3)"
            );
            return;
        }
    }

    let cb = Cb::new();
    for i in 0..3 {
        assert!(
            cb.run(&[
                "write",
                "--ring",
                "2",
                "--kind",
                "knowledge",
                "--name",
                &format!("n{i}"),
                "--body",
                "Body with a \"quote\" and a Ümlaut, because the hash covers the detail.",
            ])
            .status
            .success()
        );
    }

    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("export.jsonl");
    assert!(
        cb.run(&["policy", "audit", "--export", file.to_str().unwrap()])
            .status
            .success()
    );

    let out = Command::new("python3")
        .arg(&script)
        .arg(&file)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the independent checker rejected a bundle this binary wrote:\n{stdout}"
    );
    assert!(stdout.contains("chain holds over 4 row(s)"), "{stdout}");
}
