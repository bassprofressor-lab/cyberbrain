//! The six events. What each one does, and what it deliberately does not.
//!
//! | event | model sees | does |
//! |---|---|---|
//! | `session-start` | plain stdout | rings 0 and 1 in full with citations, on `startup`/`clear` the newest handoff note (ring 2/3, see `handoff`), a digest of store state, one audit row `session.start`; announces stand-down |
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

use super::governance;
use super::handoff;
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
        // C4 (2026-10-31): a store that is there but cannot be opened left pre-tool-use with
        // an empty answer, which the harness reads as allow. One bad line in cyberbrain.toml
        // thus switched off the store guard, the decoys and AgentGuard for the rest of the
        // session. A call that changes something is put to a person instead; a read goes on.
        if let (HookEvent::PreToolUse, StandDown::Unreadable(_)) = (event, &sd)
            && !super::governance::reads_only_call(&payload)
        {
            let reason = format!(
                "{} While the store cannot be opened, its guard and any governance check \
                 cannot run, so a call that changes something is put to a person.",
                sd.announce()
            );
            out.note(format!("hook {} standing down: ask", event_name(event)));
            out.stdout = json!({
                "hookSpecificOutput": {
                    "hookEventName": harness_event_name(event),
                    "permissionDecision": "ask",
                    "permissionDecisionReason": reason,
                }
            })
            .to_string();
            return Ok(out);
        }
        match event {
            HookEvent::SessionStart | HookEvent::SessionRings => out.stdout = sd.announce(),
            // The first half says it; saying it twice is noise.
            HookEvent::SessionContext => {}
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
        HookEvent::SessionStart => session_start(&ctx, &mut out, Part::Whole)?,
        HookEvent::SessionRings => session_start(&ctx, &mut out, Part::Rings)?,
        HookEvent::SessionContext => session_context(&ctx, &mut out)?,
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

/// Which part of the session-start text a hook prints. Claude Code measures each hook's
/// output on its own against its 10,000 character cap, so the text can be split over two
/// hooks for the same event; the rings come first and alone, so no handoff or digest can
/// push them past the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Whole,
    Rings,
    Context,
}

fn session_start(ctx: &Ctx<'_>, out: &mut HookOutput, part: Part) -> Result<()> {
    let source = ctx.payload.source.as_deref().unwrap_or("unknown");
    let res = resident::read_checked(ctx.app);

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

    let (text, handoff) = session_start_text(ctx.app, source, &res, state.as_ref(), part);
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
            "handoff": handoff.as_ref().map(|h| format!("{}/{}", h.note.front.ring, h.note.front.name)),
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

/// The second half of session start: handoff, digest, usage. Reads the store and the session
/// state, writes nothing: the first half (`session-rings`) owns the state and the audit row,
/// and the harness may run both at once.
fn session_context(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    let source = ctx.payload.source.as_deref().unwrap_or("unknown");
    let res = resident::read_checked(ctx.app);
    // Through `session_id`, which validates the untrusted id before it becomes a path.
    let state = ctx.session_id(out).and_then(|id| ctx.load_state(&id, out));
    out.stdout = session_start_text(ctx.app, source, &res, state.as_ref(), Part::Context).0;
    Ok(())
}

/// The session-start text for `source=startup`, as the hook would print it, without a
/// session, an audit row or any state. For `doctor`'s size check.
pub(crate) fn startup_text(app: &App, part: Part) -> String {
    let res = resident::read_checked(app);
    session_start_text(app, "startup", &res, None, part).0
}

/// The text `session-start` prints on stdout, and the handoff it carried. Reads the store,
/// writes nothing.
fn session_start_text(
    app: &App,
    source: &str,
    res: &Resident,
    state: Option<&SessionState>,
    part: Part,
) -> (String, Option<handoff::Handoff>) {
    let mut text = String::new();
    // The handoff belongs to a session's beginning. After a compaction or on resume the
    // session has its own, newer account of where it stands, and the old handoff would
    // only compete with it. Looked up in every part: the first half audits which one the
    // second half shows.
    let handoff = match source {
        "startup" | "clear" => {
            handoff::latest(app.store(), &app.config().handoff, jiff::Timestamp::now())
        }
        _ => None,
    };
    if part == Part::Context {
        text.push_str("# Cyberbrain memory, continued: handoff and store state\n\n");
        if let Some(h) = &handoff {
            handoff::render(h, &app.config().handoff, &mut text);
        }
        digest(app, res, state, source, &mut text);
        usage(&mut text);
        return (text, handoff);
    }
    text.push_str("# Cyberbrain memory\n\n");
    text.push_str(&format!(
        "Store: `{}`. Rings 0 and 1 below are resident: they apply to everything you do in \
         this project. Every block is prefixed with its citation; quote it when you rely on \
         it, and expand one with `cyberbrain recall --id <citation>`.\n\n",
        Slash(app.root())
    ));
    match source {
        "compact" => {
            let (k, turns) = state.map(|s| (s.compactions, s.turns)).unwrap_or((0, 0));
            text.push_str(&format!(
                "Context was just compacted (compaction #{k} of this session, {turns} turn(s) \
                 so far). Rings 0 and 1 follow again in full. Nothing else from before the \
                 compaction is re-supplied here: what you learned and did not write with \
                 `cyberbrain write` is gone from your context, and still in the store only if \
                 you wrote it.\n\n"
            ));
        }
        "resume" => {
            if let Some(s) = state {
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
    resident::render_rings(res, &mut text);
    if part == Part::Rings {
        text.push_str(
            "The handoff note and the store digest follow in a second message \
             (`session-context`).\n\n",
        );
        return (text, handoff);
    }
    if let Some(h) = &handoff {
        handoff::render(h, &app.config().handoff, &mut text);
    }
    digest(app, res, state, source, &mut text);
    usage(&mut text);
    (text, handoff)
}

fn digest(
    app: &App,
    res: &Resident,
    state: Option<&SessionState>,
    source: &str,
    text: &mut String,
) {
    let store = app.store();
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
    // Proposals, because a proposal nobody is told about is a file in a folder. Counted
    // from the directory rather than the index — they are deliberately not in it — and a
    // directory read is cheap enough for a hook with a 150 ms budget.
    if let Ok(waiting) = store.list_proposals()
        && !waiting.is_empty()
    {
        let names: Vec<&str> = waiting
            .iter()
            .map(|n| n.front.name.as_str())
            .take(5)
            .collect();
        text.push_str(&format!(
            "- **{} proposal(s) waiting** for somebody to accept or reject: {}{}. They are \
             not in the index and `recall` cannot return them; `cyberbrain review` lists \
             them. Tell the operator.\n",
            waiting.len(),
            names.join(", "),
            if waiting.len() > names.len() {
                ", …"
            } else {
                ""
            }
        ));
    }

    let policy = app.policy();
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
    let embedding = &app.config().embedding;
    if let (cyberbrain_core::config::EmbeddingBackend::Ollama, Some(o)) =
        (embedding.backend, embedding.ollama.as_ref())
    {
        text.push_str(&format!(
            "- semantic search: {} via Ollama at {} (not contacted by this hook; `recall` asks \
             it and falls back to lexical search if it does not answer)\n",
            o.model, o.base_url
        ));
    } else {
        let model_dir = app.config().model_dir();
        let model_present = model_dir.join("model.safetensors").is_file()
            && model_dir.join("tokenizer.json").is_file();
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
    }
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
         record. Rings 0 and 1 are the operator's: never write them directly.\n\
         - `cyberbrain propose --ring 0 --kind decision --name <slug> --body '...'` is how you \
         offer one instead. It goes to `proposals/`, outside the notes tree, so it is not \
         indexed and nobody retrieves it as though it were agreed; a person other than the \
         proposer accepts it with `cyberbrain review <name> --accept`.\n\
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
    let res = resident::read_checked(ctx.app);
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
    let lexical = paths::classify_host(ctx.root(), &cwd, &file);
    // Lexically outside, but a link may lead in: `/tmp/cb -> .cyberbrain` made `Write
    // /tmp/cb/audit.db` "outside the store" (C6, 2026-10-31). Only then is the file system
    // asked, so a call inside the store or plainly elsewhere pays nothing extra but a few
    // `stat`s for the path's existing part.
    let resolved = || {
        let root = std::fs::canonicalize(ctx.root()).ok()?;
        let file = resolve_existing_prefix(&cwd.join(&file))?;
        let target = paths::classify_host(&root, &root, &file)?;
        Some((target, paths::display_inside(&root, &root, &file)))
    };
    if let Some(target) = lexical {
        return Some((target, paths::display_inside(ctx.root(), &cwd, &file)));
    }
    if let Some((target, rel)) = resolved() {
        out.note(format!(
            "{}: {} leads into the store through a link, as {rel}",
            event_name(ctx.event),
            Slash(&file)
        ));
        return Some((target, rel));
    }
    out.note(format!(
        "{}: {} is outside the store; nothing to check",
        event_name(ctx.event),
        Slash(&file)
    ));
    None
}

/// `path` with its longest existing ancestor (the path itself, when it exists) resolved
/// through every link, and the part that does not exist yet put back on.
fn resolve_existing_prefix(path: &Path) -> Option<PathBuf> {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            return Some(rest.iter().rev().fold(real, |p, c| p.join(c)));
        }
        rest.push(at.file_name()?);
        at = at.parent()?;
    }
}

/// What a shell command would do to the store that the file tools are already refused.
///
/// 2026-09-25: the matcher named only the file tools, so everything this hook denies for
/// `Edit` went through untouched as `echo … > .cyberbrain/notes/r0/x.md` or `sqlite3
/// audit.db`. And since the CLI now tells an agent from the operator by `CLAUDECODE`
/// (`cli_actor` in main.rs), dropping that variable next to a `cyberbrain` call is the one
/// way left to write ring 0/1 as the operator. Both are matched on the command text: a
/// heuristic that catches the ordinary way of doing it, not a sandbox. Reading stays free.
fn bash_verdict(cmd: &str) -> Option<BashVerdict> {
    bash_verdict_on(paths::Platform::host(), cmd)
}

/// [`bash_verdict`] for a given platform's file names, so each is tested on every host.
fn bash_verdict_on(platform: paths::Platform, cmd: &str) -> Option<BashVerdict> {
    let words: Vec<&str> = cmd
        .split(|c: char| c.is_whitespace() || ";|&()`'\"".contains(c))
        .filter(|t| !t.is_empty())
        .collect();
    // An assignment only as a word of its own (`CLAUDECODE= cyberbrain …`, `export
    // CLAUDECODE=`), and a call only where a word IS the binary: as substrings they also
    // matched `grep '^CLAUDECODE='` and a path through /root/cyberbrain/ — both only look.
    let sheds = words.iter().any(|t| t.starts_with("CLAUDECODE="))
        || [
            "unset CLAUDECODE",
            "-u CLAUDECODE",
            "-uCLAUDECODE",
            "--unset=CLAUDECODE",
            "--unset CLAUDECODE",
            "env -i",
            // C7 (2026-10-31): `exec -c` clears the environment, `unset -v`/`export -n` drop
            // the variable under another spelling, and `env -` is `env -i`.
            "unset -v CLAUDECODE",
            "export -n CLAUDECODE",
            "exec -c",
            "--ignore-environment",
        ]
        .iter()
        .any(|p| cmd.contains(p))
        || words.windows(2).any(|w| w == ["env", "-"])
        // A program that runs the call as another user resets the environment on the way
        // (`sudo`'s env_reset is the default).
        || words
            .iter()
            .any(|w| matches!(*w, "sudo" | "su" | "doas" | "runuser" | "pkexec"));
    let calls = words
        .iter()
        .any(|t| t.rsplit('/').next() == Some("cyberbrain"));
    if sheds && calls {
        return Some(BashVerdict {
            decision: "deny",
            reason: "this command drops CLAUDECODE around a cyberbrain call. The CLI reads that \
             variable to tell an agent from the operator (rings 0 and 1 are the operator's, \
             SPEC §3.2); without it the call would be recorded as the operator. Run it with the \
             variable in place, or propose the text: `cyberbrain propose --ring <n> --kind … \
             --name …`."
                .to_string(),
        });
    }
    // A path is guarded when it names ring 0/1, the audit log or the signing key, or a
    // directory that holds them: the store itself, or its notes/ tree (`rm -rf .cyberbrain`
    // takes all of it). The configuration is asked about rather than refused, as it is for the
    // file tools: the operator edits it, but one bad line stops every hook (C4, 2026-10-31).
    // Backslashes count as separators everywhere (a Windows path in a command reads
    // `.cyberbrain\notes\r0`), and on Windows and macOS case does not count either, as it
    // does not for the file system there.
    let guarded = |t: &str| guard_of(platform, t);
    // Command by command, and within a command only the arguments that program writes to.
    // The first version looked for a writing program and a guarded path anywhere in the
    // line, and refused `rm -rf /tmp/x; tar -cf - .cyberbrain` and `cp -a .cyberbrain /tmp`
    // on its first day — both only read the store.
    let written = cmd
        .split(|c: char| ";|&()\n".contains(c))
        .flat_map(|simple| {
            let words: Vec<&str> = simple
                .split_whitespace()
                .map(|w| w.trim_matches(['\'', '"']))
                .collect();
            write_targets(&words)
        });
    // A redirect only counts when it points into the guarded path, so that
    // `cat notes/r0/x.md 2>/dev/null` stays a read.
    let redirected = cmd.match_indices('>').filter_map(|(i, _)| {
        let rest = cmd[i + 1..].trim_start_matches(['>', '|']).trim_start();
        rest.split(|c: char| c.is_whitespace() || ";|&".contains(c))
            .next()
    });
    // A relative target after `cd`/`pushd` is read against every directory changed into:
    // `cd .cyberbrain/notes && echo x > r0/x.md` (C7, 2026-10-31). Over-reading is fail-safe.
    let dirs: Vec<&str> = cmd
        .split(|c: char| ";|&()\n".contains(c))
        .filter_map(|simple| {
            let mut w = simple
                .split_whitespace()
                .map(|w| w.trim_matches(['\'', '"']));
            matches!(w.next(), Some("cd" | "pushd"))
                .then(|| w.find(|a| !a.starts_with('-')))
                .flatten()
        })
        .collect();
    let mut hits: Vec<(String, &'static str)> = Vec::new();
    for t in written.chain(redirected) {
        if let Some(d) = guarded(t) {
            hits.push((t.to_string(), d));
            continue;
        }
        let relative = !t.starts_with(['/', '~', '$', '\\']) && !t.contains(':');
        for dir in dirs.iter().filter(|_| relative) {
            let joined = format!("{}/{t}", dir.trim_end_matches('/'));
            if let Some(d) = guarded(&joined) {
                hits.push((joined, d));
                break;
            }
        }
    }
    // Programs whose writes are not in their arguments: an interpreter's inline code, a
    // shell's `-c` script, `xargs` handing paths to a writer (C7).
    if let Some(found) = opaque_writes(platform, cmd) {
        hits.push(found);
    }
    // A refusal outranks a question: a command that touches both is refused.
    let (hit, decision) = hits
        .iter()
        .find(|(_, d)| *d == "deny")
        .or(hits.first())
        .cloned()?;
    let hit = hit.trim_matches(['\'', '"']);
    if decision == "ask" {
        return Some(BashVerdict {
            decision,
            reason: format!(
                "this command writes to `{hit}`, the cyberbrain configuration. It holds consent, \
                 egress and governance settings (SPEC §12.1), and a line that does not parse \
                 stops every cyberbrain command, the hooks that guard the store included. The \
                 operator edits it; `cyberbrain policy consent` is the command for consent."
            ),
        });
    }
    Some(BashVerdict {
        decision,
        reason: format!(
            "this command writes to `{hit}` inside the cyberbrain store. The audit log and its \
             signing key are append-only evidence and rings 0 and 1 are the operator's (SPEC \
             §3.2, §12.6); the file tools are refused the same edit. Reading is fine. For a \
             note, use `cyberbrain write` (rings 2–4) or `cyberbrain propose` (rings 0/1); a \
             waiting proposal is changed by rejecting it and proposing again."
        ),
    })
}

/// Whether one word of a command names a guarded store path: `deny` for rings 0/1, the
/// audit log, proposals, the signing key or a directory holding them, `ask` for the
/// configuration. Spelled-around forms count too: `.`/`..`/`//` are folded the way the file
/// system reads them, and a glob counts when what comes before its first wildcard could
/// still lead to a guarded path (`.cyberbrain/n*/r0`, `.c*`) (C7, 2026-10-31).
fn guard_of(platform: paths::Platform, word: &str) -> Option<&'static str> {
    let mut t = word.trim_matches(['\'', '"']).replace('\\', "/");
    if platform.folds_case() {
        t = t.to_lowercase();
    }
    let t = fold_dots(&t);
    // `.cyberbrain/proposals` and `.cyberbrain/keys` only under the default store name: a
    // bare `proposals/` or `keys/` is too common a directory to refuse writes to. The
    // digest check in `review` is what holds wherever the store lives.
    const DENY: [&str; 5] = [
        "notes/r0",
        "notes/r1",
        "audit.db",
        ".cyberbrain/proposals",
        ".cyberbrain/keys",
    ];
    if DENY.iter().any(|g| t.contains(g))
        || t.ends_with(".cyberbrain")
        || t.ends_with(".cyberbrain/notes")
    {
        return Some("deny");
    }
    if t.contains("cyberbrain.toml") {
        return Some("ask");
    }
    let wild = t.find(['*', '?', '[', '{'])?;
    let head = &t[..wild];
    const LEADS: [(&str, &str); 8] = [
        (".cyberbrain/notes/r0", "deny"),
        (".cyberbrain/notes/r1", "deny"),
        (".cyberbrain/audit.db", "deny"),
        (".cyberbrain/proposals", "deny"),
        (".cyberbrain/keys", "deny"),
        (".cyberbrain/cyberbrain.toml", "ask"),
        ("notes/r0", "deny"),
        ("notes/r1", "deny"),
    ];
    let mut verdict = None;
    let starts = std::iter::once(0).chain(head.match_indices('/').map(|(k, _)| k + 1));
    for tail in starts.map(|k| &head[k..]).filter(|t| !t.is_empty()) {
        for (full, d) in LEADS {
            // Without the store's name a lead needs a directory in it: `a*` is not `audit.db`.
            if !full.starts_with(".cyberbrain") && !tail.contains('/') {
                continue;
            }
            if full.starts_with(tail) {
                if d == "deny" {
                    return Some(d);
                }
                verdict = Some(d);
            }
        }
    }
    verdict
}

/// Lexical `.`, `..` and `//` folding of a slash path.
fn fold_dots(t: &str) -> String {
    let absolute = t.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for c in t.split('/') {
        match c {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    let joined = parts.join("/");
    if absolute {
        format!("/{joined}")
    } else {
        joined
    }
}

/// The simple commands of a shell line, split at unquoted `;|&()` and newlines, each as its
/// words with the quotes taken off. Backslashes are kept as they are (a Windows path is full
/// of them). Enough to find a quoted script argument whole; not a shell parser.
fn simple_commands(cmd: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in cmd.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                in_word = true;
            }
            ';' | '|' | '&' | '(' | ')' | '\n' => {
                if in_word {
                    current.push(std::mem::take(&mut word));
                    in_word = false;
                }
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    current.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        current.push(word);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Writes a command makes that its arguments do not show: a shell's `-c` script (judged
/// like any command line), an interpreter's inline code (any guarded path in it counts), and
/// `xargs` handing paths to a writing program (any guarded path in the whole line counts).
fn opaque_writes(platform: paths::Platform, cmd: &str) -> Option<(String, &'static str)> {
    let commands = simple_commands(cmd);
    let scan = |text: &str| {
        text.split(|c: char| c.is_whitespace() || "'\"(),;=`".contains(c))
            .filter(|t| !t.is_empty())
            .find_map(|t| guard_of(platform, t).map(|d| (t.to_string(), d)))
    };
    for words in &commands {
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let Some((prog, args)) = program(&words) else {
            continue;
        };
        let after = |flags: &[&str]| {
            args.iter()
                .position(|a| flags.contains(a))
                .and_then(|i| args.get(i + 1))
                .copied()
        };
        let found = match prog {
            "sh" | "bash" | "zsh" | "dash" | "ksh" => after(&["-c"])
                .and_then(|script| bash_verdict_on(platform, script))
                .map(|v| ("a shell script".to_string(), v.decision)),
            "eval" => bash_verdict_on(platform, &args.join(" "))
                .map(|v| ("an eval".to_string(), v.decision)),
            p if p.starts_with("python") => after(&["-c"]).and_then(scan),
            "perl" => args
                .iter()
                .position(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(['e', 'E']))
                .and_then(|i| args.get(i + 1))
                .and_then(|code| scan(code)),
            "node" | "deno" | "bun" => after(&["-e", "--eval", "-p", "--print"]).and_then(scan),
            "ruby" => after(&["-e"]).and_then(scan),
            "php" => after(&["-r"]).and_then(scan),
            "xargs" => args
                .iter()
                .any(|a| WRITERS.contains(&a.rsplit('/').next().unwrap_or(a)))
                .then(|| commands.iter().flatten().find_map(|w| scan(w)))
                .flatten(),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The programs `write_targets` knows to write, for `xargs`.
const WRITERS: [&str; 15] = [
    "rm", "rmdir", "unlink", "shred", "truncate", "mv", "tee", "sqlite3", "cp", "rsync", "install",
    "ln", "sed", "perl", "dd",
];

/// The program of one simple command and its arguments, past leading assignments and
/// wrappers (`sudo`, `env`, `nohup`, …).
fn program<'w, 'a>(words: &'w [&'a str]) -> Option<(&'a str, &'w [&'a str])> {
    let mut i = 0;
    while let Some(w) = words.get(i) {
        let wrapper = matches!(*w, "sudo" | "env" | "command" | "exec" | "nohup" | "time");
        let assignment = w.contains('=') && !w.starts_with('-');
        let wrapper_flag = i > 0 && w.starts_with('-') && words[i - 1] == "env";
        if !(wrapper || assignment || wrapper_flag) {
            break;
        }
        i += 1;
    }
    let prog = words.get(i)?;
    Some((prog.rsplit('/').next().unwrap_or(prog), &words[i + 1..]))
}

/// What [`bash_verdict`] decided about a shell command, and why.
#[derive(Debug)]
struct BashVerdict {
    /// `deny` or `ask`.
    decision: &'static str,
    reason: String,
}

/// Whether one argument of `sed` or `perl` asks for in-place editing.
///
/// `-i` bundles with other short options (`sed -ni`, `perl -pi`, `perl -0777 -pi`), so a short
/// cluster is read letter by letter until a letter that takes an argument: after that, the
/// rest of the cluster is that argument (`perl -Mstrict` names a module, not `-i`).
fn edits_in_place(prog: &str, arg: &str) -> bool {
    if arg == "--in-place" || arg.starts_with("--in-place=") {
        return true;
    }
    let Some(cluster) = arg.strip_prefix('-').filter(|c| !c.starts_with('-')) else {
        return false;
    };
    for c in cluster.chars() {
        if c == 'i' {
            return true;
        }
        // The letters that take an argument differ: `sed -E` is a flag, `perl -E` a script.
        let takes_argument = match prog {
            "sed" => matches!(c, 'e' | 'f' | 'l'),
            _ => {
                matches!(
                    c,
                    'e' | 'E' | 'M' | 'm' | 'I' | 'l' | 'x' | 'C' | 'd' | 'D' | 'F'
                ) || c.is_ascii_digit()
            }
        };
        if takes_argument {
            return false;
        }
    }
    false
}

/// The arguments of one simple command that it writes to, deletes or replaces.
///
/// Leading assignments and wrappers (`sudo`, `env`, `nohup`, …) are skipped to reach the
/// program. Removers and movers write every path they name; copiers only their last one,
/// the destination; `sed -i`/`perl -i` their files; `dd` its `of=`; an extracting `tar`
/// every path, since `-C` names where it writes. Anything else writes nothing we can see.
fn write_targets<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let Some((prog, args)) = program(words) else {
        return Vec::new();
    };
    let paths = || args.iter().copied().filter(|a| !a.starts_with('-'));
    // `-t DIR` names the destination of a copy wherever it stands (C7, 2026-10-31).
    let target_dir = args
        .iter()
        .position(|a| *a == "-t" || *a == "--target-directory")
        .and_then(|i| args.get(i + 1))
        .copied()
        .or_else(|| {
            args.iter()
                .find_map(|a| a.strip_prefix("--target-directory="))
        });
    match prog {
        "rm" | "rmdir" | "unlink" | "shred" | "truncate" | "mv" | "tee" | "sqlite3" => {
            paths().collect()
        }
        // A link names what it points at as well: `ln -s .cyberbrain /tmp/cb` makes every
        // guarded file writable under a path the file tools do not know (C6, 2026-10-31).
        "ln" => paths().collect(),
        "cp" | "rsync" | "install" => target_dir
            .or_else(|| paths().next_back())
            .into_iter()
            .collect(),
        "git" => git_targets(args),
        // `find` writes through `-delete` and `-exec`; what it writes to is below its
        // starting points, the arguments before the first expression.
        "find"
            if args
                .iter()
                .any(|a| matches!(*a, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir")) =>
        {
            args.iter()
                .copied()
                .take_while(|a| !a.starts_with(['-', '(', '!']))
                .collect()
        }
        "unzip" => args
            .iter()
            .position(|a| *a == "-d")
            .and_then(|i| args.get(i + 1))
            .copied()
            .into_iter()
            .collect(),
        "sed" | "perl" if args.iter().any(|a| edits_in_place(prog, a)) => paths().collect(),
        "dd" => args.iter().filter_map(|a| a.strip_prefix("of=")).collect(),
        "tar"
            if args.iter().any(|a| {
                *a == "--extract" || (a.starts_with('-') && !a.starts_with("--") && a.contains('x'))
            }) =>
        {
            args.to_vec()
        }
        _ => Vec::new(),
    }
}

/// The paths a `git` subcommand writes: the files `checkout`, `restore`, `rm` and `mv` name,
/// and for `clean -x`/`-X`, which removes ignored files, the store itself unless an `-e`
/// pattern keeps it out (C7, 2026-10-31).
fn git_targets<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut i = 0;
    // `-C dir` and `-c key=value` come before the subcommand and take a value.
    while let Some(a) = args.get(i) {
        match *a {
            "-C" | "-c" => i += 2,
            a if a.starts_with('-') => i += 1,
            _ => break,
        }
    }
    let Some(sub) = args.get(i) else {
        return Vec::new();
    };
    let rest = &args[i + 1..];
    match *sub {
        "checkout" | "restore" | "rm" | "mv" => rest
            .iter()
            .copied()
            .filter(|a| !a.starts_with('-'))
            .collect(),
        "clean" => {
            let ignored = rest
                .iter()
                .any(|a| a.starts_with('-') && !a.starts_with("--") && a.contains(['x', 'X']));
            let mut kept = false;
            let mut paths = Vec::new();
            let mut it = rest.iter().copied();
            while let Some(a) = it.next() {
                let pattern = match a {
                    "-e" | "--exclude" => it.next(),
                    a => a
                        .strip_prefix("--exclude=")
                        .or_else(|| a.strip_prefix("-e"))
                        .filter(|p| !p.is_empty()),
                };
                match pattern {
                    Some(p) => kept |= p.trim_matches('/').ends_with(".cyberbrain"),
                    None if !a.starts_with('-') => paths.push(a),
                    None => {}
                }
            }
            if ignored && !kept {
                paths.push(".cyberbrain");
            }
            paths
        }
        _ => Vec::new(),
    }
}

/// Record a refusal the guard is about to give, without letting the record decide it.
///
/// Every other audited step fails closed by not doing what it could not record. Here the
/// step is a *refusal*, and an error out of the hook is an empty answer, which the harness
/// reads as "no decision" and allows: a full disk or a locked `audit.db` would open the guard.
/// So the deny stands and the missing row is said on stderr instead.
fn record_refusal(ctx: &Ctx<'_>, out: &mut HookOutput, subject: String, detail: serde_json::Value) {
    if let Err(e) =
        ctx.app
            .policy()
            .audit()
            .record(&ctx.actor(), AuditAction::PolicyRefusal, subject, detail)
    {
        out.note(format!(
            "pre-tool-use: the refusal below stands, but it could not be recorded in the \
             audit log: {e}"
        ));
    }
}

fn pre_bash(ctx: &Ctx<'_>, out: &mut HookOutput, cmd: &str) -> Result<()> {
    let Some(BashVerdict { decision, reason }) = bash_verdict(cmd) else {
        out.note("pre-tool-use: Bash command does not touch guarded store files; allowing");
        return Ok(());
    };
    if decision == "deny" {
        // The command itself is not recorded: it may carry anything, a secret included.
        record_refusal(
            ctx,
            out,
            "bash".to_string(),
            json!({ "tool": "Bash", "session": ctx.payload.session_id, "reason": reason }),
        );
    }
    out.note(format!(
        "pre-tool-use: {decision} Bash on guarded store files"
    ));
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

/// The store's own guard first; when it has nothing to say, the governance service
/// (`governance.rs`) is asked about the call.
fn pre_tool_use(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    // A decoy in a tool call comes first: it is refused on this machine before the call is
    // shown to anybody else, the governance service included.
    if canary_guard(ctx, out) {
        return Ok(());
    }
    store_guard(ctx, out)?;
    if !out.stdout.is_empty() {
        return Ok(()); // already denied or asked by the store's guard
    }
    match governance::check(ctx.app, ctx.payload, &ctx.actor()) {
        governance::Verdict::Proceed(note) => out.note(format!("pre-tool-use: {note}")),
        governance::Verdict::Decide(decision, reason) => {
            out.note(format!("pre-tool-use: governance {decision}"));
            out.stdout = json!({
                "hookSpecificOutput": {
                    "hookEventName": harness_event_name(ctx.event),
                    "permissionDecision": decision,
                    "permissionDecisionReason": reason,
                }
            })
            .to_string();
        }
    }
    Ok(())
}

/// Deny a tool call whose input carries one of this store's decoys. Recorded twice, as a
/// refusal and as a `canary.hit`, both by fingerprint; the input itself is in neither.
fn canary_guard(ctx: &Ctx<'_>, out: &mut HookOutput) -> bool {
    let register = crate::canary::load(ctx.app.root()).unwrap_or_default();
    if register.is_empty() {
        return false;
    }
    let input = ctx.payload.tool_input.to_string();
    let Some(c) = crate::canary::find(&register, &input) else {
        return false;
    };
    let tool = ctx.payload.tool_name.as_deref().unwrap_or("tool");
    let reason = crate::canary::refusal(c);
    crate::canary::record_hit(
        ctx.app,
        c,
        &format!("tool:{tool}"),
        ctx.payload.session_id.as_deref(),
    );
    record_refusal(
        ctx,
        out,
        format!("canary:{}", c.fingerprint),
        json!({ "tool": tool, "session": ctx.payload.session_id, "reason": reason }),
    );
    out.note(format!(
        "pre-tool-use: deny {tool}: canary {}",
        c.fingerprint
    ));
    out.stdout = json!({
        "hookSpecificOutput": {
            "hookEventName": harness_event_name(ctx.event),
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
    .to_string();
    true
}

fn store_guard(ctx: &Ctx<'_>, out: &mut HookOutput) -> Result<()> {
    if let Some(cmd) = ctx.payload.bash_command() {
        return pre_bash(ctx, out, cmd);
    }
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
        StoreTarget::Proposal => (
            "deny",
            format!(
                "`{rel}` is a proposal waiting for review. Its text is what the reviewer \
                 approves under the proposer's name, so it is not edited in place; `review \
                 --accept` would refuse it anyway, since the file no longer matches what was \
                 proposed. To change it: `cyberbrain review <name> --reject --reason …` and \
                 propose again."
            ),
        ),
        StoreTarget::SigningKey => (
            "deny",
            format!(
                "`{rel}` is the audit log's signing key. Every audit row is signed with it \
                 (SPEC §12.6); replaced or removed, later rows are unsigned or signed by \
                 somebody else, and the chain no longer proves who wrote it. The operator \
                 manages it with `cyberbrain policy audit-key`."
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
        record_refusal(
            ctx,
            out,
            format!("file:{rel}"),
            json!({
                "tool": tool,
                "target": format!("{target:?}"),
                "session": ctx.payload.session_id,
                "reason": reason,
            }),
        );
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
        StoreTarget::Proposal => format!(
            "cyberbrain: `{rel}` is a waiting proposal and was changed directly. `review \
             --accept` will refuse it now; reject it and propose again."
        ),
        StoreTarget::SigningKey => format!(
            "cyberbrain: `{rel}`, the audit log's signing key, was written to by a tool. Rows \
             from now on may be signed by another key or not at all; `cyberbrain doctor` checks \
             the chain. Tell the operator."
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

#[cfg(test)]
mod bash_tests {
    use super::bash_verdict;

    #[test]
    fn shell_writes_into_rings_0_1_and_the_audit_log_are_refused() {
        for cmd in [
            "echo x > .cyberbrain/notes/r0/identity.md",
            "echo x >> /root/orderflow/.cyberbrain/notes/r1/engine.md",
            "printf x | tee .cyberbrain/notes/r0/a.md",
            "sed -i 's/a/b/' .cyberbrain/notes/r1/engine.md",
            "cp /tmp/x.md .cyberbrain/notes/r0/x.md",
            "rm -rf .cyberbrain/notes/r0",
            "sqlite3 .cyberbrain/audit.db 'delete from audit'",
            "cd .cyberbrain && mv /tmp/a audit.db",
            "rm -rf .cyberbrain",
            "rm -rf /root/orderflow/.cyberbrain/",
            "mv .cyberbrain/notes /tmp/weg",
            "echo x > '.cyberbrain/notes/r0/a.md'",
            "sudo rm -f .cyberbrain/audit.db",
            "cp /tmp/sicherung.db .cyberbrain/audit.db",
            "tar -C .cyberbrain/notes -xf /tmp/n.tar",
            "dd if=/dev/zero of=.cyberbrain/audit.db bs=1 count=1",
        ] {
            assert!(bash_verdict(cmd).is_some(), "{cmd}");
        }
    }

    /// In-place editing hides in bundled short options as often as it stands alone:
    /// `sed -ni`, `perl -pi -e`, `perl -0777 -pi`, `sed --in-place`. Only `-i` was seen.
    #[test]
    fn in_place_edits_in_bundled_options_are_refused() {
        for cmd in [
            "sed -ni 's/a/b/p' .cyberbrain/notes/r1/engine.md",
            "sed -Ei.bak 's/a/b/' .cyberbrain/notes/r0/a.md",
            "sed --in-place 's/a/b/' .cyberbrain/notes/r1/engine.md",
            "sed --in-place=.bak 's/a/b/' .cyberbrain/notes/r1/engine.md",
            "perl -pi -e 's/a/b/' .cyberbrain/notes/r1/engine.md",
            "perl -0777 -pi -e 's/a/b/' .cyberbrain/notes/r1/engine.md",
        ] {
            assert!(bash_verdict(cmd).is_some(), "{cmd}");
        }
        // Reading with options that merely contain an `i` after an argument letter.
        for cmd in [
            "sed -n 's/a/b/p' .cyberbrain/notes/r1/engine.md",
            "sed -e 's/i/j/' .cyberbrain/notes/r1/engine.md",
            "perl -Mstrict -ne 'print' .cyberbrain/notes/r1/engine.md",
        ] {
            assert!(bash_verdict(cmd).is_none(), "{cmd}");
        }
    }

    /// A waiting proposal is approved as written; the shell is refused the edit the file
    /// tools are (2026-10-03). Reading and listing stay free.
    #[test]
    fn shell_writes_into_waiting_proposals_are_refused() {
        for cmd in [
            "echo x >> .cyberbrain/proposals/friday-freeze.md",
            "sed -i 's/Friday/Monday/' /root/orderflow/.cyberbrain/proposals/friday-freeze.md",
            "cp /tmp/x.md .cyberbrain/proposals/friday-freeze.md",
            r"echo x > C:\proj\.cyberbrain\proposals\x.md",
        ] {
            assert!(bash_verdict(cmd).is_some(), "{cmd}");
        }
        for cmd in [
            "cat .cyberbrain/proposals/friday-freeze.md",
            "ls .cyberbrain/proposals",
            "rm -rf /tmp/proposals/x",
        ] {
            assert!(bash_verdict(cmd).is_none(), "{cmd}");
        }
    }

    /// On Windows a path in a command is as likely to use backslashes as slashes, and the
    /// store guard only knew slashes: `echo x > C:\…\notes\r0\x.md` went through
    /// (windows CI, `the_stores_own_guard_decides_first_…`, 0.7.0 and 0.7.1).
    #[test]
    fn backslash_paths_are_guarded_like_slash_paths() {
        for cmd in [
            r"echo x > C:\Users\a\proj\.cyberbrain\notes\r0\x.md",
            r"echo x >> .cyberbrain\notes\r1\engine.md",
            r"rm -rf .cyberbrain\notes\r1",
            r"cp C:\tmp\x.md .cyberbrain\notes\r0\x.md",
            r"rm -rf C:\proj\.cyberbrain\",
            r"mv .cyberbrain\notes C:\tmp\weg",
            r"sqlite3 C:\proj\.cyberbrain\audit.db 'delete from audit'",
            r"echo x > C:/proj/.cyberbrain\notes\r0/x.md",
        ] {
            assert!(bash_verdict(cmd).is_some(), "{cmd}");
        }
        for cmd in [
            r"cat .cyberbrain\notes\r0\identity.md",
            r"cp .cyberbrain\notes\r2\x.md C:\tmp\",
            r"echo hi > C:\tmp\x",
        ] {
            assert!(bash_verdict(cmd).is_none(), "{cmd}");
        }
    }

    /// C5 (review 2026-10-31): APFS ignores case by default, and the guard on a Mac did not:
    /// `rm .cyberbrain/Audit.db` removed the audit log and was allowed.
    #[test]
    fn on_a_mac_the_guard_ignores_case() {
        use super::{bash_verdict_on, paths::Platform};
        for cmd in [
            "rm .cyberbrain/Audit.db",
            "echo x > .cyberbrain/notes/R0/x.md",
            "echo x > /Users/a/p/.CYBERBRAIN/notes/r1/x.md",
            "rm -rf .Cyberbrain",
        ] {
            assert!(bash_verdict_on(Platform::Mac, cmd).is_some(), "{cmd}");
        }
        // Linux file systems keep case, and so does the guard there.
        assert!(bash_verdict_on(Platform::Unix, "rm .cyberbrain/Audit.db").is_none());
    }

    /// Windows file names do not care about case, so neither does the guard there.
    #[cfg(windows)]
    #[test]
    fn on_windows_the_guard_ignores_case() {
        assert!(bash_verdict(r"echo x > C:\P\.CYBERBRAIN\NOTES\R0\x.md").is_some());
        assert!(bash_verdict(r"echo x > .Cyberbrain\Audit.db").is_some());
    }

    #[test]
    fn dropping_the_agent_marker_around_cyberbrain_is_refused() {
        for cmd in [
            "env -u CLAUDECODE cyberbrain write --ring 0 --kind decision --name x --body y",
            "unset CLAUDECODE; cyberbrain review x --accept --by me",
            "CLAUDECODE= /usr/local/bin/cyberbrain write --ring 1 --name x --body y",
            "env -i PATH=$PATH cyberbrain write --ring 0 --name x --body y",
            "export CLAUDECODE=; cyberbrain write --ring 0 --name x --body y",
            "sh -c 'unset CLAUDECODE; cyberbrain write --ring 0 --name x --body y'",
        ] {
            assert!(bash_verdict(cmd).is_some(), "{cmd}");
        }
    }

    /// C4 (review 2026-10-31): the shell could break `cyberbrain.toml` (after which every
    /// hook stood down, store guard and governance included) or delete the signing key, and
    /// neither was guarded. The file tools were already asked about the configuration.
    #[test]
    fn shell_writes_to_the_config_and_the_signing_key_are_stopped() {
        for cmd in [
            "echo 'x = ' >> .cyberbrain/cyberbrain.toml",
            "sed -i 's/enforce/shadow/' /root/p/.cyberbrain/cyberbrain.toml",
            "cp /tmp/c.toml .cyberbrain/cyberbrain.toml",
            r"echo x > C:\proj\.cyberbrain\cyberbrain.toml",
        ] {
            assert_eq!(bash_verdict(cmd).map(|v| v.decision), Some("ask"), "{cmd}");
        }
        for cmd in [
            "rm -rf .cyberbrain/keys",
            "rm .cyberbrain/keys/audit.ed25519",
            "mv /tmp/other.ed25519 /root/p/.cyberbrain/keys/audit.ed25519",
            "echo x > .cyberbrain/keys/audit.ed25519",
        ] {
            assert_eq!(bash_verdict(cmd).map(|v| v.decision), Some("deny"), "{cmd}");
        }
        for cmd in [
            "cat .cyberbrain/cyberbrain.toml",
            "ls -la .cyberbrain/keys",
            "cp .cyberbrain/cyberbrain.toml /tmp/",
            "rm -rf /tmp/keys",
        ] {
            assert!(bash_verdict(cmd).is_none(), "{cmd}");
        }
    }

    /// C6 (review 2026-10-31): `ln` was read like `cp`, by its last argument only, so a link
    /// to the store from elsewhere went through; the file tools then saw "outside the store".
    #[test]
    fn linking_the_store_or_its_guarded_files_elsewhere_is_refused() {
        for cmd in [
            "ln -s \"$PWD/.cyberbrain\" /tmp/cb",
            "ln -s /root/p/.cyberbrain/notes/r0 /tmp/r0",
            "ln /root/p/.cyberbrain/audit.db /tmp/log.db",
            "ln -sf .cyberbrain/proposals /tmp/p",
        ] {
            assert!(bash_verdict(cmd).is_some(), "{cmd}");
        }
        assert!(bash_verdict("ln -s .cyberbrain/notes/r2/x.md /tmp/x.md").is_none());
    }

    /// C7 (review 2026-10-31): the cheap ways around the heuristic, each one verified to go
    /// through before. The heuristic is still not a sandbox; these are the ordinary spellings.
    #[test]
    fn the_cheap_ways_around_the_shell_guard_are_closed() {
        for cmd in [
            // Path spellings that do not contain the guarded text as it is written.
            "echo x > .cyberbrain/notes/./r0/x.md",
            "echo x > .cyberbrain/notes//r0/x.md",
            "echo x > .cyberbrain/notes/r2/../r0/x.md",
            "rm .cyberbrain/n*/r0/x.md",
            "rm .cyberbrain/notes/r[0]/x.md",
            "rm .cyberbrain/notes/{r0,r1}/x.md",
            "rm -rf .cyberbrain/a*",
            "rm -rf .c*",
            // A directory changed into first.
            "cd .cyberbrain/notes && echo x > r0/x.md",
            "cd /root/p/.cyberbrain; rm audit*",
            "pushd .cyberbrain/notes; rm -rf r1",
            // A target directory given by option.
            "cp -t .cyberbrain/notes/r0 /tmp/x.md",
            "cp --target-directory=.cyberbrain/notes/r0 /tmp/x.md",
            "mv -t .cyberbrain/notes/r1 /tmp/x.md",
            "install -t .cyberbrain/keys /tmp/k",
            // Programs that write and were not known to.
            "git checkout -- .cyberbrain/notes/r0/x.md",
            "git restore .cyberbrain/notes/r1/x.md",
            "git clean -fdx",
            "git clean -fX",
            "find .cyberbrain -delete",
            "find .cyberbrain/notes/r0 -name '*.md' -exec rm {} +",
            "find .cyberbrain/notes -type f | xargs rm",
            "ls .cyberbrain/proposals/* | xargs -0 rm -f",
            "python3 -c \"open('.cyberbrain/audit.db','w')\"",
            "python3 -c \"import shutil; shutil.rmtree('.cyberbrain')\"",
            "perl -e 'unlink q(.cyberbrain/audit.db)'",
            "node -e \"require('fs').rmSync('.cyberbrain/notes/r0',{recursive:true})\"",
            "sh -c 'rm .cyberbrain/notes/r0/x.md'",
            "bash -c \"echo x >> .cyberbrain/notes/r1/x.md\"",
            "unzip -o x.zip -d .cyberbrain/notes/r0",
            // The agent marker dropped in ways the patterns did not know.
            "exec -c cyberbrain write --ring 0 --kind decision --name x --body y",
            "sudo cyberbrain write --ring 0 --kind decision --name x --body y",
            "unset -v CLAUDECODE; cyberbrain write --ring 0 --name x --body y",
            "export -n CLAUDECODE; cyberbrain write --ring 0 --name x --body y",
            "env - PATH=/usr/bin cyberbrain write --ring 0 --name x --body y",
            "doas cyberbrain write --ring 1 --name x --body y",
        ] {
            assert!(bash_verdict(cmd).is_some(), "{cmd}");
        }
        // What the new rules must not catch.
        for cmd in [
            "rm .cyberbrain/notes/r2/*.md",
            "rm -rf build/* target/*",
            "rm a*",
            "cd .cyberbrain/notes && cat r0/x.md",
            "cd /tmp && rm -rf x",
            "cp -t /tmp .cyberbrain/notes/r0/x.md",
            "git checkout main",
            "git restore src/main.rs",
            "git clean -fd",
            "git clean -fdx -e .cyberbrain",
            "find . -name '*.rs' -newer Cargo.toml",
            "find .cyberbrain/notes/r0 -name '*.md'",
            "find /tmp/x -delete",
            "ls | xargs wc -l",
            "python3 -c 'print(1)'",
            "python3 scripts/x.py --out /tmp/y",
            "sudo systemctl restart nginx",
            "unzip -o x.zip -d /tmp/x",
        ] {
            assert!(bash_verdict(cmd).is_none(), "{cmd}");
        }
    }

    #[test]
    fn reads_and_unrelated_commands_pass() {
        for cmd in [
            "cat .cyberbrain/notes/r0/identity.md 2>/dev/null",
            "grep -rn foo .cyberbrain/notes/r1 > /tmp/out.txt",
            "ls -la .cyberbrain/notes/r0",
            "cyberbrain write --ring 2 --kind lesson --name x --body y",
            "cyberbrain recall 'notes/r0 audit.db'",
            "env -u CLAUDECODE claude -p hello",
            "echo hi > /tmp/x",
            "cp -r .cyberbrain/notes/r2/x.md /tmp/",
            "rm -rf /tmp/cbprobe/.cyberbrain2",
            // The two false alarms of the first day (2026-09-25): looking for the variable,
            // and a path through the source tree that only mentions the patterns.
            "printenv | grep -c '^CLAUDECODE='; cyberbrain status",
            "grep -n 'env -i PATH' /root/cyberbrain/crates/cyberbrain/src/hook/events.rs",
            // Reading the store out of itself (a copy for a benchmark, a backup).
            "cp -a /root/orderflow/.cyberbrain /tmp/kopie",
            "rm -rf /tmp/p && tar -C /root/orderflow -cf - .cyberbrain | tar -C /tmp/p -xf -",
            "rsync -a .cyberbrain/ /backup/cb/",
        ] {
            assert!(bash_verdict(cmd).is_none(), "{cmd}");
        }
    }
}
