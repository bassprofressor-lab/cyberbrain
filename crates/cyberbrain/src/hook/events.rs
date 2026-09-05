//! The six events. What each one does, and what it deliberately does not.
//!
//! | event | model sees | does |
//! |---|---|---|
//! | `session-start` | plain stdout | rings 0 and 1 in full with citations, a digest of store state, one audit row `session.start`; announces stand-down |
//! | `user-prompt-submit` | plain stdout | re-injects a resident note that changed since it was injected; injects everything if no session-start ran; else nothing |
//! | `pre-tool-use` | JSON decision | refuses a raw edit of the audit log or of rings 0/1, asks the operator about the config and rings 2..4; otherwise nothing |
//! | `post-tool-use` | JSON `additionalContext` | after an allowed raw edit of a store file, says the index is now stale (or the chain broken); otherwise nothing |
//! | `stop` | nothing | counts the turn in the session state; never blocks |
//! | `pre-compact` | nothing (no channel) | counts the compaction; tells the human what survives and how |
//!
//! Why the hot-path events are this thin: `pre-tool-use` and `post-tool-use` fire on
//! every tool call, and their stdout never reaches the model. The only thing worth their
//! cost is a decision the harness will act on, and the only decision this tool is entitled
//! to make is about its own files: the audit log is append-only evidence (§4, §12.6),
//! rings 0 and 1 are the operator's (§3.2, §11), and a raw edit of any note skips the PII
//! gate (§12.4), the audit row and the reindex that `cyberbrain write` performs. So they
//! check one path against one prefix and are otherwise silent. No counters: a counter
//! nobody reads is pure cost.
//!
//! Why `user-prompt-submit` checks the resident rings: §3.2 says rings 0 and 1 are
//! *always* injected, and an invariant the operator changed mid-session is not in the
//! agent's context until something puts it there. Fingerprinting the resident files costs
//! what the resident cap allows (≤ 8k tokens, a few files) and nothing more; it does not
//! grow with the store. A recall of the prompt text was considered and rejected: it would
//! have to be lexical-only (§6.5 forbids the embedder here), which §7 refuses as a mode,
//! and it would put unasked-for hits into every prompt.
//!
//! Why `pre-compact` injects nothing: the documented interface gives it no channel to the
//! model, and the harness fires `session-start` with `source: "compact"` afterwards, which
//! is where rings 0 and 1 come back. What compaction actually loses is what the agent
//! learned and did not write; no hook can recover that, so this one records the
//! compaction and tells the human.
//!
//! Why `stop` writes no note and no audit row: it fires at the end of every assistant
//! turn, not at the end of the session; `App::write` would load the embedder (§6.5) and a
//! per-turn audit row is a log of nothing. The turn count lands in the session state, which
//! `session-start` reads back on `resume` and `compact`.

use super::paths::{self, StoreTarget};
use super::payload::Payload;
use super::resident::{self, Resident};
use super::session::{self, SessionState};
use super::{HookOutput, StandDown, event_name, harness_event_name};
use crate::app::App;
use crate::cli::HookEvent;
use cyberbrain_core::{Result, Ring, Slash, slash};
use cyberbrain_policy::{Actor, AuditAction};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

// Test hook for the never-fail rule: 1 makes the next dispatch return an error, 2 makes
// it panic. Zero otherwise. See `tests::the_never_fail_rule`.
//
// **Thread-local, not global.** As a process-wide atomic it was consumed by whichever
// hook test happened to dispatch next, so an injected panic surfaced inside an unrelated
// test and the suite failed about one workspace run in two while every test passed when
// run alone. Making it thread-local confines the injection to the test that asked for it,
// which is what a fault injector is supposed to mean. Serialising the tests would have
// hidden the same race behind a lock instead of removing it.
#[cfg(test)]
thread_local! {
    pub(super) static FAIL_NEXT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

struct Ctx<'a> {
    app: &'a App,
    payload: &'a Payload,
    event: HookEvent,
    now: String,
}

impl Ctx<'_> {
    fn actor(&self) -> Actor {
        Actor::Hook(event_name(self.event).into())
    }

    fn root(&self) -> &Path {
        self.app.root()
    }

    fn cwd(&self) -> PathBuf {
        self.payload
            .cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    /// The sanitised session id, or a stderr line saying why there is none.
    fn session_id(&self, out: &mut HookOutput) -> Option<String> {
        match &self.payload.session_id {
            None => {
                out.note("payload carries no session_id; per-session state is off for this call");
                None
            }
            Some(raw) => match session::safe_id(raw) {
                Some(id) => Some(id),
                None => {
                    out.note(format!(
                        "session_id {raw:?} cannot name a file; per-session state is off for this call"
                    ));
                    None
                }
            },
        }
    }

    /// The state for `id`, or `None` with a reason on stderr.
    fn load_state(&self, id: &str, out: &mut HookOutput) -> Option<SessionState> {
        match session::load(self.root(), id) {
            Ok(s) => s,
            Err(e) => {
                out.note(format!("{e}; treating this session as having no baseline"));
                None
            }
        }
    }
}

pub(super) fn dispatch(
    app: Option<&App>,
    stand_down: Option<StandDown>,
    event: HookEvent,
    stdin: &str,
) -> Result<HookOutput> {
    let payload = Payload::parse(stdin);
    let mut out = HookOutput::empty();
    for n in &payload.notes {
        out.note(n);
    }

    #[cfg(test)]
    match FAIL_NEXT.with(|f| f.replace(0)) {
        1 => return Err(cyberbrain_core::Error::Index("injected failure".into())),
        2 => panic!("injected panic"),
        _ => {}
    }

    if let Some(sd) = stand_down {
        match event {
            HookEvent::SessionStart => out.stdout = sd.announce(),
            _ => out.note(format!(
                "hook {} standing down: {}",
                event_name(event),
                sd.announce()
            )),
        }
        return Ok(out);
    }
    let Some(app) = app else {
        out.note("no store handle and no stand-down reason; this is a wiring bug, doing nothing");
        return Ok(out);
    };
    let ctx = Ctx {
        app,
        payload: &payload,
        event,
        now: now(),
    };
    match event {
        HookEvent::SessionStart => session_start(&ctx, &mut out)?,
        HookEvent::UserPromptSubmit => user_prompt_submit(&ctx, &mut out)?,
        HookEvent::PreToolUse => pre_tool_use(&ctx, &mut out)?,
        HookEvent::PostToolUse => post_tool_use(&ctx, &mut out)?,
        HookEvent::Stop => stop(&ctx, &mut out)?,
        HookEvent::PreCompact => pre_compact(&ctx, &mut out)?,
    }
    Ok(out)
}

fn now() -> String {
    let t = jiff::Timestamp::now();
    t.round(jiff::Unit::Second).unwrap_or(t).to_string()
}

fn ts_of(t: SystemTime) -> String {
    jiff::Timestamp::try_from(t)
        .ok()
        .and_then(|t| t.round(jiff::Unit::Second).ok())
        .map(|t| t.to_string())
        .unwrap_or_else(|| "unknown time".into())
}

// ---------------------------------------------------------------------------------------
// session-start

fn session_start(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    let store = ctx.app.store();
    let source = ctx.payload.source.as_deref().unwrap_or("unknown");
    let res = resident::read(store);

    // State: continue an existing session on resume/compact, start fresh otherwise.
    let id = ctx.session_id(out);
    let mut state = id.as_deref().map(|id| {
        ctx.load_state(id, out)
            .unwrap_or_else(|| SessionState::new(id, &ctx.now))
    });
    if let Some(s) = state.as_mut() {
        if s.source.is_none() {
            s.source = Some(source.to_string());
        }
        if s.cwd.is_none() {
            s.cwd = ctx.payload.cwd.as_deref().map(slash);
        }
        s.injections += 1;
        s.last_injection_at = Some(ctx.now.clone());
        s.resident = resident::marks(&res);
    }

    let mut text = String::new();
    text.push_str("# Cyberbrain memory\n\n");
    text.push_str(&format!(
        "Store: `{}`. Rings 0 and 1 below are resident: they apply to everything you do in \
         this project. Every block is prefixed with its citation; quote it when you rely on \
         it, and expand one with `cyberbrain recall --id <citation>`.\n\n",
        Slash(ctx.root())
    ));
    match source {
        "compact" => {
            let (k, turns) = state
                .as_ref()
                .map(|s| (s.compactions, s.turns))
                .unwrap_or((0, 0));
            text.push_str(&format!(
                "Context was just compacted (compaction #{k} of this session, {turns} turn(s) \
                 so far). Rings 0 and 1 follow again in full. Nothing else from before the \
                 compaction is re-supplied here: what you learned and did not write with \
                 `cyberbrain write` is gone from your context, and still in the store only if \
                 you wrote it.\n\n"
            ));
        }
        "resume" => {
            if let Some(s) = &state {
                text.push_str(&format!(
                    "Resumed session (started {}, {} turn(s), {} compaction(s) so far).\n\n",
                    s.started_at.as_deref().unwrap_or("at an unknown time"),
                    s.turns,
                    s.compactions
                ));
            }
        }
        "clear" => text.push_str("Context was cleared; this is a fresh injection.\n\n"),
        _ => {}
    }
    resident::render_rings(&res, &mut text);
    digest(ctx, &res, state.as_ref(), source, &mut text);
    usage(&mut text);
    out.stdout = text;

    // The record: which invariants were shown to which session, when (§12.6 is the seam
    // AgentGuard reads). One row per injection, never per turn.
    let subject = format!("session:{}", id.as_deref().unwrap_or("unknown"));
    ctx.app.policy().audit().record_raw(
        &ctx.actor().to_string(),
        "session.start",
        subject,
        json!({
            "source": source,
            "cwd": ctx.payload.cwd.as_deref().map(slash),
            "resident_notes": res.notes.iter().map(resident::ResidentNote::key).collect::<Vec<_>>(),
            "blocks": res.notes.iter().map(|n| n.blocks.len()).sum::<usize>(),
            "approx_tokens": res.tokens,
            "unreadable": res.unreadable.len(),
            "injection": state.as_ref().map(|s| s.injections),
        }),
    )?;

    if let (Some(id), Some(s)) = (&id, &state) {
        session::save(ctx.root(), id, s)?;
    }
    if source == "startup" {
        match session::sweep(ctx.root(), SystemTime::now()) {
            Ok(0) => {}
            Ok(n) => out.note(format!(
                "removed {n} session state file(s) older than {} days",
                session::STATE_MAX_AGE_DAYS
            )),
            Err(e) => out.note(format!("session state sweep skipped: {e}")),
        }
    }
    Ok(())
}

fn digest(
    ctx: &Ctx<'_>,
    res: &Resident,
    state: Option<&SessionState>,
    source: &str,
    text: &mut String,
) {
    let store = ctx.app.store();
    let cap = store.resident_cap();
    text.push_str("## Store digest\n\n");
    let pct = (res.tokens * 100).checked_div(cap).unwrap_or(0);
    text.push_str(&format!(
        "- resident: {} note(s) in rings 0/1, ~{} of {} tokens ({pct}%){}\n",
        res.notes.len(),
        res.tokens,
        cap,
        if res.tokens > cap {
            "; OVER THE CAP, a hand edit crossed it (SPEC §3.2): tell the operator"
        } else {
            ""
        }
    ));
    if res.other_files > 0 {
        text.push_str(&format!(
            "- {} entr{} in notes/r0 or notes/r1 that are not `.md` notes were passed over\n",
            res.other_files,
            if res.other_files == 1 { "y" } else { "ies" }
        ));
    }
    let policy = ctx.app.policy();
    text.push_str(&format!(
        "- policy profile: {} (PII scan on writes: {})\n",
        policy.profile().as_str(),
        if policy.status().pii_scan_active {
            "on"
        } else {
            "off"
        }
    ));

    // Index freshness, as far as two stats can tell: the cache's mtime against the
    // resident files'. Anything wider costs a walk of the store.
    match std::fs::metadata(store.db_path()) {
        Ok(md) => {
            let written = md.modified().ok();
            text.push_str(&format!(
                "- index: cyberbrain.db last written {} ({} KB)\n",
                written.map(ts_of).unwrap_or_else(|| "unknown".into()),
                md.len() / 1024
            ));
            let newer: Vec<&str> = res
                .notes
                .iter()
                .filter(|n| match (n.fingerprint.mtime, written) {
                    (Some(a), Some(b)) => a > b,
                    _ => false,
                })
                .map(|n| n.note.front.name.as_str())
                .collect();
            if !newer.is_empty() {
                text.push_str(&format!(
                    "- resident note(s) changed after the last index write: {}; run `cyberbrain scan` \
                     before relying on recall for them (only rings 0/1 were checked)\n",
                    newer.join(", ")
                ));
            }
        }
        Err(_) => text.push_str(
            "- index: cyberbrain.db is absent; `recall` has nothing to search until `cyberbrain scan` runs\n",
        ),
    }
    let model_dir = ctx.app.config().model_dir();
    let model_present =
        model_dir.join("model.safetensors").is_file() && model_dir.join("tokenizer.json").is_file();
    text.push_str(&format!(
        "- semantic search: model artefact {} at {}{}\n",
        if model_present { "present" } else { "absent" },
        Slash(&model_dir),
        if model_present {
            " (not loaded by this hook; `recall` loads it)"
        } else {
            "; recall is lexical only until one is placed"
        }
    ));
    match state {
        Some(s) => text.push_str(&format!(
            "- session {}: source {source}, injection #{}, {} turn(s), {} compaction(s)\n",
            s.session_id, s.injections, s.turns, s.compactions
        )),
        None => text.push_str(
            "- session: no usable session_id in the payload, so nothing is tracked across \
             events for this session\n",
        ),
    }
    text.push('\n');
}

fn usage(text: &mut String) {
    text.push_str("## Using the store\n\n");
    text.push_str(
        "- `cyberbrain recall \"<question>\"` searches rings 2 to 4 (hybrid; every hit carries a \
         citation such as `r2-a91f2c33e1bd` and a `conflict` marker when a lower ring \
         disagrees). `cyberbrain recall --id <citation>` expands one.\n\
         - `cyberbrain find <symbol>` gives exact line ranges from the code index; read the slice, \
         not the file.\n\
         - `cyberbrain write --ring 2 --kind knowledge|bug|lesson|decision|reference --name <kebab-slug> \
         --body '...'` records what you learn. Ring 2 is project knowledge, ring 3 a session \
         record. Rings 0 and 1 are the operator's: propose text, never write them.\n\
         - Do not edit files under the store with Edit/Write. The pre-tool-use hook refuses the \
         audit log and rings 0/1 and asks the operator about the rest, because a raw edit skips \
         the PII gate, the audit row and the reindex that `cyberbrain write` performs.\n\n",
    );
}

// ---------------------------------------------------------------------------------------
// user-prompt-submit

fn user_prompt_submit(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    let Some(id) = ctx.session_id(out) else {
        out.note("cannot compare the resident rings against an earlier injection; nothing added");
        return Ok(());
    };
    let res = resident::read(ctx.app.store());
    match ctx.load_state(&id, out) {
        None => {
            // session-start never ran for this session: the hook was registered mid-way,
            // the harness has no session-start hook, or the state was swept. Rings 0/1
            // are "always injected" (§3.2); this is the first chance.
            let mut text = String::new();
            text.push_str(
                "# Cyberbrain memory (injected at this prompt)\n\nNo session-start record \
                 exists for this session, so rings 0 and 1 were never shown to you. Here they \
                 are; they apply to everything in this project. Quote a block's citation when \
                 you rely on it.\n\n",
            );
            resident::render_rings(&res, &mut text);
            out.stdout = text;
            let mut s = SessionState::new(&id, &ctx.now);
            s.source = Some("user-prompt-submit (no session-start record)".into());
            s.cwd = ctx.payload.cwd.as_deref().map(slash);
            s.injections = 1;
            s.last_injection_at = Some(ctx.now.clone());
            s.resident = resident::marks(&res);
            ctx.app.policy().audit().record_raw(
                &ctx.actor().to_string(),
                "session.start",
                format!("session:{id}"),
                json!({
                    "source": "user-prompt-submit: no session-start record",
                    "resident_notes": res.notes.iter().map(resident::ResidentNote::key).collect::<Vec<_>>(),
                    "approx_tokens": res.tokens,
                    "unreadable": res.unreadable.len(),
                }),
            )?;
            session::save(ctx.root(), &id, &s)?;
        }
        Some(mut s) => {
            let changes = resident::diff(&s.resident, &res);
            if changes.is_empty() {
                out.note("resident rings unchanged since they were injected; nothing added");
                return Ok(());
            }
            let mut text = String::new();
            text.push_str(
                "# Cyberbrain: rings 0/1 changed since they were injected\n\nThe operator \
                 edited the resident rings during this session. The current text replaces what \
                 you were shown earlier.\n\n",
            );
            for n in &changes.changed {
                text.push_str(&format!(
                    "## Changed: {} `{}` (updated {})\n\n",
                    n.note.front.ring,
                    n.note.front.name,
                    n.note.front.updated.strftime("%Y-%m-%d %H:%M UTC")
                ));
                resident::render_note(n, &mut text);
            }
            for n in &changes.added {
                text.push_str(&format!(
                    "## New: {} `{}`\n\n",
                    n.note.front.ring, n.note.front.name
                ));
                resident::render_note(n, &mut text);
            }
            for key in &changes.removed {
                text.push_str(&format!(
                    "## Removed: `{key}` is no longer in the resident rings (deleted, moved, or now unreadable); what it said no longer applies from this ring.\n\n"
                ));
            }
            if !res.unreadable.is_empty() {
                text.push_str("## Resident files that could not be read\n\n");
                for (p, why) in &res.unreadable {
                    text.push_str(&format!("- {}: {why}\n", Slash(p)));
                }
                text.push('\n');
            }
            out.stdout = text;
            s.resident = resident::marks(&res);
            s.reinjections += 1;
            s.last_injection_at = Some(ctx.now.clone());
            session::save(ctx.root(), &id, &s)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// pre-tool-use / post-tool-use

/// The edited store file, classified, with a display name. `None` when the call is not a
/// file edit or the file is outside the store, with a stderr line saying so.
fn edited_store_file(ctx: &Ctx<'_>, out: &mut HookOutput) -> Option<(StoreTarget, String)> {
    let Some(file) = ctx.payload.edited_file() else {
        out.note(format!(
            "{}: not a file edit ({}); nothing to check",
            event_name(ctx.event),
            ctx.payload.tool_name.as_deref().unwrap_or("no tool_name")
        ));
        return None;
    };
    let cwd = ctx.cwd();
    let Some(target) = paths::classify_host(ctx.root(), &cwd, &file) else {
        out.note(format!(
            "{}: {} is outside the store; nothing to check",
            event_name(ctx.event),
            Slash(&file)
        ));
        return None;
    };
    Some((target, paths::display_inside(ctx.root(), &cwd, &file)))
}

fn pre_tool_use(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    let Some((target, rel)) = edited_store_file(ctx, out) else {
        return Ok(());
    };
    let tool = ctx.payload.tool_name.as_deref().unwrap_or("tool");
    let (decision, reason) = match &target {
        StoreTarget::AuditLog => (
            "deny",
            format!(
                "`{rel}` is the cyberbrain audit log: append-only, hash-chained evidence (SPEC §4, \
                 §12.6). It is never edited; a change would be reported as tampering. Nothing \
                 you need is in it; `cyberbrain policy audit` reads it."
            ),
        ),
        StoreTarget::ResidentNote(ring) => (
            "deny",
            format!(
                "`{rel}` is a ring {} note: {} (SPEC §3.2). The agent does not write rings 0 or 1; \
                 they are the operator's, size-capped and audited on write. Propose the text to the \
                 operator, who can `cyberbrain write --ring {}` it.",
                ring.as_u8(),
                if *ring == Ring::Invariant {
                    "operator invariants"
                } else {
                    "operating protocol"
                },
                ring.as_u8()
            ),
        ),
        StoreTarget::Config => (
            "ask",
            format!(
                "`{rel}` is the cyberbrain configuration. It holds consent and egress settings \
                 (SPEC §12.1), and a key that does not parse stops every command. The operator \
                 edits it; `cyberbrain policy consent` is the command for consent."
            ),
        ),
        StoreTarget::Note(ring) => (
            "ask",
            format!(
                "`{rel}` is a ring {} note. A raw edit skips the PII gate (SPEC §12.4), the audit \
                 row and the reindex that `cyberbrain write --ring {} --name <name>` performs; \
                 the index will answer from the old text until `cyberbrain scan` runs. Prefer \
                 `cyberbrain write`.",
                ring.as_u8(),
                ring.as_u8()
            ),
        ),
        StoreTarget::IndexCache => (
            "ask",
            format!(
                "`{rel}` is the search index, a disposable cache. Nothing in it is edited by \
                 hand: delete it and run `cyberbrain scan` if it is suspect."
            ),
        ),
        StoreTarget::SessionState => (
            "ask",
            format!(
                "`{rel}` is the hooks' own session state. Editing it only confuses the hooks; \
                 it is safe to delete."
            ),
        ),
        StoreTarget::Other => {
            out.note(format!(
                "pre-tool-use: {rel} is inside the store but not a file the hooks guard; allowing"
            ));
            return Ok(());
        }
    };
    if decision == "deny" {
        // A refusal is the most interesting audit row there is (§12.1). Rare, so the
        // fsync it costs is paid only when something was actually stopped.
        ctx.app.policy().audit().record(
            &ctx.actor(),
            AuditAction::PolicyRefusal,
            format!("file:{rel}"),
            json!({
                "tool": tool,
                "target": format!("{target:?}"),
                "session": ctx.payload.session_id,
                "reason": reason,
            }),
        )?;
    }
    out.note(format!("pre-tool-use: {decision} {tool} on {rel}"));
    out.stdout = json!({
        "hookSpecificOutput": {
            "hookEventName": harness_event_name(ctx.event),
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        }
    })
    .to_string();
    Ok(())
}

fn post_tool_use(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    let Some((target, rel)) = edited_store_file(ctx, out) else {
        return Ok(());
    };
    let context = match &target {
        StoreTarget::Note(ring) | StoreTarget::ResidentNote(ring) => format!(
            "cyberbrain: `{rel}` is a ring {} note and was edited directly. The index does not \
             see this edit until `cyberbrain scan` runs; `recall` answers from the old text \
             until then. The frontmatter's `id`, `name` and `ring` must stay consistent with the \
             path or the note becomes unreadable.{}",
            ring.as_u8(),
            if ring.is_resident() {
                " Rings 0/1 are re-injected at your next prompt if their content changed."
            } else {
                ""
            }
        ),
        StoreTarget::Config => format!(
            "cyberbrain: `{rel}` was edited. The configuration is read at the start of every \
             command and a key that does not parse stops all of them; run `cyberbrain status` now."
        ),
        StoreTarget::AuditLog => format!(
            "cyberbrain: `{rel}` was written to by a tool. The audit log is append-only and \
             hash-chained; `cyberbrain policy audit --verify` will now name the first altered \
             row. Tell the operator."
        ),
        StoreTarget::IndexCache | StoreTarget::SessionState | StoreTarget::Other => {
            out.note(format!("post-tool-use: {rel} edited; nothing to add"));
            return Ok(());
        }
    };
    out.note(format!(
        "post-tool-use: {rel} edited; index staleness noted for the model"
    ));
    out.stdout = json!({
        "hookSpecificOutput": {
            "hookEventName": harness_event_name(ctx.event),
            "additionalContext": context,
        }
    })
    .to_string();
    Ok(())
}

// ---------------------------------------------------------------------------------------
// stop / pre-compact

fn stop(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    let Some(id) = ctx.session_id(out) else {
        out.note("stop: no turn recorded");
        return Ok(());
    };
    let mut s = ctx.load_state(&id, out).unwrap_or_else(|| {
        out.note("stop: no session-start record; starting one now");
        SessionState::new(&id, &ctx.now)
    });
    s.turns += 1;
    s.last_stop_at = Some(ctx.now.clone());
    session::save(ctx.root(), &id, &s)?;
    out.note(format!(
        "stop: turn {} recorded in {}{}",
        s.turns,
        Slash(&session::state_path(ctx.root(), &id)),
        if ctx.payload.stop_hook_active {
            "; stop_hook_active is set, and this hook never blocks a stop anyway"
        } else {
            ""
        }
    ));
    Ok(())
}

fn pre_compact(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    let trigger = ctx.payload.trigger.as_deref().unwrap_or("unknown trigger");
    let mut count = 1;
    if let Some(id) = ctx.session_id(out) {
        let mut s = ctx
            .load_state(&id, out)
            .unwrap_or_else(|| SessionState::new(&id, &ctx.now));
        s.compactions += 1;
        s.last_compaction_at = Some(ctx.now.clone());
        s.last_compaction_trigger = Some(trigger.to_string());
        count = s.compactions;
        session::save(ctx.root(), &id, &s)?;
    }
    out.note(format!(
        "pre-compact: compaction #{count} ({trigger}) recorded"
    ));
    // No channel to the model exists here (the harness documents stdout as debug-only for
    // this event); the human gets one line, and session-start(compact) does the rest.
    out.stdout = json!({
        "systemMessage": format!(
            "cyberbrain: compaction #{count} ({trigger}). Rings 0 and 1 are re-injected by the \
             session-start hook right after it. Anything the agent learned this session and did \
             not write with `cyberbrain write` does not survive compaction."
        )
    })
    .to_string();
    Ok(())
}
