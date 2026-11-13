//! `hook pre-compact-snapshot`: what the compaction mod hands over, what lands in the store,
//! and what comes back as instructions for the summary.

use super::*;

const CITED: &str = "r2-a91f2c33e1bd";
const CITED_LATER: &str = "r0-0123456789ab";

fn snapshot_payload(f: &Fixture, messages: serde_json::Value) -> String {
    f.payload(serde_json::json!({
        "hook_event_name": "PreCompact",
        "trigger": "auto",
        "messages": messages,
    }))
}

fn snapshot(f: &Fixture, messages: serde_json::Value) -> HookOutput {
    let app = f.open();
    run(
        Some(&app),
        HookEvent::PreCompactSnapshot,
        &snapshot_payload(f, messages),
    )
}

fn answer(out: &HookOutput) -> serde_json::Value {
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {:?}", out.stdout))
}

fn conversation() -> serde_json::Value {
    serde_json::json!([
        { "role": "user", "text": "Fix the login bug in auth.rs", "tools": [] },
        { "role": "assistant", "text": format!("Per [{CITED}] the session cookie is the cause."),
          "tools": ["Read", "Edit"] },
        { "role": "user", "text": "",
          "toolResults": [{ "tool_use_id": "t1", "text": "SECRET TOOL OUTPUT", "isError": false }] },
        { "role": "assistant", "text": format!("Done; see {CITED_LATER}."),
          "toolUses": [{ "tool": "Bash", "tool_use_id": "t2", "input": { "command": "cargo test" } }] },
    ])
}

#[test]
fn the_snapshot_lands_in_ring_3_and_its_citations_steer_the_summary() {
    let f = fixture();
    f.write(
        Ring::Protocol,
        "handoff-state",
        "Where we stand.\n\n- [ ] open: migrate the session table\n- done: the rest",
    );
    session_start(&f, "startup");

    let out = snapshot(&f, conversation());
    assert_eq!(out.exit_code, 0);
    let v = answer(&out);
    let instructions = v["instructions"].as_str().unwrap();
    // Most recently cited first, both kept verbatim.
    let (a, b) = (
        instructions.find(CITED_LATER).unwrap(),
        instructions.find(CITED).unwrap(),
    );
    assert!(a < b, "{instructions}");
    assert!(
        instructions.contains("migrate the session table"),
        "{instructions}"
    );
    let name = v["snapshot"]["name"].as_str().unwrap();
    let citation = v["snapshot"]["citation"].as_str().unwrap();
    assert!(citation.starts_with("r3-"), "{v}");
    assert!(instructions.contains(citation), "{instructions}");

    let app = f.open();
    let note = app.store().read(name).unwrap();
    assert_eq!(note.front.ring, Ring::Session);
    assert_eq!(note.front.kind, NoteKind::Session);
    assert_eq!(note.front.retention.as_deref(), Some("P30D"));
    assert!(note.body.contains("Fix the login bug"), "{}", note.body);
    assert!(note.body.contains(CITED), "{}", note.body);
    assert!(note.body.contains("Bash"), "{}", note.body);
    // Tool names only: no output, no input.
    assert!(!note.body.contains("SECRET TOOL OUTPUT"), "{}", note.body);
    assert!(!note.body.contains("cargo test"), "{}", note.body);
    assert!(audit_actions(&app).iter().any(|a| a == "note.write"));

    let st = f.state().unwrap();
    let mark = st.last_snapshot.unwrap();
    assert_eq!(mark.name, name);
    assert_eq!(mark.citation, citation);
    assert_eq!(mark.compaction, 1);
}

#[test]
fn personal_data_in_the_conversation_is_redacted_before_it_is_written() {
    let f = fixture();
    let out = snapshot(
        &f,
        serde_json::json!([
            { "role": "user", "text": "Mail max.mustermann@example.com about it", "tools": [] },
        ]),
    );
    let v = answer(&out);
    let name = v["snapshot"]["name"].as_str().unwrap();
    let note = f.open().store().read(name).unwrap();
    assert!(
        !note.body.contains("max.mustermann@example.com"),
        "{}",
        note.body
    );
    assert!(note.body.contains("Mail"), "{}", note.body);
}

#[test]
fn reminders_stay_out_and_the_size_is_bounded() {
    let f = fixture();
    let huge = "x".repeat(200_000);
    let mut messages = Vec::new();
    for i in 0..200 {
        messages.push(serde_json::json!({
            "role": if i % 2 == 0 { "user" } else { "assistant" },
            "text": format!("<system-reminder>RING ZERO TEXT</system-reminder>turn {i} {huge}"),
            "tools": ["Read"],
        }));
    }
    let out = snapshot(&f, serde_json::Value::Array(messages));
    let v = answer(&out);
    let note = f
        .open()
        .store()
        .read(v["snapshot"]["name"].as_str().unwrap())
        .unwrap();
    assert!(
        note.body.len() <= crate::hook::snapshot::MAX_BODY_BYTES,
        "{}",
        note.body.len()
    );
    assert!(!note.body.contains("RING ZERO TEXT"));
    assert!(!note.body.contains("system-reminder"));
    assert!(note.body.contains("turn 199"), "the newest turn is kept");
    assert!(
        v["instructions"].as_str().unwrap().len() <= crate::hook::snapshot::MAX_INSTRUCTIONS_BYTES
    );
}

#[test]
fn session_start_after_the_compaction_points_to_the_snapshot() {
    let f = fixture();
    session_start(&f, "startup");
    let v = answer(&snapshot(&f, conversation()));
    let citation = v["snapshot"]["citation"].as_str().unwrap().to_string();
    let app = f.open();
    run(
        Some(&app),
        HookEvent::PreCompact,
        &f.payload(serde_json::json!({"hook_event_name": "PreCompact", "trigger": "auto"})),
    );
    let out = session_start(&f, "compact");
    assert!(out.stdout.contains(&citation), "{}", out.stdout);
    assert!(out.stdout.contains("recall --id"), "{}", out.stdout);

    // A later compaction without a snapshot does not point to the old one as if it were new.
    run(
        Some(&app),
        HookEvent::PreCompact,
        &f.payload(serde_json::json!({"hook_event_name": "PreCompact", "trigger": "auto"})),
    );
    let out = session_start(&f, "compact");
    assert!(!out.stdout.contains(&citation), "{}", out.stdout);
}

#[test]
fn a_second_snapshot_for_the_same_compaction_replaces_the_first() {
    // `precompute` runs ahead of the real compaction, which then runs the hook again.
    let f = fixture();
    session_start(&f, "startup");
    let first = answer(&snapshot(&f, conversation()));
    let second = answer(&snapshot(&f, conversation()));
    assert_eq!(first["snapshot"]["name"], second["snapshot"]["name"]);
}

#[test]
fn a_store_that_cannot_take_the_note_still_gets_instructions_and_exit_0() {
    let f = fixture();
    // notes/r3 as a file: the write fails, the answer must not.
    let r3 = f.store.join("notes").join("r3");
    let _ = std::fs::remove_dir_all(&r3);
    std::fs::write(&r3, "not a directory").unwrap();
    let out = snapshot(&f, conversation());
    assert_eq!(out.exit_code, 0);
    let v = answer(&out);
    assert!(v["snapshot"].is_null(), "{v}");
    assert!(v["instructions"].as_str().unwrap().contains(CITED));
    assert!(out.stderr.contains("snapshot"), "{}", out.stderr);
}

#[test]
fn a_subagent_transcript_is_not_kept_and_a_bad_payload_answers_nothing() {
    let f = fixture();
    let app = f.open();
    let out = run(
        Some(&app),
        HookEvent::PreCompactSnapshot,
        &f.payload(serde_json::json!({
            "hook_event_name": "PreCompact",
            "agent_id": "a1",
            "messages": conversation(),
        })),
    );
    let v = answer(&out);
    assert!(v["snapshot"].is_null(), "{v}");
    assert!(v["instructions"].as_str().unwrap().contains(CITED));

    for bad in ["", "not json", "[]", r#"{"messages": 7}"#] {
        let out = run(Some(&app), HookEvent::PreCompactSnapshot, bad);
        assert_eq!(out.exit_code, 0, "{bad}");
    }
    // Standing down: no store, still exit 0 and no answer the mod would act on.
    let out = run(
        None,
        HookEvent::PreCompactSnapshot,
        &snapshot_payload(&f, conversation()),
    );
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.is_empty(), "{}", out.stdout);
}

#[test]
fn pre_compact_message_names_the_snapshot_only_when_the_mod_is_installed() {
    // 13.11.2026: the shell PreCompact line still said that nothing unwritten survives a
    // compaction, also in projects where the compaction mod keeps a snapshot and the citations.
    let f = fixture();
    session_start(&f, "startup");
    let app = f.open();
    let payload =
        f.payload(serde_json::json!({"hook_event_name": "PreCompact", "trigger": "manual"}));
    let without = run(Some(&app), HookEvent::PreCompact, &payload);
    assert!(
        without.stdout.contains("does not survive"),
        "{}",
        without.stdout
    );
    assert!(
        without.stdout.contains("--compact-mod"),
        "{}",
        without.stdout
    );

    let hooks = crate::install::compact_mod::folder(f.store.parent().unwrap()).join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(hooks.join("register.ts"), "// test\n").unwrap();
    let with = run(Some(&app), HookEvent::PreCompact, &payload);
    assert!(with.stdout.contains("snapshot"), "{}", with.stdout);
    assert!(!with.stdout.contains("does not survive"), "{}", with.stdout);
}
