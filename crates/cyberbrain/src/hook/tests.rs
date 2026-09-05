//! Driven through `run` / `run_with` with realistic payloads. Every case in here must come
//! back with exit code 0, whatever else it does.

use super::events::FAIL_NEXT;
use super::*;
use crate::app::{App, WriteRequest};
use cyberbrain_core::{NoteKind, Ring};
use cyberbrain_policy::{Actor, AuditFilter};
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

// ---------------------------------------------------------------------------------------
// Fixtures

/// Holds the process lock for the life of the test.
///
/// These tests manipulate state that belongs to the process and not to a test: the working
/// directory, environment variables, the panic hook, and a fault injector. Chasing each of
/// those races separately kept producing a different failing test every few runs, always
/// green when run alone. They simply must not run beside each other, and the fixture every
/// hook test already builds is the one place that cannot be forgotten. 86 tests in 0.3 s;
/// serialising them costs nothing worth measuring.
struct Fixture {
    _dir: TempDir,
    store: std::path::PathBuf,
    _lock: ProcessLock,
}

fn fixture() -> Fixture {
    let _lock = process_lock();
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".cyberbrain");
    App::init(&store, &Actor::Operator).unwrap();
    Fixture {
        _dir: dir,
        store,
        _lock,
    }
}

impl Fixture {
    fn open(&self) -> App {
        App::open(Some(&self.store), Actor::Hook("test".into())).unwrap()
    }

    fn write(&self, ring: Ring, name: &str, body: &str) {
        let app = self.open();
        let out = app
            .write(WriteRequest {
                ring,
                kind: NoteKind::Knowledge,
                name: name.into(),
                body: body.into(),
                tags: vec![],
                retention: None,
                force: true,
                choice: None,
                expected_updated: None,
                dry_run: false,
            })
            .unwrap();
        assert!(
            matches!(out, crate::app::WriteOutcome::Written(_)),
            "{out:?}"
        );
    }

    fn payload(&self, extra: serde_json::Value) -> String {
        let mut v = serde_json::json!({
            "session_id": "sess-0123",
            "transcript_path": "/tmp/t.jsonl",
            "cwd": self.store.parent().unwrap().to_string_lossy(),
            "permission_mode": "default",
        });
        if let (Some(a), Some(b)) = (v.as_object_mut(), extra.as_object()) {
            for (k, val) in b {
                a.insert(k.clone(), val.clone());
            }
        }
        v.to_string()
    }

    fn state(&self) -> Option<session::SessionState> {
        session::load(&self.store, "sess-0123").unwrap()
    }
}

fn session_start(f: &Fixture, source: &str) -> HookOutput {
    let app = f.open();
    run(
        Some(&app),
        HookEvent::SessionStart,
        &f.payload(serde_json::json!({ "hook_event_name": "SessionStart", "source": source })),
    )
}

fn tool_payload(f: &Fixture, tool: &str, path: &Path) -> String {
    f.payload(serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": { "file_path": path.to_string_lossy(), "old_string": "a", "new_string": "b" },
        "tool_use_id": "toolu_01",
    }))
}

fn audit_actions(app: &App) -> Vec<String> {
    app.policy()
        .audit()
        .read(&AuditFilter::default())
        .unwrap()
        .into_iter()
        .map(|e| e.action)
        .collect()
}

// ---------------------------------------------------------------------------------------
// session-start

#[test]
fn session_start_injects_rings_0_and_1_with_citations_and_a_digest() {
    let f = fixture();
    f.write(
        Ring::Invariant,
        "never-copy-config",
        "Never copy config.py to S2.\n\nS2 has its own.",
    );
    f.write(
        Ring::Protocol,
        "handoff",
        "Read the handoff before resuming.",
    );
    f.write(Ring::Knowledge, "pg18", "PG 18 moves PGDATA.");
    let out = session_start(&f, "startup");
    assert_eq!(out.exit_code, 0);
    let s = &out.stdout;
    assert!(s.contains("## Ring 0"), "{s}");
    assert!(s.contains("### r0 `never-copy-config`"), "{s}");
    assert!(s.contains("Never copy config.py to S2."), "{s}");
    assert!(s.contains("### r1 `handoff`"), "{s}");
    // Ring 2 is on demand, never injected.
    assert!(!s.contains("PG 18 moves PGDATA"), "{s}");
    // Citations, computed by the same function the index uses.
    let cit = s
        .lines()
        .find(|l| l.starts_with("[r0-"))
        .expect("a ring 0 citation");
    assert_eq!(cit.len(), "[r0-".len() + 12 + 1, "{cit}");
    let app = f.open();
    let citation = cit.trim_matches(['[', ']']);
    let expanded = app.recall_id(citation).unwrap();
    assert_eq!(expanded.note.front.name, "never-copy-config");
    // Digest and usage.
    assert!(s.contains("## Store digest"), "{s}");
    assert!(s.contains("resident: 2 note(s)"), "{s}");
    assert!(s.contains("policy profile: eu"), "{s}");
    assert!(s.contains("cyberbrain recall"), "{s}");
    // The record.
    assert!(audit_actions(&app).contains(&"session.start".to_string()));
    // The state.
    let st = f.state().unwrap();
    assert_eq!(st.injections, 1);
    assert_eq!(st.resident.len(), 2);
    assert!(st.resident.contains_key("r0/never-copy-config"));
}

/// What the hook injects is read by an agent and quoted back by it; the store path in it
/// is spelt with forward slashes on every platform, like every other rendered path.
#[test]
fn session_start_names_the_store_with_forward_slashes() {
    let f = fixture();
    let out = session_start(&f, "startup");
    assert_eq!(out.exit_code, 0);
    let line = format!("Store: `{}`.", cyberbrain_core::slash(f.open().root()));
    assert!(out.stdout.contains(&line), "{}", out.stdout);
    assert!(!out.stdout.contains('\\'), "{}", out.stdout);
}

#[test]
fn session_start_on_an_empty_store_says_the_rings_are_empty() {
    let f = fixture();
    let out = session_start(&f, "startup");
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("(ring 0 is empty)"), "{}", out.stdout);
    assert!(out.stdout.contains("(ring 1 is empty)"), "{}", out.stdout);
    assert!(
        out.stdout.contains("cyberbrain.db last written"),
        "{}",
        out.stdout
    );
}

#[test]
fn an_unreadable_resident_note_is_reported_not_dropped() {
    let f = fixture();
    f.write(Ring::Invariant, "good", "fine");
    std::fs::write(
        f.store.join("notes/r0/broken.md"),
        "---\nid: not-a-ulid\n---\nbody",
    )
    .unwrap();
    let out = session_start(&f, "startup");
    assert!(out.stdout.contains("### r0 `good`"), "{}", out.stdout);
    assert!(
        out.stdout.contains("could NOT be injected") && out.stdout.contains("broken.md"),
        "{}",
        out.stdout
    );
}

#[test]
fn session_start_after_compaction_says_so_and_counts() {
    let f = fixture();
    f.write(Ring::Invariant, "rule", "the rule");
    session_start(&f, "startup");
    let app = f.open();
    for _ in 0..3 {
        run(
            Some(&app),
            HookEvent::Stop,
            &f.payload(serde_json::json!({"hook_event_name": "Stop"})),
        );
    }
    let pc = run(
        Some(&app),
        HookEvent::PreCompact,
        &f.payload(serde_json::json!({"hook_event_name": "PreCompact", "trigger": "auto"})),
    );
    assert_eq!(pc.exit_code, 0);
    let v: serde_json::Value = serde_json::from_str(&pc.stdout).unwrap();
    assert!(
        v["systemMessage"]
            .as_str()
            .unwrap()
            .contains("compaction #1 (auto)")
    );
    let out = session_start(&f, "compact");
    assert!(
        out.stdout
            .contains("compaction #1 of this session, 3 turn(s)"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("### r0 `rule`"));
    let st = f.state().unwrap();
    assert_eq!(st.injections, 2);
    assert_eq!(st.compactions, 1);
    assert_eq!(st.turns, 3);
}

// ---------------------------------------------------------------------------------------
// Standing down, never silently

#[test]
fn no_store_is_announced_on_session_start_and_logged_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let _cwd = CwdGuard::enter(dir.path());
    let out = run(None, HookEvent::SessionStart, "{}");
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("standing down"), "{}", out.stdout);
    assert!(out.stdout.contains("cyberbrain init"), "{}", out.stdout);
    for ev in [
        HookEvent::UserPromptSubmit,
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::Stop,
        HookEvent::PreCompact,
    ] {
        let out = run(None, ev, "{}");
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty(), "{ev:?}: {}", out.stdout);
        assert!(
            out.stderr.contains("standing down"),
            "{ev:?}: {}",
            out.stderr
        );
    }
}

#[test]
fn an_unreadable_store_is_announced_with_the_reason() {
    let f = fixture();
    // A store whose audit log is not a database.
    std::fs::write(f.store.join("audit.db"), "this is not sqlite").unwrap();
    let err = match App::open(Some(&f.store), Actor::Hook("test".into())) {
        Err(e) => e,
        Ok(_) => panic!("a store with a text file for audit.db opened"),
    };
    let out = run_with(None, Some(&err), HookEvent::SessionStart, "{}");
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("could not be opened"), "{}", out.stdout);
    assert!(out.stdout.contains("cyberbrain doctor"), "{}", out.stdout);
    // Without the error, `run` still classifies it as unreadable rather than absent.
    let _cwd = CwdGuard::enter(f.store.parent().unwrap());
    let out = run(None, HookEvent::SessionStart, "{}");
    assert!(out.stdout.contains("could not be opened"), "{}", out.stdout);
}

#[test]
fn disabled_by_environment_is_announced() {
    let f = fixture();
    let app = f.open();
    let _env = EnvGuard::set(DISABLE_ENV, "1");
    let out = run(
        Some(&app),
        HookEvent::SessionStart,
        &f.payload(serde_json::json!({})),
    );
    assert!(
        out.stdout.contains("standing down for this session"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains(DISABLE_ENV), "{}", out.stdout);
    assert!(f.state().is_none(), "nothing recorded while disabled");
    let out = run(
        Some(&app),
        HookEvent::Stop,
        &f.payload(serde_json::json!({})),
    );
    assert!(out.stdout.is_empty());
    assert!(f.state().is_none());
}

#[test]
fn the_disable_switch_ignores_off_values() {
    for v in ["", "0", "false", "no", "OFF"] {
        let _env = EnvGuard::set(DISABLE_ENV, v);
        assert!(disabled_by_env().is_none(), "{v:?}");
    }
    let _env = EnvGuard::set(DISABLE_ENV, "yes");
    assert!(disabled_by_env().is_some());
}

// ---------------------------------------------------------------------------------------
// Hostile or odd stdin

#[test]
fn malformed_empty_and_over_full_payloads_all_exit_zero() {
    let f = fixture();
    f.write(Ring::Invariant, "rule", "the rule");
    let app = f.open();
    let cases: [(&str, &str); 6] = [
        ("empty", ""),
        ("whitespace", "  \r\n "),
        ("not json", "{ this is not json"),
        ("array", "[1, 2, 3]"),
        ("string", "\"hello\""),
        (
            "extra fields",
            r#"{"session_id":"s","cwd":"/","hook_event_name":"SessionStart","source":"startup","model":"x","brand_new_field":{"deep":[1,2]},"tool_input":42}"#,
        ),
    ];
    for ev in [
        HookEvent::SessionStart,
        HookEvent::UserPromptSubmit,
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::Stop,
        HookEvent::PreCompact,
    ] {
        for (label, stdin) in cases {
            let out = run(Some(&app), ev, stdin);
            assert_eq!(out.exit_code, 0, "{ev:?} / {label}");
            if matches!(ev, HookEvent::SessionStart) {
                assert!(
                    out.stdout.contains("### r0 `rule`"),
                    "{ev:?} / {label}: {}",
                    out.stdout
                );
            }
            if label != "extra fields" {
                assert!(
                    out.stderr.contains("proceeding as if the payload were {}"),
                    "{ev:?} / {label}: {}",
                    out.stderr
                );
            }
        }
    }
    // The CI job's exact payload: `echo {} |` under cmd.exe yields "{} \r\n".
    for ev in [
        HookEvent::SessionStart,
        HookEvent::UserPromptSubmit,
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::Stop,
        HookEvent::PreCompact,
    ] {
        let out = run(Some(&app), ev, "{} \r\n");
        assert_eq!(out.exit_code, 0);
    }
}

#[test]
fn a_session_id_that_cannot_name_a_file_is_refused_not_used() {
    assert_eq!(
        session::safe_id("sess-01AB.c_d"),
        Some("sess-01AB.c_d".into())
    );
    // Separators become `_` and a leading run of dots is trimmed; no component can escape.
    assert_eq!(
        session::safe_id("../../etc/passwd"),
        Some("_.._etc_passwd".into())
    );
    assert_eq!(session::safe_id(".."), None);
    assert_eq!(session::safe_id("///"), None);
    assert_eq!(session::safe_id(""), None);
    let long = "x".repeat(500);
    assert_eq!(session::safe_id(&long).unwrap().len(), 96);
    let f = fixture();
    let app = f.open();
    let out = run(
        Some(&app),
        HookEvent::Stop,
        r#"{"session_id":"..","hook_event_name":"Stop"}"#,
    );
    assert_eq!(out.exit_code, 0);
    assert!(out.stderr.contains("cannot name a file"), "{}", out.stderr);
    assert!(!f.store.join("sessions").exists());
}

// ---------------------------------------------------------------------------------------
// user-prompt-submit

fn prompt(f: &Fixture, app: &App) -> HookOutput {
    run(
        Some(app),
        HookEvent::UserPromptSubmit,
        &f.payload(serde_json::json!({"hook_event_name": "UserPromptSubmit", "prompt": "hi"})),
    )
}

#[test]
fn a_prompt_adds_nothing_while_the_resident_rings_are_unchanged() {
    let f = fixture();
    f.write(Ring::Invariant, "rule", "the rule");
    session_start(&f, "startup");
    let app = f.open();
    let out = prompt(&f, &app);
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
    assert!(out.stderr.contains("unchanged"), "{}", out.stderr);
}

#[test]
fn a_prompt_reinjects_a_resident_note_the_operator_changed() {
    let f = fixture();
    f.write(Ring::Invariant, "rule", "the old rule");
    f.write(Ring::Protocol, "proto", "the protocol");
    session_start(&f, "startup");
    // Operator edits ring 0 mid-session and adds a ring 1 note, removes another.
    f.write(Ring::Invariant, "rule", "the NEW rule");
    f.write(Ring::Protocol, "added", "brand new");
    std::fs::remove_file(f.store.join("notes/r1/proto.md")).unwrap();
    let app = f.open();
    let out = prompt(&f, &app);
    assert_eq!(out.exit_code, 0);
    let s = &out.stdout;
    assert!(s.contains("rings 0/1 changed"), "{s}");
    assert!(
        s.contains("## Changed: r0 `rule`") && s.contains("the NEW rule"),
        "{s}"
    );
    assert!(!s.contains("the old rule"), "{s}");
    assert!(s.contains("## New: r1 `added`"), "{s}");
    assert!(s.contains("## Removed: `r1/proto`"), "{s}");
    // And the baseline moved: the next prompt is quiet again.
    let out = prompt(&f, &app);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
    assert_eq!(f.state().unwrap().reinjections, 1);
}

#[test]
fn a_prompt_with_no_session_start_record_injects_everything_once() {
    let f = fixture();
    f.write(Ring::Invariant, "rule", "the rule");
    let app = f.open();
    let out = prompt(&f, &app);
    assert!(
        out.stdout.contains("No session-start record"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("### r0 `rule`"), "{}", out.stdout);
    assert!(audit_actions(&app).contains(&"session.start".to_string()));
    let out = prompt(&f, &app);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
}

// ---------------------------------------------------------------------------------------
// pre-tool-use / post-tool-use

fn decision(out: &HookOutput) -> (String, String) {
    let v: serde_json::Value = serde_json::from_str(&out.stdout)
        .unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", out.stdout));
    let h = &v["hookSpecificOutput"];
    assert_eq!(h["hookEventName"], "PreToolUse");
    (
        h["permissionDecision"].as_str().unwrap().to_string(),
        h["permissionDecisionReason"].as_str().unwrap().to_string(),
    )
}

#[test]
fn pre_tool_use_refuses_ring_0_and_the_audit_log_and_records_the_refusal() {
    let f = fixture();
    let app = f.open();
    let before = audit_actions(&app).len();
    let r0 = f.store.join("notes/r0/rule.md");
    let out = run(
        Some(&app),
        HookEvent::PreToolUse,
        &tool_payload(&f, "Edit", &r0),
    );
    assert_eq!(out.exit_code, 0);
    let (d, why) = decision(&out);
    assert_eq!(d, "deny");
    assert!(why.contains("ring 0") && why.contains("operator"), "{why}");
    let audit = f.store.join("audit.db");
    let out = run(
        Some(&app),
        HookEvent::PreToolUse,
        &tool_payload(&f, "Write", &audit),
    );
    let (d, why) = decision(&out);
    assert_eq!(d, "deny");
    assert!(why.contains("audit log"), "{why}");
    let wal = f.store.join("audit.db-wal");
    let (d, _) = decision(&run(
        Some(&app),
        HookEvent::PreToolUse,
        &tool_payload(&f, "Write", &wal),
    ));
    assert_eq!(d, "deny");
    let actions = audit_actions(&app);
    assert_eq!(actions.len(), before + 3);
    assert!(
        actions[before..].iter().all(|a| a == "policy.refusal"),
        "{actions:?}"
    );
}

#[test]
fn pre_tool_use_asks_about_rings_2_to_4_and_the_config() {
    let f = fixture();
    let app = f.open();
    let (d, why) = decision(&run(
        Some(&app),
        HookEvent::PreToolUse,
        &tool_payload(&f, "Edit", &f.store.join("notes/r2/pg18.md")),
    ));
    assert_eq!(d, "ask");
    assert!(why.contains("cyberbrain write"), "{why}");
    let (d, why) = decision(&run(
        Some(&app),
        HookEvent::PreToolUse,
        &tool_payload(&f, "Write", &f.store.join("cyberbrain.toml")),
    ));
    assert_eq!(d, "ask");
    assert!(why.contains("consent"), "{why}");
    // Only refusals reach the audit log; an `ask` is the operator's call.
    assert!(!audit_actions(&app).iter().any(|a| a == "policy.refusal"));
}

#[test]
fn pre_tool_use_is_silent_for_everything_outside_the_store() {
    let f = fixture();
    let app = f.open();
    let project = f.store.parent().unwrap();
    for (tool, path) in [
        ("Edit", project.join("src/main.rs")),
        ("Write", project.join(".cyberbrain2/notes/r0/x.md")),
        ("Write", project.join("docs/.cyberbrain/notes/r0/x.md")),
    ] {
        let out = run(
            Some(&app),
            HookEvent::PreToolUse,
            &tool_payload(&f, tool, &path),
        );
        assert_eq!(out.exit_code, 0);
        assert!(
            out.stdout.is_empty(),
            "{tool} {}: {}",
            path.display(),
            out.stdout
        );
        assert!(out.stderr.contains("outside the store"), "{}", out.stderr);
    }
    // Bash and Read are not file edits.
    for tool in ["Bash", "Read", "Grep", "Agent"] {
        let out = run(
            Some(&app),
            HookEvent::PreToolUse,
            &f.payload(serde_json::json!({"tool_name": tool, "tool_input": {"command": "rm -rf .cyberbrain"}})),
        );
        assert!(out.stdout.is_empty());
        assert!(out.stderr.contains("not a file edit"), "{}", out.stderr);
    }
    // A relative path is resolved against the payload's cwd.
    let out = run(
        Some(&app),
        HookEvent::PreToolUse,
        &f.payload(serde_json::json!({"tool_name": "Edit", "tool_input": {"file_path": ".cyberbrain/notes/r1/h.md"}})),
    );
    assert_eq!(decision(&out).0, "deny");
    // `..` inside the path is folded lexically.
    let out = run(
        Some(&app),
        HookEvent::PreToolUse,
        &f.payload(serde_json::json!({"tool_name": "Edit", "tool_input": {"file_path": "src/../.cyberbrain/notes/r0/h.md"}})),
    );
    assert_eq!(decision(&out).0, "deny");
}

#[test]
fn post_tool_use_says_the_index_is_stale_after_a_raw_note_edit() {
    let f = fixture();
    let app = f.open();
    let mut payload: serde_json::Value =
        serde_json::from_str(&tool_payload(&f, "Edit", &f.store.join("notes/r2/pg18.md"))).unwrap();
    payload["hook_event_name"] = "PostToolUse".into();
    payload["tool_response"] = serde_json::json!({"filePath": "x", "success": true});
    let out = run(Some(&app), HookEvent::PostToolUse, &payload.to_string());
    assert_eq!(out.exit_code, 0);
    let v: serde_json::Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PostToolUse");
    let ctx = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(ctx.contains("cyberbrain scan"), "{ctx}");
    assert!(ctx.contains("ring 2"), "{ctx}");
    // Outside the store: nothing.
    let out = run(
        Some(&app),
        HookEvent::PostToolUse,
        &tool_payload(&f, "Edit", &f.store.parent().unwrap().join("README.md")),
    );
    assert!(out.stdout.is_empty());
}

#[test]
fn path_classification_holds_on_both_platforms() {
    use paths::{Platform, StoreTarget, classify};
    let u = Platform::Unix;
    let w = Platform::Windows;
    // Unix: exact case, `/` only, `..` folded, relative against cwd.
    assert_eq!(
        classify(u, "/p/.cyberbrain", "/p", "/p/.cyberbrain/notes/r0/a.md"),
        Some(StoreTarget::ResidentNote(Ring::Invariant))
    );
    assert_eq!(
        classify(u, ".cyberbrain", "/p", "src/../.cyberbrain/notes/r3/a.md"),
        Some(StoreTarget::Note(Ring::Session))
    );
    assert_eq!(
        classify(u, "/p/.cyberbrain", "/p", "/p/.Cyberbrain/audit.db"),
        None
    );
    assert_eq!(
        classify(u, "/p/.cyberbrain", "/p", "/p/.cyberbrain2/audit.db"),
        None
    );
    assert_eq!(classify(u, "/p/.cyberbrain", "/p", "/p/.cyberbrain"), None);
    assert_eq!(
        classify(u, "/p/.cyberbrain", "/p", "/p/.cyberbrain/audit.db-journal"),
        Some(StoreTarget::AuditLog)
    );
    assert_eq!(
        classify(u, "/p/.cyberbrain", "/p", "/p/.cyberbrain/models/x"),
        Some(StoreTarget::Other)
    );
    assert_eq!(
        classify(u, "/p/.cyberbrain", "/p", "/p/.cyberbrain/notes/r9/x.md"),
        Some(StoreTarget::Other)
    );
    // A Windows-style path on Unix is one file name and never matches.
    assert_eq!(
        classify(u, "/p/.cyberbrain", "/p", r"\p\.cyberbrain\audit.db"),
        None
    );
    // Windows: either separator, case folded, drive letters, the CI job's relative store.
    assert_eq!(
        classify(
            w,
            r"C:\Program Files\x\.cyberbrain",
            r"C:\x",
            r"c:/program files/X/.CYBERBRAIN/Notes/R1/h.md"
        ),
        Some(StoreTarget::ResidentNote(Ring::Protocol))
    );
    assert_eq!(
        classify(
            w,
            ".cyberbrain",
            r"D:\a b\c",
            r"D:\a b\c\.cyberbrain\cyberbrain.toml"
        ),
        Some(StoreTarget::Config)
    );
    assert_eq!(
        classify(
            w,
            r"C:\x\.cyberbrain",
            r"C:\x",
            r"D:\x\.cyberbrain\audit.db"
        ),
        None
    );
    assert_eq!(
        classify(
            w,
            r"\\?\C:\x\.cyberbrain",
            r"C:\x",
            r"C:\x\.cyberbrain\audit.db"
        ),
        Some(StoreTarget::AuditLog)
    );
    assert_eq!(
        classify(
            w,
            r"\\srv\share\.cyberbrain",
            r"\\srv\share",
            r"\\srv\share\.cyberbrain\sessions\s.json"
        ),
        Some(StoreTarget::SessionState)
    );
}

// ---------------------------------------------------------------------------------------
// stop

#[test]
fn stop_counts_the_turn_and_never_blocks() {
    let f = fixture();
    let app = f.open();
    let before = audit_actions(&app).len();
    for i in 1..=3 {
        let out = run(
            Some(&app),
            HookEvent::Stop,
            &f.payload(serde_json::json!({"hook_event_name": "Stop", "stop_hook_active": i == 3})),
        );
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.is_empty(), "{}", out.stdout);
        assert_eq!(f.state().unwrap().turns, i);
    }
    // No audit rows and no notes for a turn.
    assert_eq!(audit_actions(&app).len(), before);
    assert!(
        std::fs::read_dir(f.store.join("notes/r3"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn old_session_state_is_swept_at_startup() {
    let f = fixture();
    let dir = session::sessions_dir(&f.store);
    std::fs::create_dir_all(&dir).unwrap();
    let old = dir.join("old.json");
    std::fs::write(&old, "{}").unwrap();
    let then = std::time::SystemTime::now() - Duration::from_secs(40 * 24 * 3600);
    let ft = std::fs::File::options().write(true).open(&old).unwrap();
    ft.set_modified(then).unwrap();
    drop(ft);
    session_start(&f, "startup");

    // Diagnostic rather than a bare assertion. This failed intermittently until the hook
    // tests stopped running beside each other: they share the process's working directory,
    // environment, panic hook and fault injector, and a sibling test's temporary store was
    // being resolved here. The fixture now holds a process lock, and the cause is known.
    //
    // The instrumentation stays as a tripwire. If it ever fires again the next person gets
    // the age as read back and the directory's contents, rather than a line number.
    if old.exists() {
        let age = std::fs::metadata(&old)
            .and_then(|m| m.modified())
            .map(|t| std::time::SystemTime::now().duration_since(t));
        let siblings: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        panic!(
            "the sweep left a state file older than {} days behind.\n             file: {}\n  age as read back: {age:?}\n  directory now holds: {siblings:?}",
            session::STATE_MAX_AGE_DAYS,
            old.display(),
        );
    }
    assert!(f.state().is_some());
}

// ---------------------------------------------------------------------------------------
// SPEC §9.1 / §14.2: the never-fail rule, demonstrated on an injected failure.

#[test]
fn the_never_fail_rule_holds_for_an_error_and_for_a_panic() {
    let f = fixture();
    let app = f.open();
    let before = audit_actions(&app).len();

    FAIL_NEXT.with(|f| f.set(1));
    let out = run(
        Some(&app),
        HookEvent::SessionStart,
        &f.payload(serde_json::json!({})),
    );
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
    assert!(out.stderr.contains("injected failure"), "{}", out.stderr);
    assert!(
        out.stderr.contains("recorded in the audit log"),
        "{}",
        out.stderr
    );

    FAIL_NEXT.with(|f| f.set(2));
    let out = run(
        Some(&app),
        HookEvent::PreToolUse,
        &f.payload(serde_json::json!({})),
    );
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.is_empty());
    assert!(
        out.stderr.contains("panic: injected panic"),
        "{}",
        out.stderr
    );

    let actions = audit_actions(&app);
    assert_eq!(&actions[before..], ["hook.error", "hook.error"]);

    // With no store, the error is still swallowed; there is just nowhere to record it.
    FAIL_NEXT.with(|f| f.set(1));
    let out = run(None, HookEvent::Stop, "{}");
    assert_eq!(out.exit_code, 0);
    assert!(
        out.stderr.contains("no store to record it in"),
        "{}",
        out.stderr
    );
}

// ---------------------------------------------------------------------------------------
// Budgets (SPEC §9.1). Debug builds are far slower; CI runs these in release with
// `cargo test --release -- --ignored`. The store here holds 2000 ring-2 notes precisely so
// that a cost growing with the store would show.

fn big_fixture() -> Fixture {
    let f = fixture();
    f.write(Ring::Invariant, "rule-a", &"Invariant text. ".repeat(60));
    f.write(
        Ring::Invariant,
        "rule-b",
        &"More invariant text. ".repeat(60),
    );
    f.write(Ring::Protocol, "handoff", &"Protocol text. ".repeat(80));
    // Straight to disk: 2000 writes through the App would take a while and are not the
    // thing under test. The hook never lists ring 2, so this is what has to not matter.
    let dir = f.store.join("notes/r2");
    for i in 0..2000 {
        let id = cyberbrain_core::NoteId::generate();
        std::fs::write(
            dir.join(format!("note-{i:04}.md")),
            format!(
                "---\nid: {id}\nname: note-{i:04}\nring: 2\nkind: knowledge\ncreated: 2026-09-05T00:00:00Z\nupdated: 2026-09-05T00:00:00Z\n---\n\nBody of note {i}.\n"
            ),
        )
        .unwrap();
    }
    f
}

fn p99(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[(samples.len() * 99 / 100).min(samples.len() - 1)]
}

#[test]
#[ignore = "budget benchmark; run in release: cargo test --release -- --ignored"]
fn hot_path_events_stay_under_15_ms_p99_and_session_start_under_150() {
    let f = big_fixture();
    let app = f.open();
    let r0 = f.store.join("notes/r0/rule-a.md");
    let start = session_start(&f, "startup");
    assert!(start.stdout.contains("### r0 `rule-a`"));

    let mut report = String::new();
    let cases: Vec<(&str, HookEvent, String)> = vec![
        (
            "session-start",
            HookEvent::SessionStart,
            f.payload(serde_json::json!({"source": "startup"})),
        ),
        (
            "user-prompt-submit",
            HookEvent::UserPromptSubmit,
            f.payload(serde_json::json!({"prompt": "x"})),
        ),
        (
            "pre-tool-use (deny)",
            HookEvent::PreToolUse,
            tool_payload(&f, "Edit", &r0),
        ),
        (
            "pre-tool-use (other)",
            HookEvent::PreToolUse,
            f.payload(serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}})),
        ),
        (
            "post-tool-use",
            HookEvent::PostToolUse,
            f.payload(serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "ls"}})),
        ),
        ("stop", HookEvent::Stop, f.payload(serde_json::json!({}))),
        (
            "pre-compact",
            HookEvent::PreCompact,
            f.payload(serde_json::json!({"trigger": "auto"})),
        ),
    ];
    for (label, ev, stdin) in &cases {
        let n = if matches!(ev, HookEvent::SessionStart) {
            50
        } else {
            200
        };
        let mut samples = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let out = run(Some(&app), *ev, stdin);
            samples.push(t.elapsed());
            assert_eq!(out.exit_code, 0);
        }
        let p = p99(samples.clone());
        let med = samples[samples.len() / 2];
        report.push_str(&format!("{label}: p99 {p:?}, median {med:?}\n"));
        assert!(
            p.as_millis() < budget_ms(*ev),
            "{label}: p99 {p:?} over budget {} ms\n{report}",
            budget_ms(*ev)
        );
    }
    // What the caller pays before `run`: opening the App.
    let mut samples = Vec::new();
    for _ in 0..100 {
        let t = Instant::now();
        let _ = f.open();
        samples.push(t.elapsed());
    }
    report.push_str(&format!("App::open: p99 {:?}\n", p99(samples)));
    eprintln!("{report}");
}

// ---------------------------------------------------------------------------------------
// Guards

/// The working directory and the environment belong to the process, not to a test, and
/// these tests run in parallel with every other test in this binary. Both guards therefore
/// hold **the same** lock, and hold it for as long as the change is in effect.
///
/// The first version took the lock inside `enter()` into a local binding, so it was
/// released the moment the guard was returned — it serialised the call to
/// `set_current_dir` and left the whole period during which the directory was changed
/// unprotected. Sibling tests resolving a store from the working directory then saw
/// another test's temporary directory, and failed roughly one workspace run in two while
/// passing every time they were run alone. A lock that does not span the critical section
/// is not a lock.
type ProcessLock = std::sync::MutexGuard<'static, ()>;

fn process_lock() -> ProcessLock {
    cwd_lock().lock().unwrap_or_else(|e| e.into_inner())
}

// No lock of its own: the fixture already holds it, and taking a non-reentrant mutex twice
// on one thread would deadlock. Every user of this guard builds a fixture first.
struct CwdGuard(std::path::PathBuf);
impl CwdGuard {
    fn enter(p: &Path) -> Self {
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(p).unwrap();
        CwdGuard(old)
    }
}
impl Drop for CwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

struct EnvGuard(&'static str, Option<std::ffi::OsString>);
impl EnvGuard {
    fn set(k: &'static str, v: &str) -> Self {
        let old = std::env::var_os(k);
        // Writes are scoped to the guard, and the fixture's process lock keeps a sibling
        // test from observing them; Rust 2024 marks the call unsafe regardless.
        unsafe { std::env::set_var(k, v) };
        EnvGuard(k, old)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.1 {
                Some(v) => std::env::set_var(self.0, v),
                None => std::env::remove_var(self.0),
            }
        }
    }
}

fn cwd_lock() -> &'static std::sync::Mutex<()> {
    static L: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &L
}
