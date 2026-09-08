//! Integration tests over the router, in process, against a store `App` builds itself.
//! Every request takes the user's route (SPEC §14.9): the HTTP API, JSON bodies, and the
//! files and databases on disk afterwards.
//!
//! Two of these were first shown to fail against the defect they cover (SPEC §14.2); the
//! module report says what was seen: the CSP header test without the header layer, and
//! the dry-run isolation test with `?dry_run=true` ignored by the write route.

use super::router_with;
use crate::app::App;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use cyberbrain_embed::synthetic::write_synthetic_model;
use cyberbrain_policy::Actor;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tower::ServiceExt;

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

struct Fx {
    _dir: tempfile::TempDir,
    store: PathBuf,
    app: Arc<App>,
    router: Router,
}

impl Fx {
    fn new() -> Fx {
        Fx::with_router(|app| router_with(app, PathBuf::new(), None, Vec::new()))
    }

    /// A fixture whose `POST /command` runs the real `cyberbrain` this test run built,
    /// rather than the test harness that `current_exe()` would name here.
    fn with_cli() -> Fx {
        let exe = cli_binary();
        Fx::with_router(move |app| router_with(app, exe.clone(), None, Vec::new()))
    }

    fn with_router(make: impl FnOnce(Arc<App>) -> Router) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        App::init(&store, &Actor::Operator).unwrap();
        let app = Arc::new(App::open(Some(&store), Actor::Operator).unwrap());
        let router = make(app.clone());
        Fx {
            _dir: dir,
            store,
            app,
            router,
        }
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

    async fn raw(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Vec<u8>) {
        let mut b = Request::builder().method(method).uri(path);
        for (k, v) in extra {
            b = b.header(*k, *v);
        }
        let req = match body {
            Some(v) => b
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(v.to_string()))
                .unwrap(),
            None => b.body(Body::empty()).unwrap(),
        };
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, headers, bytes.to_vec())
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let (status, _, bytes) = self.raw(method, path, body, &[]).await;
        let v = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&bytes)))
        };
        (status, v)
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.call(Method::GET, path, None).await
    }

    async fn ok(&self, path: &str) -> Value {
        let (s, v) = self.get(path).await;
        assert_eq!(s, StatusCode::OK, "{path}: {v}");
        v
    }

    /// Create a note through the API and return its detail.
    async fn create(&self, ring: u8, name: &str, body: &str) -> Value {
        let (s, v) = self
            .call(
                Method::POST,
                "/api/v1/notes",
                Some(json!({ "body": body, "front": { "name": name, "ring": ring, "kind": "knowledge" } })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        v
    }

    fn note_path(&self, ring: u8, name: &str) -> PathBuf {
        self.store
            .join("notes")
            .join(format!("r{ring}"))
            .join(format!("{name}.md"))
    }

    /// Every byte under the store, including SQLite's write-ahead logs: in-process, a
    /// committed write may live only in `-wal` until a checkpoint, and a snapshot that
    /// skipped it would call a real write a no-op. `-shm` is a reader-side index and
    /// changes on reads; it is excluded.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, out);
                } else if !p.to_string_lossy().ends_with("-shm") {
                    out.insert(p.clone(), std::fs::read(&p).unwrap());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(&self.store, &mut out);
        out
    }

    fn audit_count(&self) -> i64 {
        rusqlite::Connection::open(self.store.join("audit.db"))
            .unwrap()
            .query_row("SELECT count(*) FROM audit", [], |r| r.get(0))
            .unwrap()
    }
}

/// A snapshot minus the audit record, for the routes whose only legitimate footprint is
/// the row that says they happened.
fn without_audit(snap: &BTreeMap<PathBuf, Vec<u8>>) -> BTreeMap<PathBuf, Vec<u8>> {
    snap.iter()
        .filter(|(p, _)| !p.to_string_lossy().contains("audit.db"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect()
}

fn err_of(v: &Value) -> &Value {
    assert!(v.get("error").is_some(), "not an error body: {v}");
    &v["error"]
}

// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn status_reports_the_store_and_says_what_it_cannot_measure() {
    let fx = Fx::new();
    fx.create(2, "redis-limits", "Redis maxmemory must be set.")
        .await;
    fx.create(0, "never-copy-config", "Never copy config to server two.")
        .await;
    let s = fx.ok("/api/v1/status").await;
    assert_eq!(s["version"], env!("CARGO_PKG_VERSION"));
    // A JSON field compared across machines: forward slashes whatever the platform.
    assert_eq!(s["store"]["path"], cyberbrain_core::slash(fx.app.root()));
    assert_eq!(s["store"]["notes"], 2);
    assert_eq!(s["store"]["rings"].as_array().unwrap().len(), 5);
    assert_eq!(s["store"]["rings"][0]["notes"], 1);
    assert_eq!(s["store"]["rings"][2]["blocks"], 1);
    assert!(s["store"]["rings"][0]["tokens"].as_u64().unwrap() > 0);
    assert_eq!(s["store"]["resident_cap"]["tokens"], 8192);
    assert!(s["store"]["resident_cap"]["used"].as_u64().unwrap() > 0);
    assert!(s["store"]["db_bytes"].as_u64().unwrap() > 0);
    assert_eq!(s["index"]["stale_notes"], 0, "{s}");
    assert_eq!(s["index"]["orphan_vectors"], 0);
    assert_eq!(s["index"]["fts_ok"], true);
    assert_eq!(s["index"]["last_scan"], Value::Null);
    assert_eq!(s["embedding"]["loaded"], false);
    assert_eq!(s["embedding"]["backend"], "static");
    assert_eq!(s["inference"]["configured"], false);
    assert_eq!(s["inference"]["endpoint_class"], "loopback");
    assert_eq!(s["inference"]["last_backend"], "unknown");
    assert_eq!(s["policy"]["profile"], "eu");
    assert_eq!(s["policy"]["pii_scan"], true);
    assert!(s["policy"]["audit_rows"].as_u64().unwrap() >= 3);
    let caveats = s["caveats"].as_array().unwrap();
    assert!(!caveats.is_empty(), "unmeasured members must be named");

    // A hand edit makes the index stale, and the count is the real scan's.
    let p = fx.note_path(2, "redis-limits");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text.replace("must be set", "must be set, always")).unwrap();
    let s = fx.ok("/api/v1/status").await;
    assert_eq!(s["index"]["stale_notes"], 1, "{s}");

    let (st, v) = fx.call(Method::POST, "/api/v1/scan", None).await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["changed"], 1);
    assert_eq!(v["full"], false);
    let s = fx.ok("/api/v1/status").await;
    assert_eq!(s["index"]["stale_notes"], 0);
    assert!(s["index"]["last_scan"].is_string(), "{s}");
}

#[tokio::test]
async fn recall_hits_carry_citations_block_index_mode_and_params() {
    let fx = Fx::new();
    fx.create(
        2,
        "redis-limits",
        "Redis maxmemory must be set or the box swaps.",
    )
    .await;
    fx.create(3, "unrelated", "Alpha beta gamma.").await;
    let r = fx.ok("/api/v1/recall?q=redis%20maxmemory").await;
    let hits = r["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{r}");
    let cit = hits[0]["citation"].as_str().unwrap();
    assert!(cit.starts_with("r2-") && cit.len() == 15, "{cit}");
    assert_eq!(hits[0]["block_idx"], 0);
    assert_eq!(hits[0]["note_name"], "redis-limits");
    assert_eq!(hits[0]["sources"], json!(["lexical"]));
    assert_eq!(r["mode"], "lexical");
    assert_eq!(r["params"]["q"], "redis maxmemory");
    assert_eq!(r["params"]["n"], 8);
    assert_eq!(r["params"]["ring"], Value::Null);
    assert_eq!(r["params"]["k_lex"], 50);
    assert!(r["elapsed_ms"].is_number());
    assert!(
        r["caveats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().contains("contradiction check skipped")),
        "{r}"
    );

    let e = fx.ok(&format!("/api/v1/recall/{cit}")).await;
    assert_eq!(e["citation"], cit);
    assert_eq!(e["ring"], 2);
    assert_eq!(e["block_idx"], 0);
    assert!(e["block_text"].as_str().unwrap().contains("maxmemory"));
    assert_eq!(e["note"]["front"]["name"], "redis-limits");
    assert_eq!(e["note"]["path"], "notes/r2/redis-limits.md");
    assert_eq!(e["note"]["blocks"][0]["citation"], cit);

    let r = fx.ok("/api/v1/recall?q=alpha&ring=2&n=3").await;
    assert!(r["hits"].as_array().unwrap().is_empty(), "ring filter: {r}");
    assert_eq!(r["params"]["ring"], 2);
    assert_eq!(r["params"]["n"], 3);

    let (s, v) = fx.get("/api/v1/recall?q=%20").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(err_of(&v)["code"], "bad-request");
    assert_eq!(err_of(&v)["exit_code"], 1);
    let (s, _) = fx.get("/api/v1/recall?q=x&n=101").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, v) = fx.get("/api/v1/recall?q=x&ring=9").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(err_of(&v)["variant"], "bad-ring");
    let (s, v) = fx.get("/api/v1/recall/not-a-citation").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "malformed is a user error: {v}");
    let (s, v) = fx.get("/api/v1/recall/r2-a91f2c33e1bd").await;
    assert_eq!(s, StatusCode::NOT_FOUND, "well-formed but unknown: {v}");
    assert_eq!(err_of(&v)["code"], "not-found");
}

#[tokio::test]
async fn notes_list_detail_and_graph_come_from_the_files() {
    let fx = Fx::new();
    let a = fx
        .create(
            2,
            "pg18-moves-pgdata",
            "Postgres 18 moved PGDATA. See [[docker-bind-mount-inode-drift]] and [[nowhere]].",
        )
        .await;
    fx.create(
        2,
        "docker-bind-mount-inode-drift",
        "Bind mounts keep the inode.",
    )
    .await;
    fx.create(3, "session-one", "alpha").await;

    let list = fx.ok("/api/v1/notes").await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 3);
    assert_eq!(names[0], "session-one", "updated desc: {names:?}");
    let pg = list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "pg18-moves-pgdata")
        .unwrap();
    assert_eq!(pg["links_out"], 2);
    assert_eq!(pg["dangling"], 1);
    assert_eq!(pg["blocks"], 1);
    assert_eq!(pg["pii"], "none");
    assert!(pg["bytes"].as_u64().unwrap() > 0);
    let docker = list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"] == "docker-bind-mount-inode-drift")
        .unwrap();
    assert_eq!(docker["links_in"], 1);

    let l = fx.ok("/api/v1/notes?ring=3").await;
    assert_eq!(l.as_array().unwrap().len(), 1);
    let l = fx.ok("/api/v1/notes?q=docker&sort=name").await;
    assert_eq!(l.as_array().unwrap().len(), 1);
    let (s, _) = fx.get("/api/v1/notes?sort=sideways").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let d = fx.ok("/api/v1/notes/pg18-moves-pgdata").await;
    assert_eq!(d["path"], "notes/r2/pg18-moves-pgdata.md");
    assert_eq!(d["outbound"][0]["target"], "docker-bind-mount-inode-drift");
    assert_eq!(d["outbound"][0]["resolved"]["ring"], 2);
    assert_eq!(d["outbound"][1]["target"], "nowhere");
    assert_eq!(
        d["outbound"][1]["resolved"],
        Value::Null,
        "dangling is intent"
    );
    assert_eq!(d["blocks"][0]["idx"], 0);
    assert!(
        d["blocks"][0]["preview"]
            .as_str()
            .unwrap()
            .starts_with("Postgres 18")
    );
    let by_id = fx
        .ok(&format!(
            "/api/v1/notes/{}",
            a["front"]["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(by_id["front"]["name"], "pg18-moves-pgdata");
    let d = fx.ok("/api/v1/notes/docker-bind-mount-inode-drift").await;
    assert_eq!(d["inbound"][0]["from"]["name"], "pg18-moves-pgdata");

    let (s, v) = fx.get("/api/v1/notes/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(err_of(&v)["code"], "not-found");
    assert_eq!(err_of(&v)["exit_code"], 1);
    assert_eq!(err_of(&v)["variant"], "no-such-note");

    let g = fx.ok("/api/v1/graph").await;
    assert_eq!(g["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(g["edges"].as_array().unwrap().len(), 1);
    assert_eq!(g["dangling"][0]["to_name"], "nowhere");
    assert_eq!(g["dangling"][0]["from"], a["front"]["id"]);
}

#[tokio::test]
async fn put_writes_the_file_reindexes_in_the_same_request_and_409s_on_a_stale_stamp() {
    let fx = Fx::new();
    let d = fx
        .create(2, "redis-limits", "Redis maxmemory must be set.")
        .await;
    let updated = d["front"]["updated"].as_str().unwrap().to_string();

    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/redis-limits",
            Some(json!({ "body": "Redis maxmemory must be set. Unique marker.", "expected_updated": updated, "front": { "tags": ["redis"] } })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["front"]["tags"], json!(["redis"]));
    assert!(v["body"].as_str().unwrap().contains("Unique marker"));
    let text = std::fs::read_to_string(fx.note_path(2, "redis-limits")).unwrap();
    assert!(text.contains("Unique marker"), "{text}");
    assert!(text.contains("- redis"), "{text}");
    // §8.1: the index is current in the same request.
    let r = fx.ok("/api/v1/recall?q=marker").await;
    assert_eq!(r["hits"].as_array().unwrap().len(), 1, "{r}");

    // The stamp the first client saw has moved; last-writer-wins is refused.
    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/redis-limits",
            Some(json!({ "body": "clobber", "expected_updated": updated })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let e = err_of(&v);
    assert_eq!(e["code"], "write-conflict");
    assert_eq!(e["exit_code"], 1);
    assert_eq!(
        e["current_updated"],
        v_str(&fx.ok("/api/v1/notes/redis-limits").await["front"]["updated"])
    );
    let text = std::fs::read_to_string(fx.note_path(2, "redis-limits")).unwrap();
    assert!(!text.contains("clobber"));

    // Server-owned fields, renames, and a missing note.
    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/redis-limits",
            Some(json!({ "body": "x", "expected_updated": updated, "front": { "id": "01ARZ3NDEKTSV4RRFFQ69G5FAV" } })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(err_of(&v)["code"], "bad-frontmatter");
    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/redis-limits",
            Some(json!({ "body": "x", "expected_updated": updated, "front": { "name": "other" } })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, _) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/missing",
            Some(json!({ "body": "x", "expected_updated": updated })),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/redis-limits",
            Some(json!({ "body": 3 })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(err_of(&v)["code"], "bad-request");
}

fn v_str(v: &Value) -> Value {
    v.clone()
}

#[tokio::test]
async fn post_creates_and_refuses_to_overwrite_or_take_server_owned_fields() {
    let fx = Fx::new();
    let d = fx.create(4, "imported", "External claim.").await;
    assert_eq!(d["front"]["ring"], 4);
    assert_eq!(d["front"]["kind"], "knowledge");
    assert!(fx.note_path(4, "imported").is_file());

    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "body": "again", "front": { "name": "imported", "ring": 4, "kind": "knowledge" } })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(err_of(&v)["code"], "write-conflict");

    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "body": "x", "front": { "name": "n", "ring": 2, "kind": "knowledge", "links": ["a"] } })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(err_of(&v)["code"], "bad-frontmatter");
    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "body": "x", "front": { "name": "Bad Name", "ring": 2, "kind": "knowledge" } })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert_eq!(err_of(&v)["code"], "bad-frontmatter");
    assert_eq!(err_of(&v)["variant"], "frontmatter");

    // Ring cap: a user error with the numbers the UI needs to explain it.
    let big = "word ".repeat(9000);
    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/notes",
            Some(
                json!({ "body": big, "front": { "name": "huge", "ring": 0, "kind": "decision" } }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let e = err_of(&v);
    assert_eq!(e["code"], "ring-cap-exceeded");
    assert_eq!(e["exit_code"], 1);
    assert_eq!(e["cap"]["tokens"], 8192);
    assert!(e["cap"]["would_be"].as_u64().unwrap() > 8192);
    assert_eq!(e["cap"]["used"], 0);
}

#[tokio::test]
async fn a_held_write_is_409_with_findings_and_a_hold_the_operator_can_answer() {
    let fx = Fx::new();
    let d = fx.create(3, "contact", "Meeting notes.").await;
    let updated = d["front"]["updated"].as_str().unwrap().to_string();
    let before = fx.snapshot();

    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/contact",
            Some(json!({ "body": "Meeting notes.\nreach bob@corp.example.org", "expected_updated": updated })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    let e = err_of(&v);
    assert_eq!(e["code"], "pii-held");
    assert_eq!(e["exit_code"], 3, "a hold is a policy decision");
    let hold = &e["hold"];
    assert_eq!(hold["note"], "contact");
    assert_eq!(hold["findings"][0]["kind"], "email");
    assert_eq!(hold["findings"][0]["line"], 2);
    assert_eq!(hold["findings"][0]["col"], 7);
    let excerpt = hold["findings"][0]["excerpt"].as_str().unwrap();
    assert!(!excerpt.contains("bob@"), "never the raw match: {excerpt}");
    assert!(excerpt.contains("@corp.example.org"), "{excerpt}");
    assert!(hold["expires_at"].is_string());
    let hold_id = hold["hold_id"].as_str().unwrap().to_string();
    // A hold writes nothing to the tree or the index. It *is* recorded: the audit log
    // gains a `note.write.held` row (kinds and offsets, never the text), so the store
    // comparison excludes audit.db and the row is checked on its own.
    assert_eq!(
        without_audit(&before),
        without_audit(&fx.snapshot()),
        "a held write writes nothing"
    );
    let held = fx.ok("/api/v1/policy/audit?action=note.write.held").await;
    assert_eq!(held["rows"][0]["subject"], "note:contact");
    assert!(
        !held.to_string().contains("bob@"),
        "the matched text never enters the log"
    );

    // The hold is visible on the compliance screen until it is answered.
    let p = fx.ok("/api/v1/policy/pii").await;
    assert_eq!(p["holds"][0]["hold_id"], hold_id);

    // Redact: written, findings gone, state reviewed, and the log never saw the address.
    let (s, v) = fx
        .call(
            Method::POST,
            &format!("/api/v1/holds/{hold_id}"),
            Some(json!({ "action": "redact" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(
        v["body"].as_str().unwrap().contains("[redacted:email]"),
        "{v}"
    );
    assert_eq!(v["front"]["pii"], "reviewed");
    let text = std::fs::read_to_string(fx.note_path(3, "contact")).unwrap();
    assert!(
        text.contains("[redacted:email]") && !text.contains("bob@"),
        "{text}"
    );
    let (s, _) = fx
        .call(
            Method::POST,
            &format!("/api/v1/holds/{hold_id}"),
            Some(json!({ "action": "redact" })),
        )
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "a hold is answered once");
    let a = fx.ok("/api/v1/policy/audit?action=hold-resolved").await;
    assert_eq!(a["rows"][0]["action"], "note.write.resolved");
    assert!(!a.to_string().contains("bob@"));

    // Discard: nothing written, 204, and the decision is on the record.
    let updated = v["front"]["updated"].as_str().unwrap().to_string();
    let (_, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/contact",
            Some(json!({ "body": "call +49 371 1234567 now", "expected_updated": updated })),
        )
        .await;
    let hold_id = err_of(&v)["hold"]["hold_id"].as_str().unwrap().to_string();
    let before = fx.snapshot();
    let (s, headers, bytes) = fx
        .raw(
            Method::POST,
            &format!("/api/v1/holds/{hold_id}"),
            Some(json!({ "action": "discard" })),
            &[],
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert!(bytes.is_empty());
    assert!(headers.get(header::CONTENT_SECURITY_POLICY).is_some());
    // The discard row is the one permitted change.
    assert_eq!(
        without_audit(&before),
        without_audit(&fx.snapshot()),
        "discard wrote to the store"
    );
    let a = fx
        .ok("/api/v1/policy/audit?action=note.write.discarded")
        .await;
    assert_eq!(a["rows"].as_array().unwrap().len(), 1, "{a}");

    // Proceed and mark-reviewed keep the body and stamp the state.
    let (_, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/contact",
            Some(json!({ "body": "call +49 371 1234567 now", "expected_updated": updated })),
        )
        .await;
    let hold_id = err_of(&v)["hold"]["hold_id"].as_str().unwrap().to_string();
    let (s, v) = fx
        .call(
            Method::POST,
            &format!("/api/v1/holds/{hold_id}"),
            Some(json!({ "action": "proceed" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["front"]["pii"], "flagged");
    assert!(v["body"].as_str().unwrap().contains("1234567"));
    let p = fx.ok("/api/v1/policy/pii").await;
    assert_eq!(p["entries"][0]["state"], "flagged");
    assert_eq!(p["entries"][0]["findings"][0]["kind"], "phone");
    assert!(p["holds"].as_array().unwrap().is_empty());
    let (s, v) = fx
        .call(
            Method::POST,
            &format!("/api/v1/holds/{hold_id}"),
            Some(json!({ "action": "shred" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
}

/// The error contract for all three exit codes. A policy refusal cannot currently be
/// provoked through a route (App turns the only refusal source, the egress gate, into a
/// recall caveat), so its mapping is exercised on the type; the other two go over HTTP.
#[tokio::test]
async fn error_bodies_carry_the_cli_exit_code() {
    use super::error::ApiError;
    use axum::response::IntoResponse;
    use cyberbrain_core::Error;

    let fx = Fx::new();
    // 1: user error over HTTP.
    let (s, v) = fx.get("/api/v1/notes/does-not-exist").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        v,
        json!({ "error": { "code": "not-found", "message": "no note named does-not-exist", "exit_code": 1, "variant": "no-such-note" } })
    );
    let (s, v) = fx.get("/api/v1/policy/subject?q=ab").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(err_of(&v)["exit_code"], 1);
    assert_eq!(err_of(&v)["code"], "bad-request");
    let (s, v) = fx.get("/api/v1/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(err_of(&v)["code"], "not-found");

    // 2: internal error over HTTP. The notes tree vanishing under a running server is an
    // I/O failure, which the taxonomy calls internal.
    std::fs::remove_dir_all(fx.store.join("notes")).unwrap();
    let (s, v) = fx.get("/api/v1/notes").await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{v}");
    let e = err_of(&v);
    assert_eq!(e["code"], "internal");
    assert_eq!(e["exit_code"], 2);
    assert_eq!(e["variant"], "io");

    // 3: policy refusal, on the mapping itself.
    let core = Error::PolicyRefusal {
        profile: "eu".into(),
        reason: "public endpoint".into(),
    };
    assert_eq!(core.exit_code(), 3);
    let api = ApiError::from(core);
    assert_eq!(api.status, StatusCode::FORBIDDEN);
    assert_eq!(api.exit_code, 3);
    let body = api.body();
    assert_eq!(body["error"]["code"], "policy-refusal");
    assert_eq!(body["error"]["exit_code"], 3);
    let resp = api.into_response();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        resp.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    // Every core variant maps to a status whose exit code is the variant's own.
    for e in [
        Error::Index("x".into()),
        Error::Embed("x".into()),
        Error::Llm("x".into()),
        Error::StoreIntegrity("x".into()),
        Error::Config("x".into()),
        Error::BadRing(9),
        Error::BadCitation("x".into()),
    ] {
        let want = e.exit_code();
        assert_eq!(ApiError::from(e).exit_code, want);
    }
}

/// SPEC §8.1: the CSP is a *header*, on API responses and assets alike, and it carries
/// `frame-ancestors 'none'`, which a `<meta>` cannot. Shown to fail with the header layer
/// removed from the router: every response came back without a Content-Security-Policy
/// at all (the meta in the page was, as the spec warns, the only copy).
#[tokio::test]
async fn csp_is_a_header_with_frame_ancestors_on_api_and_asset_responses() {
    let fx = Fx::new();
    let (s, h, _) = fx.raw(Method::GET, "/api/v1/status", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let csp = h
        .get(header::CONTENT_SECURITY_POLICY)
        .expect("CSP header on an API response")
        .to_str()
        .unwrap()
        .to_string();
    assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    assert!(csp.contains("default-src 'none'"), "{csp}");
    assert!(csp.contains("connect-src 'self'"), "{csp}");
    assert_eq!(h.get(header::CACHE_CONTROL).unwrap(), "no-store");
    assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");

    // The API half above holds in every build. The rest needs a page to serve, and
    // without the `ui` feature there is none — that is the configuration a published
    // crate installs under, and it is what CI runs.
    #[cfg(feature = "ui")]
    csp_on_the_page(&fx, &csp).await;
    #[cfg(not(feature = "ui"))]
    {
        let _ = &csp;
        let (s, ..) = fx.raw(Method::GET, "/", None, &[]).await;
        assert_eq!(
            s,
            StatusCode::NOT_FOUND,
            "without the ui feature there is no page, and the route must say so plainly"
        );
    }
}

/// The page half of the CSP check. Only meaningful when a page is embedded.
#[cfg(feature = "ui")]
async fn csp_on_the_page(fx: &Fx, csp: &str) {
    let (s, h, body) = fx.raw(Method::GET, "/", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let page = String::from_utf8_lossy(&body);
    assert!(page.contains("<div id=\"root\">"), "{page}");
    assert_eq!(
        h.get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap(),
        csp
    );
    assert!(
        h.get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    // The header is the page's own policy plus the directive only a header can carry, so
    // the inline-script hash the bundler computed is the one the header allows.
    let meta = super::assets::meta_csp().expect("the built page carries a CSP meta");
    assert!(
        csp.starts_with(meta.trim_end_matches(';')),
        "header {csp} vs meta {meta}"
    );
    assert!(
        !meta.contains("frame-ancestors"),
        "the meta cannot carry it; the header must"
    );

    // An asset: correct type, an ETag, immutable caching, a 304 on revalidation.
    let js = page
        .split("src=\"./")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("index references a script");
    let (s, h, body) = fx.raw(Method::GET, &format!("/{js}"), None, &[]).await;
    assert_eq!(s, StatusCode::OK, "{js}");
    assert!(!body.is_empty());
    assert!(
        h.get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("javascript")
    );
    assert!(
        h.get(header::CONTENT_SECURITY_POLICY)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert_eq!(
        h.get(header::CACHE_CONTROL).unwrap(),
        "public, max-age=31536000, immutable"
    );
    let etag = h.get(header::ETAG).unwrap().to_str().unwrap().to_string();
    assert!(etag.starts_with('"') && etag.len() == 18, "{etag}");
    let (s, h, body) = fx
        .raw(
            Method::GET,
            &format!("/{js}"),
            None,
            &[("if-none-match", etag.as_str())],
        )
        .await;
    assert_eq!(s, StatusCode::NOT_MODIFIED);
    assert!(body.is_empty());
    assert_eq!(h.get(header::ETAG).unwrap(), etag.as_str());

    // Unknown paths: extension-less ones get the page (hash router), files do not.
    let (s, _, body) = fx.raw(Method::GET, "/some/deep/link", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert!(String::from_utf8_lossy(&body).contains("<div id=\"root\">"));
    let (s, _, body) = fx.raw(Method::GET, "/assets/missing.css", None, &[]).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(err_of(&v)["code"], "not-found");
    let (s, _, _) = fx.raw(Method::GET, "/../Cargo.toml", None, &[]).await;
    assert_ne!(s, StatusCode::OK);
    let (s, h, body) = fx.raw(Method::GET, "/favicon.svg", None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        h.get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("svg")
    );
    assert!(String::from_utf8_lossy(&body).contains("<svg"));
}

/// SPEC §8 / §8.2 / §14.6: `?dry_run=true` runs the real path with no-op writers. Shown
/// to fail with the query parameter ignored by the write route (`dry_run: false` passed
/// to `App::write`): the snapshot differed in `notes/r2/keep.md`, `cyberbrain.db-wal`
/// and `audit.db-wal`, and the audit count had grown by one.
#[tokio::test]
async fn dry_run_mutations_leave_the_store_byte_identical() {
    let fx = Fx::new();
    fx.install_model(1);
    fx.create(2, "keep", "postgres pgdata is unique").await;
    fx.create(2, "gone-soon", "docker inode drift").await;
    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "body": "alpha beta", "front": { "name": "old-session", "ring": 3, "kind": "session", "retention": "P1D" } })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let p = fx.note_path(3, "old-session");
    let text = std::fs::read_to_string(&p).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("created:"))
        .unwrap()
        .to_string();
    std::fs::write(&p, text.replace(&line, "created: 2020-01-01T00:00:00Z")).unwrap();
    let (s, v) = fx.call(Method::POST, "/api/v1/scan", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let keep = fx.ok("/api/v1/notes/keep").await;
    let updated = keep["front"]["updated"].as_str().unwrap().to_string();

    // The instrument: a real write must move the snapshot, or the assertions below
    // prove nothing.
    let probe = fx.snapshot();
    let (s, _) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/keep",
            Some(
                json!({ "body": "postgres pgdata is unique marker", "expected_updated": updated }),
            ),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_ne!(probe, fx.snapshot(), "the snapshot cannot see a real write");
    let keep = fx.ok("/api/v1/notes/keep").await;
    let updated = keep["front"]["updated"].as_str().unwrap().to_string();

    let audit_before = fx.audit_count();
    let before = fx.snapshot();

    // PUT
    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/keep?dry_run=true",
            Some(json!({ "body": "postgres pgdata is unique marker, edited", "expected_updated": updated })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(before, fx.snapshot(), "dry-run PUT changed the store");
    assert_eq!(v["dry_run"], true);
    assert!(
        v["body"].as_str().unwrap().ends_with("edited"),
        "describes what would be written"
    );

    // POST
    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/notes?dry_run=true",
            Some(json!({ "body": "beta gamma", "front": { "name": "new-one", "ring": 2, "kind": "lesson" } })),
        )
        .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "a dry run creates nothing, so not 201: {v}"
    );
    assert_eq!(v["front"]["name"], "new-one");
    assert_eq!(v["dry_run"], true);
    assert!(!fx.note_path(2, "new-one").exists());
    assert_eq!(before, fx.snapshot(), "dry-run POST changed the store");

    // A held dry-run write, and its resolution, stay dry.
    let (s, v) = fx
        .call(
            Method::PUT,
            "/api/v1/notes/keep?dry_run=true",
            Some(json!({ "body": "mail bob@corp.example.org", "expected_updated": updated })),
        )
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{v}");
    assert_eq!(err_of(&v)["hold"]["dry_run"], true);
    let hold_id = err_of(&v)["hold"]["hold_id"].as_str().unwrap().to_string();
    let (s, v) = fx
        .call(
            Method::POST,
            &format!("/api/v1/holds/{hold_id}"),
            Some(json!({ "action": "redact" })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["dry_run"], true);
    assert!(
        v["body"].as_str().unwrap().contains("[redacted:email]"),
        "{v}"
    );
    assert_eq!(
        before,
        fx.snapshot(),
        "dry-run hold resolution changed the store"
    );

    // DELETE
    let (s, v) = fx
        .call(Method::DELETE, "/api/v1/notes/keep?dry_run=true", None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["note"]["name"], "keep");
    assert_eq!(v["note"]["path"], "notes/r2/keep.md");
    assert_eq!(
        v["removed"]["file"], true,
        "it *would* have removed the file"
    );
    assert_eq!(v["removed"]["blocks"], 1, "the real erasure logic counted");
    assert_eq!(v["removed"]["vectors"], 1);
    assert_eq!(v["removed"]["fts_rows"], 1);
    assert!(fx.note_path(2, "keep").is_file());
    assert_eq!(before, fx.snapshot(), "dry-run DELETE changed the store");

    // scan, with real work for it to find.
    std::fs::remove_file(fx.note_path(2, "gone-soon")).unwrap();
    let before = fx.snapshot();
    let (s, v) = fx
        .call(Method::POST, "/api/v1/scan?full=true&dry_run=true", None)
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["full"], true);
    assert_eq!(v["removed"], 1, "{v}");
    assert_eq!(v["detail"]["dropped_missing_file"], json!(["gone-soon"]));
    assert_eq!(before, fx.snapshot(), "dry-run scan changed the store");

    // retention apply
    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/policy/retention/apply?dry_run=true",
            Some(json!({})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["removed"].as_array().unwrap().len(), 1);
    assert_eq!(v["removed"][0]["note"]["name"], "old-session");
    assert_eq!(v["removed"][0]["dry_run"], true);
    assert!(p.is_file(), "a dry-run sweep erases nothing");
    assert_eq!(
        before,
        fx.snapshot(),
        "dry-run retention apply changed the store"
    );

    assert_eq!(
        fx.audit_count(),
        audit_before,
        "dry runs must not append to audit.db"
    );

    // And the real things still work afterwards.
    let (s, v) = fx.call(Method::DELETE, "/api/v1/notes/keep", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["dry_run"], false);
    assert!(!fx.note_path(2, "keep").exists());
    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/policy/retention/apply",
            Some(json!({})),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(!p.exists(), "the sweep erased the due note");
    assert!(fx.audit_count() > audit_before);
}

#[tokio::test]
async fn policy_routes_read_the_register_the_log_retention_models_and_subjects() {
    let fx = Fx::new();
    fx.create(2, "team", "Bob Smith owns the pager.").await;
    fx.create(3, "fresh", "gamma").await;

    // The obligation catalogue: the profile's own claims, each with a basis and a
    // confidence. Read over HTTP because that is where it was missing: the list existed in
    // the library and no surface printed it.
    let o = fx.ok("/api/v1/policy/obligations").await;
    assert_eq!(o["profile"], "eu");
    assert!(o["law"].as_str().unwrap().contains("2016/679"));
    let obs = o["obligations"].as_array().unwrap();
    assert!(
        obs.len() >= 10,
        "eu encodes more than a handful: {}",
        obs.len()
    );
    for ob in obs {
        assert!(
            !ob["basis"].as_str().unwrap().is_empty(),
            "no basis on {ob}"
        );
        let c = ob["confidence"].as_str().unwrap();
        assert!(matches!(c, "low" | "medium" | "high"), "odd confidence {c}");
        // The rule the library enforces in its own tests, now visible to a caller: a line
        // the author is unsure about has to say what is unsure.
        if c == "low" {
            assert!(
                !ob["note"].as_str().unwrap().is_empty(),
                "low without a note: {ob}"
            );
        }
    }
    assert!(
        obs.iter().any(|o| o["topic"] == "ai-regulation"),
        "the AI Act line is the one a deployer comes for"
    );

    // Egress register: the spec's two purposes and nothing else.
    let e = fx.ok("/api/v1/policy/egress").await;
    assert_eq!(e["profile"], "eu");
    assert!(e["since"].is_string());
    assert!(e["register_hash"].as_str().unwrap().starts_with("b3:"));
    assert_eq!(e["refused_total"], 0);
    let paths = e["paths"].as_array().unwrap();
    assert_eq!(
        paths.len(),
        4,
        "model download, local inference, audit sync, terminal"
    );
    assert_eq!(paths[0]["purpose"], "model-download");
    // Audit sync is in the register whether or not the store is enrolled, and says which
    // it is. A path that only appears once it is in use is a path nobody audits.
    assert_eq!(paths[2]["purpose"], "audit-sync");
    assert_eq!(paths[2]["enabled"], false);
    assert_eq!(paths[2]["carries_note_content"], false);
    assert_eq!(paths[0]["enabled"], false);
    assert!(
        paths[0]["disabled_reason"]
            .as_str()
            .unwrap()
            .contains("no model_source")
    );
    assert_eq!(paths[0]["destination_class"], "unresolved");
    assert_eq!(paths[0]["uses_total"], 0);
    assert_eq!(paths[1]["purpose"], "local-inference");
    assert_eq!(paths[1]["enabled"], true);
    assert_eq!(paths[1]["disabled_reason"], Value::Null);
    assert_eq!(paths[1]["destination"], "http://127.0.0.1:11434/v1");
    assert_eq!(paths[1]["destination_class"], "loopback");
    assert_eq!(paths[1]["permitted_by"], json!(["eu", "ch", "off"]));
    assert_eq!(paths[1]["carries_note_content"], true);

    // Audit: newest first, numbered, filterable in either vocabulary, paged.
    let a = fx.ok("/api/v1/policy/audit").await;
    let rows = a["rows"].as_array().unwrap();
    assert!(rows.len() >= 3, "{a}");
    assert_eq!(rows[0]["action"], "note.write");
    assert!(rows[0]["seq"].as_u64().unwrap() > rows[1]["seq"].as_u64().unwrap());
    assert_eq!(rows.last().unwrap()["action"], "store.init");
    assert_eq!(rows.last().unwrap()["seq"], 1);
    assert_eq!(rows[0]["detail"]["name"], "fresh");
    assert!(
        rows[0]["detail"].get("_chain").is_none(),
        "the chain is not for display"
    );
    assert_eq!(a["total"], rows.len());
    assert_eq!(a["next_before"], Value::Null);
    let w = fx.ok("/api/v1/policy/audit?action=write").await;
    assert_eq!(w["rows"].as_array().unwrap().len(), 2);
    let w = fx
        .ok("/api/v1/policy/audit?action=note.write&limit=1")
        .await;
    assert_eq!(w["rows"].as_array().unwrap().len(), 1);
    assert_eq!(w["total"], 2);
    let next = w["next_before"]
        .as_u64()
        .expect("a cursor when more rows remain");
    let w2 = fx
        .ok(&format!(
            "/api/v1/policy/audit?action=note.write&limit=1&before={next}"
        ))
        .await;
    assert_eq!(w2["rows"][0]["detail"]["name"], "team");
    assert_eq!(w2["next_before"], Value::Null);
    let (s, v) = fx.get("/api/v1/policy/audit?action=gibtsnicht").await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    assert!(
        err_of(&v)["message"]
            .as_str()
            .unwrap()
            .contains("note.write")
    );
    let q = fx.ok("/api/v1/policy/audit?q=fresh").await;
    assert_eq!(q["rows"].as_array().unwrap().len(), 1);
    let act = fx.ok("/api/v1/policy/audit?actor=operator").await;
    assert!(act["rows"].as_array().unwrap().len() >= 3);

    // Subject access: found with a citation, recorded by hash.
    let s = fx.ok("/api/v1/policy/subject?q=Bob%20Smith").await;
    assert_eq!(s["identifier"], "Bob Smith");
    assert_eq!(s["hits"][0]["where"], "block");
    assert_eq!(s["hits"][0]["ref"], "team");
    assert!(
        s["hits"][0]["citation"]
            .as_str()
            .unwrap()
            .starts_with("r2-")
    );
    assert_eq!(s["searched"]["notes"], 2);
    assert_eq!(s["searched"]["blocks"], 2);
    assert!(s["searched"]["audit_rows"].as_u64().unwrap() >= 3);
    let a = fx.ok("/api/v1/policy/audit?action=subject-access").await;
    assert!(
        a["rows"][0]["subject"]
            .as_str()
            .unwrap()
            .starts_with("id:blake3:")
    );
    let (s, _) = fx.get("/api/v1/policy/subject").await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // Retention: the queue lists what is due and never erases by itself.
    let (s, _) = fx
        .call(
            Method::POST,
            "/api/v1/notes",
            Some(json!({ "body": "alpha", "front": { "name": "old", "ring": 3, "kind": "session", "retention": "P1D" } })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let p = fx.note_path(3, "old");
    let text = std::fs::read_to_string(&p).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("created:"))
        .unwrap()
        .to_string();
    std::fs::write(&p, text.replace(&line, "created: 2020-01-01T00:00:00Z")).unwrap();
    let r = fx.ok("/api/v1/policy/retention").await;
    assert_eq!(r["due"], 1, "{r}");
    assert_eq!(r["indefinite"], 2);
    assert_eq!(r["entries"][0]["note"]["name"], "old");
    assert_eq!(r["entries"][0]["due"], true);
    assert_eq!(r["entries"][0]["retention"], "P1D");
    assert_eq!(r["entries"][0]["expires_at"], "2020-01-02T00:00:00Z");
    assert!(p.is_file(), "listing never erases");
    let (s, v) = fx
        .call(
            Method::POST,
            "/api/v1/policy/retention/apply",
            Some(json!({ "names": ["old"] })),
        )
        .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a subset is refused, not widened: {v}"
    );
    assert!(p.is_file());

    // Model card: nothing in use says so; a model gives a card with its hash. The
    // embedder is loaded once per process (App's OnceLock), so a model placed after the
    // first use is not seen until restart: a fresh App stands in for that restart here.
    let m = fx.ok("/api/v1/policy/model-card").await;
    assert!(m.as_array().unwrap().is_empty());
    fx.install_model(1);
    let m = fx.ok("/api/v1/policy/model-card").await;
    assert!(
        m.as_array().unwrap().is_empty(),
        "a model installed after the embedder was first consulted is not loaded by a running server; restart"
    );
    let fx = Fx::new();
    fx.install_model(1);
    fx.create(3, "fresh", "gamma").await;
    let m = fx.ok("/api/v1/policy/model-card").await;
    assert_eq!(m[0]["role"], "embedding", "{m}");
    assert_eq!(m[0]["dim"], 16);
    assert_eq!(m[0]["license"], "not stated", "never guessed");
    assert!(m[0]["hash"].as_str().unwrap().len() == 64);
    assert_eq!(m[0]["active"], true);
    assert_eq!(m[0]["verified_at"], Value::Null);
    let st = fx.ok("/api/v1/status").await;
    assert_eq!(st["embedding"]["loaded"], true);
    assert_eq!(st["embedding"]["dim"], 16);
    let r = fx.ok("/api/v1/recall?q=gamma").await;
    assert_eq!(r["mode"], "hybrid", "{r}");
    assert_eq!(r["hits"][0]["sources"], json!([]));
}

#[tokio::test]
async fn doctor_maps_findings_and_scan_reports_what_it_did() {
    let fx = Fx::new();
    fx.create(2, "a", "see [[missing]]").await;
    let d = fx.ok("/api/v1/doctor").await;
    assert_eq!(d["ok"], false);
    assert!(d["checked_at"].is_string());
    let f = d["findings"].as_array().unwrap();
    assert_eq!(f.len(), 1, "{d}");
    assert_eq!(f[0]["check"], "dangling-link");
    assert_eq!(f[0]["severity"], "warn");
    assert_eq!(f[0]["subject"], "a");
    assert!(f[0]["message"].as_str().unwrap().contains("[[missing]]"));
    assert!(d["checks_run"].as_array().unwrap().len() >= 8);

    std::fs::write(fx.note_path(2, "junk"), "not a note").unwrap();
    let d = fx.ok("/api/v1/doctor").await;
    let checks: Vec<&str> = d["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["check"].as_str().unwrap())
        .collect();
    assert!(checks.contains(&"unreadable-note"), "{d}");
    std::fs::remove_file(fx.note_path(2, "junk")).unwrap();

    let (s, v) = fx.call(Method::POST, "/api/v1/scan?full=true", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["full"], true);
    assert_eq!(v["dry_run"], false);
    assert_eq!(v["scanned"], 1);
    assert_eq!(v["added"], 1, "a full scan re-adds everything: {v}");
    assert_eq!(v["blocks_written"], 1);
    assert_eq!(v["vectors_written"], 0);
    assert!(v["elapsed_ms"].is_number());
    let (s, v) = fx.call(Method::POST, "/api/v1/scan", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["changed"], 0);
    assert_eq!(v["detail"]["unchanged"], 1);
    let st = fx.ok("/api/v1/status").await;
    assert!(st["index"]["last_full_scan"].is_string(), "{st}");
    assert!(st["index"]["last_scan"].is_string());
}

#[tokio::test]
async fn pii_findings_are_masked_and_never_the_raw_match() {
    use super::notes::mask;
    use cyberbrain_policy::PiiKind;
    for (kind, raw) in [
        (PiiKind::Email, "bob@corp.example.org"),
        (PiiKind::Ipv4, "203.0.113.9"),
        (PiiKind::Ipv6, "2001:db8::1"),
        (PiiKind::ApiKey, "sk-live-abcdefghijklmnop"),
        (PiiKind::Iban, "DE89 3704 0044 0532 0130 00"),
        (PiiKind::Phone, "+49 371 1234567"),
    ] {
        let m = mask(kind, raw);
        assert_ne!(m, raw, "{kind:?}");
        assert!(!m.contains(raw), "{kind:?}: {m}");
    }
    assert_eq!(
        mask(PiiKind::Email, "bob@corp.example.org"),
        "b***@corp.example.org"
    );
    assert_eq!(mask(PiiKind::Ipv4, "203.0.113.9"), "203.0.*.*");
}

#[tokio::test]
async fn the_bind_is_loopback_and_the_port_is_the_only_knob() {
    // `serve` takes an `Arc<App>` and a port; there is no address parameter to open.
    // Bind an ephemeral loopback port through the same code path and hit it for real.
    let fx = Fx::new();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    assert!(addr.ip().is_loopback());
    let app = fx.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            super::router_with(app, PathBuf::new(), None, Vec::new()),
        )
        .await
        .unwrap();
    });
    let body = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        write!(
            s,
            "GET /api/v1/status HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    })
    .await
    .unwrap();
    assert!(body.starts_with("HTTP/1.1 200"), "{body}");
    assert!(
        body.to_lowercase().contains("content-security-policy:"),
        "{body}"
    );
    assert!(body.contains("\"version\""), "{body}");
    server.abort();
}

// ---------------------------------------------------------------------------------------
// POST /command — the command line, in the window.

/// The binary this test run built, found the way the desktop launcher's tests find it:
/// `target/<profile>/deps/<test binary>` sits two levels under `target/<profile>`.
fn cli_binary() -> PathBuf {
    let mut dir = std::env::current_exe().expect("test binary path");
    dir.pop();
    if dir.ends_with("deps") {
        dir.pop();
    }
    let exe = dir.join(if cfg!(windows) {
        "cyberbrain.exe"
    } else {
        "cyberbrain"
    });
    assert!(
        exe.is_file(),
        "cyberbrain is not built at {}; run `cargo build -p cyberbrain` first",
        exe.display()
    );
    exe
}

#[test]
fn a_typed_line_is_split_the_way_somebody_would_expect() {
    use super::command::tokenise;
    let t = |s: &str| tokenise(s).unwrap().unwrap();
    assert_eq!(t("doctor"), vec!["doctor"]);
    assert_eq!(
        t("  find   discover_store  "),
        vec!["find", "discover_store"]
    );
    assert_eq!(
        t(r#"write --ring 2 --name x --body "two words""#),
        vec!["write", "--ring", "2", "--name", "x", "--body", "two words"]
    );
    assert_eq!(
        t(r#"recall 'a "quoted" thing'"#),
        vec!["recall", r#"a "quoted" thing"#]
    );
    // A backslash stands for itself. This is the platform this feature lives on, and a path
    // is the first thing anybody types: `C:\Users\me\tool.exe` used to become
    // `C:Usersmetool.exe`, and a saved connection passed its own check and never started.
    assert_eq!(
        t(r"ssh -i C:\keys\id_rsa root@host"),
        vec!["ssh", "-i", r"C:\keys\id_rsa", "root@host"]
    );
    assert_eq!(
        t(r#""C:\Program Files\Git\bin\bash.exe""#),
        vec![r"C:\Program Files\Git\bin\bash.exe"]
    );
    // Inside single quotes nothing is special, which is what every shell agrees on.
    assert_eq!(t(r"'C:\tools\x'"), vec![r"C:\tools\x"]);
    // What an escape is still for: a quote inside a quoted argument, and a literal backslash
    // before one.
    assert_eq!(
        t("write --body \"say \\\"hi\\\"\""),
        vec!["write", "--body", r#"say "hi""#]
    );
    assert_eq!(t(r"a\\b"), vec![r"a\b"]);
    // An empty argument is an argument: `--body ""` is a thing somebody means.
    assert_eq!(t("write --body \"\""), vec!["write", "--body", ""]);
    assert!(tokenise("   ").unwrap().is_none());
    assert!(tokenise(r#"recall "unclosed"#).is_err());
    // No longer an error, and that is the point: `C:\` is an ordinary path. Under the old
    // rule a trailing backslash meant "escape the next character" and there was none.
    assert_eq!(
        tokenise(r"export C:\").unwrap().unwrap(),
        vec!["export", r"C:\"]
    );
}

/// Not a shell, and the point is that nothing here is honoured as one: what looks like two
/// commands is one command with odd arguments, which clap will then reject.
#[test]
fn a_semicolon_is_an_argument_not_a_second_command() {
    use super::command::tokenise;
    assert_eq!(
        tokenise("doctor; rm -rf /").unwrap().unwrap(),
        vec!["doctor;", "rm", "-rf", "/"]
    );
    assert_eq!(
        tokenise("doctor && status").unwrap().unwrap(),
        vec!["doctor", "&&", "status"]
    );
}

#[tokio::test]
async fn a_command_runs_and_answers_the_way_a_terminal_would() {
    let fx = Fx::with_cli();
    let (code, v) = fx
        .call(
            Method::POST,
            "/api/v1/command",
            Some(json!({ "line": "doctor" })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{v:#}");
    assert_eq!(v["exit_code"], 0, "{v:#}");
    assert_eq!(v["argv"], json!(["doctor"]));
    assert!(
        v["stdout"].as_str().unwrap().contains("link"),
        "doctor names what it checked: {v:#}"
    );
}

/// The store is the window's. Nothing typed here reaches another one, and a command that
/// fails is still that command's own failure rather than a different exit code of ours.
#[tokio::test]
async fn the_command_runs_against_this_windows_store() {
    let fx = Fx::with_cli();
    fx.create(2, "only-here", "a note that exists in this store alone")
        .await;
    let (_, v) = fx
        .call(
            Method::POST,
            "/api/v1/command",
            Some(json!({ "line": "export only-here" })),
        )
        .await;
    assert_eq!(v["exit_code"], 0, "{v:#}");
    assert!(v["stdout"].as_str().unwrap().contains("only-here"), "{v:#}");
}

#[tokio::test]
async fn a_command_that_fails_reports_its_own_exit_code() {
    let fx = Fx::with_cli();
    let (code, v) = fx
        .call(
            Method::POST,
            "/api/v1/command",
            Some(json!({ "line": "export no-such-note" })),
        )
        .await;
    // The request worked; the command did not. Those are different things and the page has
    // to be able to tell them apart.
    assert_eq!(code, StatusCode::OK, "{v:#}");
    assert_eq!(v["exit_code"], 1, "{v:#}");
    assert!(!v["stderr"].as_str().unwrap().is_empty(), "{v:#}");
}

#[tokio::test]
async fn the_commands_that_do_not_belong_in_a_window_are_refused_with_a_reason() {
    let fx = Fx::new();
    for line in [
        "serve",
        "mcp",
        "hook session-start",
        "init",
        "install",
        "import --plan p.toml",
        "verify-export f.json",
        "hub fleet",
    ] {
        let (code, v) = fx
            .call(
                Method::POST,
                "/api/v1/command",
                Some(json!({ "line": line })),
            )
            .await;
        assert_eq!(
            code,
            StatusCode::BAD_REQUEST,
            "{line} was not refused: {v:#}"
        );
        let message = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            message.len() > 30,
            "{line} was refused without saying why: {v:#}"
        );
    }
}

/// An unauthenticated write to any path on the machine, through a command that was let
/// through because the reasoning only considered reading.
///
/// Reproduced before it was fixed: `{"line":"policy audit --export /tmp/x"}` answered 200
/// and left a file there, over whatever had been there before — including, if pointed at
/// it, this store's own hash-chained audit log.
#[tokio::test]
async fn a_command_may_not_write_a_file_wherever_it_is_pointed() {
    let fx = Fx::with_cli();
    let target = fx.store.join("..").join("written-by-a-request.json");
    let line = format!("policy audit --export {}", target.display());

    let (code, v) = fx
        .call(
            Method::POST,
            "/api/v1/command",
            Some(json!({ "line": line })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST, "{v:#}");
    assert!(
        v["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("--export"),
        "{v:#}"
    );
    assert!(!target.exists(), "a refused command wrote a file anyway");

    // The same command without the path is fine: reading the log is what this is for.
    let (code, v) = fx
        .call(
            Method::POST,
            "/api/v1/command",
            Some(json!({ "line": "policy audit" })),
        )
        .await;
    assert_eq!(code, StatusCode::OK, "{v:#}");
    assert_eq!(v["exit_code"], 0, "{v:#}");
}

/// The one way this endpoint could reach a store the person is not looking at.
#[tokio::test]
async fn naming_another_store_is_refused() {
    let fx = Fx::new();
    for line in [
        "status --store /tmp/elsewhere",
        "status --store=/tmp/elsewhere",
    ] {
        let (code, v) = fx
            .call(
                Method::POST,
                "/api/v1/command",
                Some(json!({ "line": line })),
            )
            .await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{line}: {v:#}");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains("--store"),
            "{v:#}"
        );
    }
}

#[tokio::test]
async fn a_line_that_is_not_a_command_gets_clap_s_own_message() {
    let fx = Fx::new();
    let (code, v) = fx
        .call(
            Method::POST,
            "/api/v1/command",
            Some(json!({ "line": "recal something" })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let message = v["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("recal"), "{v:#}");
}

#[tokio::test]
async fn an_empty_line_is_a_bad_request_not_a_process() {
    let fx = Fx::new();
    let (code, _) = fx
        .call(
            Method::POST,
            "/api/v1/command",
            Some(json!({ "line": "   " })),
        )
        .await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
}
