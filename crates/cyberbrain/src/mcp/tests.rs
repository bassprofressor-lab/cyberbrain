//! The server driven the way a client drives it: framed JSON-RPC over a pipe, against a
//! store `App` built in a temporary directory.
//!
//! Two transports. Most tests run [`serve`] over an in-memory duplex pipe, which is the
//! server loop end to end but *not* the process's stdout — so a stray `println!` in a
//! handler is invisible to them. The one test that can see it spawns this test binary as a
//! child in server mode and talks to its real stdout over a real pipe
//! ([`stdout_carries_protocol_only_over_real_pipes`]). SPEC §14.2: that test was watched
//! failing against a deliberately planted `println!` before it was trusted.

use super::*;
use crate::app::App;
use cyberbrain_policy::Actor;
use serde_json::{Value, json};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{
    AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf,
};

const SENTINEL: &str = "CYBERBRAIN-MCP-PROTOCOL-BEGINS";
const CHILD_ENV: &str = "CYBERBRAIN_MCP_TEST_CHILD_STORE";

fn temp_store() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("store");
    App::init(&root, &Actor::Operator).unwrap();
    (dir, root)
}

fn temp_app() -> (tempfile::TempDir, PathBuf, Arc<App>) {
    let (dir, root) = temp_store();
    let app = App::open(Some(&root), Actor::Mcp).unwrap();
    (dir, root, Arc::new(app))
}

struct Client {
    w: WriteHalf<DuplexStream>,
    r: BufReader<ReadHalf<DuplexStream>>,
    next_id: u64,
    /// Everything the server put on the wire, line by line, for the purity check.
    wire: Vec<String>,
}

impl Client {
    async fn send_raw(&mut self, bytes: &[u8]) {
        self.w.write_all(bytes).await.unwrap();
        self.w.flush().await.unwrap();
    }

    async fn read_line(&mut self) -> String {
        let mut s = String::new();
        let n = self.r.read_line(&mut s).await.unwrap();
        assert!(
            n > 0,
            "server closed the stream while a response was expected"
        );
        self.wire.push(s.clone());
        s
    }

    async fn read_json_line(&mut self) -> Value {
        let line = self.read_line().await;
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON: {e}: {line:?}"))
    }

    async fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.send_raw(format!("{msg}\n").as_bytes()).await;
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.send_raw(format!("{msg}\n").as_bytes()).await;
        let v = self.read_json_line().await;
        assert_eq!(v["id"], id, "response id: {v}");
        v
    }

    async fn init(&mut self) -> Value {
        self.init_with(LATEST_PROTOCOL).await
    }

    async fn init_with(&mut self, protocol: &str) -> Value {
        let v = self
            .request(
                "initialize",
                json!({
                    "protocolVersion": protocol,
                    "capabilities": {},
                    "clientInfo": { "name": "cyberbrain-test", "version": "0" },
                }),
            )
            .await;
        assert!(v.get("error").is_none(), "{v}");
        self.notify("notifications/initialized", json!({})).await;
        v["result"].clone()
    }

    /// A successful `tools/call`: the *result* (which may still carry `isError`).
    async fn call(&mut self, tool: &str, args: Value) -> Value {
        let v = self
            .request("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        assert!(v.get("error").is_none(), "tools/call {tool}: {v}");
        v["result"].clone()
    }

    /// A `tools/call` that must be refused at the protocol level: the *error*.
    async fn call_err(&mut self, tool: &str, args: Value) -> Value {
        let v = self
            .request("tools/call", json!({ "name": tool, "arguments": args }))
            .await;
        assert!(v.get("result").is_none(), "tools/call {tool}: {v}");
        v["error"].clone()
    }
}

/// Run `f` as the client of a fresh session over a duplex pipe; return the wire lines.
/// Every line the server wrote must be one JSON-RPC message — checked here for every
/// test, so no test can pass while the stream is dirty (within what this transport sees).
async fn session<F, Fut>(app: Arc<App>, f: F) -> Vec<String>
where
    F: FnOnce(Client) -> Fut,
    Fut: std::future::Future<Output = Client>,
{
    let (a, b) = tokio::io::duplex(1 << 16);
    let (sr, sw) = tokio::io::split(a);
    let (cr, cw) = tokio::io::split(b);
    let client = Client {
        w: cw,
        r: BufReader::new(cr),
        next_id: 1,
        wire: Vec::new(),
    };
    let server = serve(app, BufReader::new(sr), sw);
    let driver = async move {
        let mut c = f(client).await;
        // After `shutdown` the server has already dropped its end; that is not a failure.
        let _ = c.w.shutdown().await;
        c.wire
    };
    let (served, wire) = tokio::join!(server, driver);
    served.expect("the server loop returned an error");
    for line in &wire {
        let v: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("a non-JSON line reached the wire: {e}: {line:?}"));
        // One message, or a batch of them; nothing else.
        let messages: Vec<&Value> = match &v {
            Value::Array(items) => items.iter().collect(),
            single => vec![single],
        };
        assert!(!messages.is_empty(), "empty batch on the wire: {line}");
        for m in messages {
            assert_eq!(m["jsonrpc"], "2.0", "not a JSON-RPC message: {line}");
        }
    }
    wire
}

fn text_of(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn initialize_negotiates_and_degrades_clearly() {
    let (_d, _root, app) = temp_app();
    session(app.clone(), |mut c| async move {
        let r = c.init().await;
        assert_eq!(r["protocolVersion"], LATEST_PROTOCOL);
        assert_eq!(r["serverInfo"]["name"], "cyberbrain");
        assert_eq!(r["capabilities"]["tools"]["listChanged"], false);
        assert!(
            r["instructions"].as_str().unwrap().contains("caveats"),
            "{r}"
        );
        assert!(
            !r["instructions"]
                .as_str()
                .unwrap()
                .contains("protocol note")
        );
        c
    })
    .await;

    // An older revision we speak: answered in kind.
    session(app.clone(), |mut c| async move {
        let r = c.init_with("2024-11-05").await;
        assert_eq!(r["protocolVersion"], "2024-11-05");
        c
    })
    .await;

    // A revision we do not speak: answered with ours, and the degradation is stated
    // where the client's model will see it.
    session(app, |mut c| async move {
        let r = c.init_with("1999-01-01").await;
        assert_eq!(r["protocolVersion"], LATEST_PROTOCOL);
        let i = r["instructions"].as_str().unwrap();
        assert!(i.contains("protocol note"), "{i}");
        assert!(i.contains("1999-01-01"), "{i}");
        let p = c.request("ping", json!({})).await;
        assert_eq!(p["result"], json!({}));
        c
    })
    .await;
}

#[tokio::test]
async fn tools_list_offers_the_five_tools_with_schemas() {
    let (_d, _root, app) = temp_app();
    session(app, |mut c| async move {
        c.init().await;
        let r = c.request("tools/list", json!({})).await;
        let tools = r["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, tools::TOOL_NAMES);
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "{t}");
            assert!(!t["description"].as_str().unwrap().is_empty(), "{t}");
            assert!(t["annotations"]["readOnlyHint"].is_boolean());
        }
        let write = tools.iter().find(|t| t["name"] == "write").unwrap();
        assert_eq!(
            write["inputSchema"]["required"],
            json!(["ring", "kind", "name", "body"])
        );
        assert_eq!(write["inputSchema"]["properties"]["ring"]["maximum"], 4);
        assert_eq!(write["annotations"]["readOnlyHint"], false);
        c
    })
    .await;
}

/// SPEC §9.2: descriptions come from the CLI help, not from a second copy. The first
/// paragraph of every tool description is byte-for-byte the clap `about` (or, for
/// `recall_id`, the help of `recall --id`).
#[test]
fn tool_descriptions_are_the_cli_help_verbatim() {
    use clap::CommandFactory;
    let cli = crate::cli::Cli::command();
    for t in tools::catalogue() {
        let first = t.description.split("\n\n").next().unwrap();
        let expected = match t.name {
            "recall_id" => cli
                .find_subcommand("recall")
                .unwrap()
                .get_arguments()
                .find(|a| a.get_id().as_str() == "id")
                .unwrap()
                .get_help()
                .unwrap()
                .to_string(),
            name => cli
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("tool {name} has no CLI subcommand of that name"))
                .get_about()
                .unwrap()
                .to_string(),
        };
        assert_eq!(first, expected, "tool {} drifted from the CLI help", t.name);
        assert!(!first.starts_with("(no description"), "{first}");
    }
}

#[tokio::test]
async fn recall_results_carry_their_caveats_verbatim() {
    let (_d, _root, app) = temp_app();
    session(app, |mut c| async move {
        c.init().await;
        for (name, body) in [
            (
                "redis-limits",
                "Redis maxmemory must be set or the box swaps.",
            ),
            (
                "redis-persist",
                "Redis persistence: RDB snapshots and maxmemory interplay.",
            ),
        ] {
            let w = c
                .call(
                    "write",
                    json!({ "ring": 2, "kind": "knowledge", "name": name, "body": body }),
                )
                .await;
            assert_eq!(w["isError"], false, "{w}");
            assert_eq!(w["structuredContent"]["outcome"], "written", "{w}");
        }

        let r = c
            .call("recall", json!({ "query": "redis maxmemory" }))
            .await;
        assert_eq!(r["isError"], false, "{r}");
        let s = &r["structuredContent"];
        assert_eq!(s["hits"].as_array().unwrap().len(), 2, "{r}");
        let cit = s["hits"][0]["citation"].as_str().unwrap();
        assert!(cit.starts_with("r2-") && cit.len() == 15, "{cit}");

        // The caveats: present in the structured result, verbatim in the text, and they
        // say the contradiction check did not run.
        let caveats: Vec<&str> = s["caveats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(!caveats.is_empty(), "{r}");
        assert!(
            caveats
                .iter()
                .any(|c| c.starts_with("contradiction check skipped:")),
            "{caveats:?}"
        );
        assert!(
            caveats.iter().any(|c| c.contains("lexical only")),
            "{caveats:?}"
        );
        let text = text_of(&r);
        for cav in &caveats {
            assert!(
                text.contains(&format!("caveat: {cav}")),
                "caveat missing from the text block: {cav}\n{text}"
            );
        }

        // Ring filter and n are honoured.
        let r = c
            .call("recall", json!({ "query": "redis", "ring": 3 }))
            .await;
        assert!(
            r["structuredContent"]["hits"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let r = c.call("recall", json!({ "query": "redis", "n": 1 })).await;
        assert_eq!(r["structuredContent"]["hits"].as_array().unwrap().len(), 1);

        // recall_id expands the citation to block and note.
        let e = c.call("recall_id", json!({ "citation": cit })).await;
        assert_eq!(e["isError"], false, "{e}");
        assert_eq!(e["structuredContent"]["block"]["citation"], cit);
        assert!(
            e["structuredContent"]["note"]["body"]
                .as_str()
                .unwrap()
                .contains("maxmemory")
        );
        assert!(text_of(&e).contains("--- full note ---"));
        c
    })
    .await;
}

/// An operation that ran and declined is a tool result carrying the §8.1 error shape,
/// never a JSON-RPC error: the agent has to read it.
#[tokio::test]
async fn app_errors_are_tool_results_with_the_shared_taxonomy() {
    let (_d, _root, app) = temp_app();
    session(app, |mut c| async move {
        c.init().await;
        let r = c
            .call("recall_id", json!({ "citation": "not-a-citation" }))
            .await;
        assert_eq!(r["isError"], true, "{r}");
        assert_eq!(r["structuredContent"]["error"]["code"], "bad-citation");
        assert_eq!(r["structuredContent"]["error"]["exit_code"], 1);
        assert!(text_of(&r).starts_with("error: "), "{r}");

        let r = c
            .call("recall_id", json!({ "citation": "r2-a91f2c33e1bd" }))
            .await;
        assert_eq!(r["isError"], true);
        assert_eq!(r["structuredContent"]["error"]["code"], "no-such-note");

        let big = "word ".repeat(9000);
        let r = c
            .call(
                "write",
                json!({ "ring": 0, "kind": "decision", "name": "huge", "body": big }),
            )
            .await;
        assert_eq!(r["isError"], true);
        assert_eq!(r["structuredContent"]["error"]["code"], "ring-cap-exceeded");

        let r = c
            .call(
                "write",
                json!({ "ring": 2, "kind": "knowledge", "name": "Bad Name", "body": "x" }),
            )
            .await;
        assert_eq!(r["isError"], true);
        assert_eq!(r["structuredContent"]["error"]["code"], "frontmatter");
        c
    })
    .await;
}

/// SPEC §12.4 over MCP: a held write comes back as a result with the findings, nothing on
/// disk, `isError` set; answering with `choice` completes it.
#[tokio::test]
async fn a_held_write_is_a_result_carrying_its_findings() {
    let (_d, root, app) = temp_app();
    let note = root.join("notes/r3/contact.md");
    session(app, |mut c| async move {
        c.init().await;
        let args = json!({
            "ring": 3, "kind": "session", "name": "contact",
            "body": "reach bob@corp.example.org about the pager",
        });
        let r = c.call("write", args.clone()).await;
        assert_eq!(r["isError"], true, "{r}");
        let s = &r["structuredContent"];
        assert_eq!(s["outcome"], "held", "{r}");
        assert_eq!(s["name"], "contact");
        assert_eq!(s["findings"][0]["kind"], "email", "{r}");
        assert!(s["findings"][0]["start"].is_number());
        let text = text_of(&r);
        assert!(text.contains("Nothing was written"), "{text}");
        for opt in ["redact", "mark-reviewed", "proceed-flagged", "force"] {
            assert!(text.contains(opt), "the text must offer {opt}: {text}");
        }
        assert!(!note.exists(), "a held write writes nothing");

        // The agent decides: redact.
        let mut with_choice = args.clone();
        with_choice["choice"] = json!("redact");
        let r = c.call("write", with_choice).await;
        assert_eq!(r["isError"], false, "{r}");
        assert_eq!(r["structuredContent"]["outcome"], "written");
        assert_eq!(r["structuredContent"]["redacted"], 1, "{r}");
        assert!(note.exists());
        let on_disk = std::fs::read_to_string(&note).unwrap();
        assert!(!on_disk.contains("bob@"), "{on_disk}");

        // Or: force, which writes it flagged.
        let mut forced = args.clone();
        forced["name"] = json!("contact-two");
        forced["force"] = json!(true);
        let r = c.call("write", forced).await;
        assert_eq!(r["structuredContent"]["outcome"], "written");
        assert_eq!(r["structuredContent"]["pii"], "flagged", "{r}");

        // Concurrency guard (§8.1): a stale expected_updated is a conflict result.
        let mut stale = args.clone();
        stale["choice"] = json!("mark-reviewed");
        stale["expected_updated"] = json!("2020-01-01T00:00:00Z");
        let r = c.call("write", stale).await;
        assert_eq!(r["isError"], true, "{r}");
        assert_eq!(r["structuredContent"]["outcome"], "conflict");

        // A dry run takes the real path and writes nothing.
        let r = c
            .call(
                "write",
                json!({ "ring": 2, "kind": "lesson", "name": "dry", "body": "beta", "dry_run": true }),
            )
            .await;
        assert_eq!(r["structuredContent"]["outcome"], "written");
        assert_eq!(r["structuredContent"]["dry_run"], true);
        assert!(!root.join("notes/r2/dry.md").exists());
        c
    })
    .await;
}

#[tokio::test]
async fn status_answers_with_the_cli_report() {
    let (_d, root, app) = temp_app();
    session(app, |mut c| async move {
        c.init().await;
        let r = c.call("status", json!({})).await;
        assert_eq!(r["isError"], false, "{r}");
        // The report renders paths with forward slashes on every platform, so the
        // expectation is the slashed form and not the native one. Comparing against
        // `root.to_str()` passes on Linux, where the two are identical, and fails on
        // Windows — which is the whole reason the rendering was made consistent.
        assert_eq!(
            r["structuredContent"]["store"].as_str().unwrap(),
            cyberbrain_core::slash(&root)
        );
        assert!(r["structuredContent"]["policy"]["profile"].is_string());
        assert!(text_of(&r).starts_with("store: "));
        // No arguments means no arguments.
        let e = c.call_err("status", json!({ "verbose": true })).await;
        assert_eq!(e["code"], jsonrpc::INVALID_PARAMS);
        c
    })
    .await;
}

#[tokio::test]
/// Accepting either answer was right while `App::find` did not exist. It does now, so the
/// test pins the working behaviour — otherwise a refusal that outlives the missing feature
/// keeps passing, which is exactly how `find` stayed stubbed over MCP for hours after the
/// code index landed and worked from the CLI.
async fn find_answers_with_a_report() {
    let (_d, _root, app) = temp_app();
    session(app, |mut c| async move {
        c.init().await;
        let r = c.call("find", json!({ "symbol": "serve_stdio" })).await;
        assert_eq!(r["isError"], false, "find must answer, not refuse: {r}");
        let sc = &r["structuredContent"];
        assert!(sc["hits"].is_array(), "{r}");
        assert!(sc["files_scanned"].is_number(), "{r}");
        // The counts name which side of the boundary they count (SPEC §14.3).
        assert!(sc["skipped"].is_object(), "{r}");
        c
    })
    .await;
}

#[tokio::test]
async fn protocol_errors_are_jsonrpc_errors() {
    let (_d, _root, app) = temp_app();
    session(app, |mut c| async move {
        c.init().await;
        let v = c.request("resources/list", json!({})).await;
        assert_eq!(v["error"]["code"], jsonrpc::METHOD_NOT_FOUND, "{v}");
        assert!(
            v["error"]["message"]
                .as_str()
                .unwrap()
                .contains("resources/list")
        );

        let e = c.call_err("nope", json!({})).await;
        assert_eq!(e["code"], jsonrpc::INVALID_PARAMS);
        assert!(
            e["message"].as_str().unwrap().contains("unknown tool"),
            "{e}"
        );
        assert!(e["message"].as_str().unwrap().contains("recall_id"));

        let e = c.call_err("recall", json!({})).await;
        assert!(
            e["message"]
                .as_str()
                .unwrap()
                .contains("`query` is required"),
            "{e}"
        );
        let e = c
            .call_err("recall", json!({ "query": "x", "rng": 2 }))
            .await;
        assert!(
            e["message"]
                .as_str()
                .unwrap()
                .contains("unknown argument `rng`"),
            "{e}"
        );
        let e = c
            .call_err("recall", json!({ "query": "x", "ring": 9 }))
            .await;
        assert!(e["message"].as_str().unwrap().contains("ring"), "{e}");
        let e = c.call_err("recall", json!({ "query": "x", "n": 0 })).await;
        assert!(e["message"].as_str().unwrap().contains(">= 1"), "{e}");
        let e = c
            .call_err(
                "write",
                json!({ "ring": 2, "kind": "poem", "name": "a", "body": "b" }),
            )
            .await;
        assert!(
            e["message"].as_str().unwrap().contains("knowledge, bug"),
            "{e}"
        );
        let e = c.call_err("recall", json!("not an object")).await;
        assert!(
            e["message"].as_str().unwrap().contains("must be an object"),
            "{e}"
        );

        // tools/call without a name.
        let v = c.request("tools/call", json!({ "arguments": {} })).await;
        assert_eq!(v["error"]["code"], jsonrpc::INVALID_PARAMS);
        c
    })
    .await;
}

#[tokio::test]
async fn a_malformed_frame_gets_an_error_and_the_server_lives() {
    let (_d, _root, app) = temp_app();
    session(app, |mut c| async move {
        c.init().await;
        c.send_raw(b"this is not json\n").await;
        let v = c.read_json_line().await;
        assert_eq!(v["error"]["code"], jsonrpc::PARSE_ERROR, "{v}");
        assert_eq!(v["id"], Value::Null);

        c.send_raw(b"{\"jsonrpc\":\"2.0\",\"id\":77}\n").await;
        let v = c.read_json_line().await;
        assert_eq!(v["error"]["code"], jsonrpc::INVALID_REQUEST, "{v}");
        assert_eq!(v["id"], 77);

        c.send_raw(b"{\"jsonrpc\":\"1.0\",\"id\":78,\"method\":\"ping\"}\n")
            .await;
        let v = c.read_json_line().await;
        assert_eq!(v["error"]["code"], jsonrpc::INVALID_REQUEST, "{v}");

        c.send_raw(b"[1, 2]\n").await;
        let v = c.read_json_line().await;
        let arr = v.as_array().expect("a batch is answered with a batch");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["error"]["code"], jsonrpc::INVALID_REQUEST);

        c.send_raw(b"[]\n").await;
        let v = c.read_json_line().await;
        assert_eq!(v["error"]["code"], jsonrpc::INVALID_REQUEST);

        // Blank lines are not messages.
        c.send_raw(b"\n\n   \n").await;
        // Unknown notifications are ignored, not answered.
        c.notify("notifications/whatever", json!({})).await;

        // Still alive, still correct.
        let p = c.request("ping", json!({})).await;
        assert_eq!(p["result"], json!({}));
        let batch = json!([
            { "jsonrpc": "2.0", "id": "a", "method": "ping" },
            { "jsonrpc": "2.0", "method": "notifications/initialized" },
            { "jsonrpc": "2.0", "id": "b", "method": "ping" },
        ]);
        c.send_raw(format!("{batch}\n").as_bytes()).await;
        let v = c.read_json_line().await;
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2, "notifications get no answer: {v}");
        assert_eq!(arr[0]["id"], "a");
        assert_eq!(arr[1]["id"], "b");
        c
    })
    .await;
}

/// The batch answered above is one line holding an array; the purity check in `session`
/// accepts it only because it is JSON. Pin that an array line is still a JSON-RPC batch.
#[tokio::test]
async fn content_length_framing_is_answered_in_kind() {
    let (_d, _root, app) = temp_app();
    session(app, |mut c| async move {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }).to_string();
        c.send_raw(
            format!(
                "Content-Length: {}\r\nX-Other: header\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await;
        let header = c.read_line().await;
        assert!(header.starts_with("Content-Length: "), "{header:?}");
        let len: usize = header.trim()["Content-Length: ".len()..].parse().unwrap();
        let blank = c.read_line().await;
        assert_eq!(blank, "\r\n");
        let mut buf = vec![0u8; len];
        c.r.read_exact(&mut buf).await.unwrap();
        let v: Value = serde_json::from_slice(&buf).unwrap();
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"], json!({}));
        // The purity check reads line by line; the header lines are not JSON. Pull them
        // out of the record and put the body in, which is what the client parsed.
        c.wire.clear();
        c.wire.push(String::from_utf8(buf).unwrap());
        c
    })
    .await;
}

#[tokio::test]
async fn shutdown_ends_the_session_after_the_answer() {
    let (_d, _root, app) = temp_app();
    let wire = session(app, |mut c| async move {
        c.init().await;
        let v = c.request("shutdown", json!({})).await;
        assert_eq!(v["result"], Value::Null);
        // The server has left; anything further gets no answer. The write itself may
        // already fail with a broken pipe, which is the same fact seen from the other side.
        let _ =
            c.w.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"ping\"}\n")
                .await;
        let mut s = String::new();
        let n = c.r.read_line(&mut s).await.unwrap();
        assert_eq!(n, 0, "the server must have closed the stream: {s:?}");
        c
    })
    .await;
    assert_eq!(wire.len(), 2, "initialize + shutdown responses: {wire:?}");
}

// ---------------------------------------------------------------------------------------
// Real pipes. The only place a stray `println!` can be seen.

/// Not a test of anything by itself: when spawned with [`CHILD_ENV`] set, this process
/// becomes the MCP server on its real stdin and stdout, through [`serve_stdio`] — the
/// production path. Without the variable (a normal `cargo test`) it does nothing.
#[test]
fn child_entry_serves_stdio_when_spawned_as_a_child() {
    let Ok(store) = std::env::var(CHILD_ENV) else {
        return;
    };
    let app = App::open(Some(Path::new(&store)), Actor::Mcp).unwrap();
    // The test harness has already printed its preamble on stdout. Mark where the
    // protocol starts; everything after this line must be JSON-RPC.
    {
        let mut out = std::io::stdout().lock();
        out.write_all(format!("{SENTINEL}\n").as_bytes()).unwrap();
        out.flush().unwrap();
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(serve_stdio(Arc::new(app))).unwrap();
}

#[test]
fn stdout_carries_protocol_only_over_real_pipes() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    let (_d, root) = temp_store();
    {
        let app = App::open(Some(&root), Actor::Operator).unwrap();
        let w = app
            .write(crate::app::WriteRequest {
                ring: cyberbrain_core::Ring::Knowledge,
                kind: cyberbrain_core::NoteKind::Knowledge,
                name: "pipe-note".into(),
                body: "Postgres 18 moved pgdata.".into(),
                tags: vec![],
                retention: None,
                force: false,
                choice: None,
                expected_updated: None,
                dry_run: false,
            })
            .unwrap();
        assert!(matches!(w, crate::app::WriteOutcome::Written(_)));
    }

    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "mcp::tests::child_entry_serves_stdio_when_spawned_as_a_child",
            "--exact",
            "--nocapture",
            "--quiet",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, &root)
        .env_remove("CYBERBRAIN_STORE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // Skip the harness preamble up to the sentinel.
    loop {
        let mut line = String::new();
        assert!(
            stdout.read_line(&mut line).unwrap() > 0,
            "child ended before the sentinel"
        );
        if line.trim() == SENTINEL {
            break;
        }
    }

    let mut exchange = |id: u64, method: &str, params: Value| -> Value {
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        stdin.write_all(format!("{msg}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        assert!(
            stdout.read_line(&mut line).unwrap() > 0,
            "no answer to {method}"
        );
        let v: Value = serde_json::from_str(&line).unwrap_or_else(|e| {
            panic!("stdout carried something that is not JSON-RPC: {e}: {line:?}")
        });
        assert_eq!(v["jsonrpc"], "2.0", "{line}");
        assert_eq!(v["id"], id, "{line}");
        v
    };

    let init = exchange(
        1,
        "initialize",
        json!({ "protocolVersion": LATEST_PROTOCOL, "capabilities": {}, "clientInfo": { "name": "pipe-test", "version": "0" } }),
    );
    assert_eq!(init["result"]["protocolVersion"], LATEST_PROTOCOL);
    let list = exchange(2, "tools/list", json!({}));
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 5);
    let st = exchange(
        3,
        "tools/call",
        json!({ "name": "status", "arguments": {} }),
    );
    assert_eq!(st["result"]["isError"], false, "{st}");
    let rc = exchange(
        4,
        "tools/call",
        json!({ "name": "recall", "arguments": { "query": "postgres pgdata" } }),
    );
    assert_eq!(
        rc["result"]["structuredContent"]["hits"][0]["note_name"], "pipe-note",
        "{rc}"
    );
    assert!(
        !rc["result"]["structuredContent"]["caveats"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let held = exchange(
        5,
        "tools/call",
        json!({ "name": "write", "arguments": { "ring": 3, "kind": "session", "name": "who", "body": "mail alice@example.org" } }),
    );
    assert_eq!(held["result"]["isError"], true, "{held}");
    assert_eq!(held["result"]["structuredContent"]["outcome"], "held");
    let bye = exchange(6, "shutdown", json!({}));
    assert_eq!(bye["result"], Value::Null);
    drop(stdin);

    let status = child.wait().unwrap();
    let mut err = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut err).unwrap();
    assert!(status.success(), "child failed: {err}");
    assert!(err.contains("cyberbrain mcp: serving on stdio"), "{err}");
}
