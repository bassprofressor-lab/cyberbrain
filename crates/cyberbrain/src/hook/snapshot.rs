//! `cyberbrain hook pre-compact-snapshot`: the conversation as it stood before a compaction.
//!
//! The shell `pre-compact` hook only counts: the harness gives it the trigger and nothing of
//! the conversation, and no channel to the summary. Claude Code's function hooks do both. The
//! mod `install --compact-mod` writes (`install/compact_mod/register.ts`) hands this command
//! the transcript on `session.compact`, over stdin, and passes what it prints on to the
//! summarizer as instructions. So this command does two things:
//!
//! - **Keeps a snapshot** in ring 3 (session records, SPEC §3.2), kind `session`, retention
//!   [`RETENTION`], one note per session and compaction (a second call for the same
//!   compaction, as `precompute` makes, replaces the first). Through [`App::write`], so the
//!   PII gate (§12.4) and the audit chain (§12.6) apply as for any write; a hold is answered
//!   with `redact`, the one choice a hook may make on its own. Bounded: the first request,
//!   the newest messages' text cut to a few hundred words each, the citations used and the
//!   names of the tools called. Tool inputs and outputs are never written, nor the reminders
//!   the harness puts into a prompt (rings 0 and 1 among them, which are in the store already).
//! - **Answers with instructions** for the summary: keep the citations the conversation used,
//!   the open points of ring 1 and the snapshot's own citation. `session-start` on `compact`
//!   names the snapshot again afterwards.
//!
//! The mod calls `next` whatever this prints; here every failure ends as a stderr line and an
//! answer without a snapshot, never as an error, so even a store that cannot be written still
//! gets its citations into the summary.

use super::HookOutput;
use super::payload::Payload;
use super::resident;
use super::session::{self, SessionState, SnapshotMark};
use crate::app::{App, WriteOutcome, WriteRequest};
use cyberbrain_core::blocks::{MAX_BLOCK_TOKENS, blocks_of};
use cyberbrain_core::{NoteKind, Result, Ring};
use cyberbrain_policy::write_gate::OperatorChoice;
use serde::Deserialize;
use serde_json::{Value, json};

/// Wall time this event may take before the hook says it was slow. It runs while the harness
/// compacts, which takes far longer, and it may load the embedder to index the note; the
/// 15 ms of the hot path are for events that fire on every tool call.
pub const BUDGET_MS: u128 = 10_000;
/// The snapshot's body, at most. About 4,000 tokens.
pub const MAX_BODY_BYTES: usize = 16 * 1024;
/// What is added to the summarizer's instructions, at most.
pub const MAX_INSTRUCTIONS_BYTES: usize = 4 * 1024;
/// How long a snapshot is kept. It is a record of a session, useful for days, and its text is
/// what people said to an agent; `policy retention --apply` erases it after this.
pub const RETENTION: &str = "P30D";
pub const TAG: &str = "compact-snapshot";

const USER_CHARS: usize = 1_200;
const ASSISTANT_CHARS: usize = 1_500;
const MAX_CITATIONS: usize = 30;
const MAX_OPEN_POINTS: usize = 8;
const OPEN_POINT_CHARS: usize = 200;
const TOOL_TAIL: usize = 20;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Input {
    messages: Vec<Message>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Message {
    role: String,
    text: String,
    /// The mod's shape: tool names.
    tools: Vec<String>,
    /// The engine's own shape (`SessionMessage.toolUses`), should a caller pass it through:
    /// only the names are read.
    #[serde(rename = "toolUses")]
    tool_uses: Vec<ToolUse>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ToolUse {
    tool: String,
}

impl Message {
    fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.tools
            .iter()
            .map(String::as_str)
            .chain(self.tool_uses.iter().map(|t| t.tool.as_str()))
            .filter(|t| !t.trim().is_empty())
    }
}

pub(super) fn run(
    app: &App,
    payload: &Payload,
    stdin: &str,
    now: &str,
    out: &mut HookOutput,
) -> Result<()> {
    let messages = match serde_json::from_str::<Value>(stdin.trim())
        .ok()
        .and_then(|v| serde_json::from_value::<Input>(v).ok())
    {
        Some(i) => i.messages,
        None => {
            out.note("pre-compact-snapshot: no messages in the payload; nothing kept");
            Vec::new()
        }
    };
    let cleaned: Vec<(String, String)> = messages
        .iter()
        .map(|m| (m.role.clone(), clean(&m.text)))
        .collect();
    let citations = citations_newest_first(cleaned.iter().map(|(_, t)| t.as_str()));
    let open = open_points(app);

    let snapshot = keep(app, payload, &messages, &cleaned, &citations, now, out);
    let instructions = instructions(&citations, &open, snapshot.as_ref());
    out.stdout = json!({
        "instructions": instructions,
        "snapshot": snapshot.as_ref().map(|s| json!({ "name": s.name, "citation": s.citation })),
    })
    .to_string();
    Ok(())
}

/// The line `session-start` adds after a compaction this mod saw.
pub(super) fn after_compaction(state: &SessionState) -> Option<String> {
    let s = state.last_snapshot.as_ref()?;
    (s.compaction == state.compactions && !s.citation.is_empty()).then(|| {
        format!(
            "Before it, the conversation was kept as the note `{}` [{}]: the first request, the \
             last messages and the citations used. `cyberbrain recall --id {}` opens it; read it \
             when the summary leaves out something you need.\n\n",
            s.name, s.citation, s.citation
        )
    })
}

/// Write the snapshot and remember it in the session state. `None`, with a stderr line, for
/// every reason it was not kept.
fn keep(
    app: &App,
    payload: &Payload,
    messages: &[Message],
    cleaned: &[(String, String)],
    citations: &[String],
    now: &str,
    out: &mut HookOutput,
) -> Option<SnapshotMark> {
    if payload.agent_id.is_some() {
        out.note("pre-compact-snapshot: a subagent's transcript; no snapshot kept");
        return None;
    }
    if messages.is_empty() {
        return None;
    }
    let Some(id) = payload.session_id.as_deref().and_then(session::safe_id) else {
        out.note("pre-compact-snapshot: no usable session_id; no snapshot kept");
        return None;
    };
    let mut state = match session::load(app.root(), &id) {
        Ok(s) => s.unwrap_or_else(|| SessionState::new(&id, now)),
        Err(e) => {
            out.note(format!("{e}; starting the session state afresh"));
            SessionState::new(&id, now)
        }
    };
    let compaction = state.compactions + 1;
    let name = format!("{TAG}-{}-{compaction}", slug(&id));
    let trigger = payload.trigger.as_deref().unwrap_or("unknown");
    let body = body(&id, compaction, trigger, now, messages, cleaned, citations);

    let req = WriteRequest {
        ring: Ring::Session,
        kind: NoteKind::Session,
        name: name.clone(),
        body,
        tags: vec![TAG.to_string()],
        bereich: None,
        retention: Some(Some(RETENTION.to_string())),
        supersedes: None,
        valid_from: None,
        invalid_at: None,
        force: false,
        choice: None,
        expected_updated: None,
        arriving: None,
        // Ein Sitzungs-Snapshot beschreibt keinen Codestand: nichts verankern, nichts erkennen.
        anchors: crate::app::Anchoring::default(),
        dry_run: false,
    };
    let written = match app.write(req.clone()) {
        Ok(WriteOutcome::Held { findings, .. }) => {
            out.note(format!(
                "pre-compact-snapshot: {} PII finding(s) redacted before writing",
                findings.len()
            ));
            app.write(WriteRequest {
                choice: Some(OperatorChoice::Redact),
                ..req
            })
        }
        other => other,
    };
    match written {
        Ok(WriteOutcome::Written(_)) => {}
        Ok(other) => {
            out.note(format!(
                "pre-compact-snapshot: the snapshot was not written ({}); the summary is \
                 steered all the same",
                outcome_name(&other)
            ));
            return None;
        }
        Err(e) => {
            out.note(format!(
                "pre-compact-snapshot: the snapshot could not be written: {e}; the summary is \
                 steered all the same"
            ));
            return None;
        }
    }
    let citation = app
        .store()
        .read(&name)
        .ok()
        .and_then(|n| {
            blocks_of(&n, MAX_BLOCK_TOKENS)
                .0
                .first()
                .map(|b| b.citation.to_string())
        })
        .unwrap_or_default();
    let mark = SnapshotMark {
        name,
        citation,
        compaction,
        at: now.to_string(),
    };
    state.last_snapshot = Some(mark.clone());
    if let Err(e) = session::save(app.root(), &id, &state) {
        out.note(format!(
            "pre-compact-snapshot: written, but the session state was not: {e}"
        ));
    }
    out.note(format!(
        "pre-compact-snapshot: kept as {} [{}]",
        mark.name, mark.citation
    ));
    (!mark.citation.is_empty()).then_some(mark)
}

fn outcome_name(o: &WriteOutcome) -> &'static str {
    match o {
        WriteOutcome::Written(_) => "written",
        WriteOutcome::Held { .. } => "held by the PII gate",
        WriteOutcome::Conflict { .. } => "a conflicting change",
        WriteOutcome::Quarantined(_) => "sent to proposals for review",
    }
}

/// The body, at most [`MAX_BODY_BYTES`]: what it is, the citations, the tools, the first
/// request, then as many of the newest messages as fit, oldest of them first.
fn body(
    id: &str,
    compaction: u32,
    trigger: &str,
    now: &str,
    messages: &[Message],
    cleaned: &[(String, String)],
    citations: &[String],
) -> String {
    let tool_calls: Vec<&str> = messages.iter().flat_map(Message::tool_names).collect();
    let mut b = format!(
        "Snapshot of session `{id}` before compaction #{compaction} ({trigger}), taken {now}: \
         {} message(s), {} tool call(s). Written by the compaction mod from the conversation's \
         text; tool inputs and outputs are not in it.\n\n",
        messages.len(),
        tool_calls.len()
    );
    if !citations.is_empty() {
        b.push_str("## Citations used, newest first\n\n");
        b.push_str(&citations.join(", "));
        b.push_str("\n\n");
    }
    if !tool_calls.is_empty() {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for t in &tool_calls {
            match counts.iter_mut().find(|(n, _)| n == t) {
                Some((_, c)) => *c += 1,
                None => counts.push((t, 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        b.push_str("## Tools\n\n");
        let list: Vec<String> = counts
            .iter()
            .map(|(n, c)| format!("{} ×{c}", one_line(n, 60)))
            .collect();
        b.push_str(&list.join(", "));
        let tail: Vec<String> = tool_calls[tool_calls.len().saturating_sub(TOOL_TAIL)..]
            .iter()
            .map(|n| one_line(n, 60))
            .collect();
        b.push_str(&format!("\n\nLast calls: {}\n\n", tail.join(", ")));
    }
    let first_user = cleaned
        .iter()
        .position(|(r, t)| r == "user" && !t.trim().is_empty());
    let mut head = String::new();
    if let Some(i) = first_user {
        head.push_str("## First request\n\n");
        head.push_str(&quote(&cleaned[i].1, USER_CHARS));
        head.push('\n');
    }
    if b.len() + head.len() < MAX_BODY_BYTES / 2 {
        b.push_str(&head);
    }

    let title = "## Last messages\n\n";
    let mut room = MAX_BODY_BYTES.saturating_sub(b.len() + title.len());
    let mut tail: Vec<String> = Vec::new();
    for (i, (role, text)) in cleaned.iter().enumerate().rev() {
        if text.trim().is_empty() || Some(i) == first_user {
            continue;
        }
        let limit = if role == "assistant" {
            ASSISTANT_CHARS
        } else {
            USER_CHARS
        };
        let who = if role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        let piece = format!("### {who}\n\n{}\n", quote(text, limit));
        if piece.len() > room {
            break;
        }
        room -= piece.len();
        tail.push(piece);
    }
    if !tail.is_empty() {
        b.push_str(title);
        for piece in tail.iter().rev() {
            b.push_str(piece);
        }
    }
    truncate_bytes(&mut b, MAX_BODY_BYTES);
    b
}

/// What the summarizer is asked to keep, at most [`MAX_INSTRUCTIONS_BYTES`]. Empty when there
/// is nothing to ask.
fn instructions(citations: &[String], open: &[String], snapshot: Option<&SnapshotMark>) -> String {
    if citations.is_empty() && open.is_empty() && snapshot.is_none() {
        return String::new();
    }
    let mut s = String::from(
        "The project's memory (cyberbrain) asks the summary to keep the following, verbatim.\n",
    );
    if let Some(m) = snapshot {
        s.push_str(&format!(
            "- The conversation as it stood before this compaction is kept as the note `{}`. \
             Name its citation {} in the summary; `cyberbrain recall --id {}` opens it.\n",
            m.name, m.citation, m.citation
        ));
    }
    if !citations.is_empty() {
        s.push_str(
            "- Citations the conversation relied on, newest first. Keep each one exactly as \
             written, next to what it supports: ",
        );
        s.push_str(&citations.join(", "));
        s.push('\n');
    }
    if !open.is_empty() {
        s.push_str(
            "- Open points of ring 1 (the operator's protocol and handoff state). Keep what the \
             conversation did about each:\n",
        );
        for p in open {
            s.push_str("  - ");
            s.push_str(p);
            s.push('\n');
        }
    }
    truncate_bytes(&mut s, MAX_INSTRUCTIONS_BYTES);
    s
}

/// Lines of ring 1 that read as open: an unticked box, or a word that says so. Each with its
/// block's citation.
fn open_points(app: &App) -> Vec<String> {
    let res = resident::read_checked(app);
    let mut points = Vec::new();
    for n in res
        .notes
        .iter()
        .filter(|n| n.note.front.ring == Ring::Protocol)
    {
        for b in &n.blocks {
            for line in b.text.lines() {
                let t = line.trim();
                let l = t.to_lowercase();
                let open = t.starts_with("- [ ]")
                    || t.starts_with("* [ ]")
                    || [
                        "todo",
                        "offen",
                        "open:",
                        "open point",
                        "pending",
                        "wartet",
                        "⏳",
                    ]
                    .iter()
                    .any(|w| l.contains(w));
                if open && points.len() < MAX_OPEN_POINTS {
                    points.push(format!(
                        "{} [{}]",
                        one_line(t, OPEN_POINT_CHARS),
                        b.citation
                    ));
                }
            }
        }
    }
    points
}

/// Every citation (`r0-` to `r4-` and twelve hex digits) in `texts`, each once, the most
/// recently used first.
fn citations_newest_first<'a>(texts: impl DoubleEndedIterator<Item = &'a str>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for text in texts.rev() {
        let mut in_text: Vec<String> = Vec::new();
        let b = text.as_bytes();
        let mut i = 0;
        while i + 15 <= b.len() {
            let starts = b[i] == b'r'
                && (b'0'..=b'4').contains(&b[i + 1])
                && b[i + 2] == b'-'
                && (i == 0 || !b[i - 1].is_ascii_alphanumeric());
            let hex = starts
                && b[i + 3..i + 15]
                    .iter()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(c));
            let ends = b.get(i + 15).is_none_or(|c| !c.is_ascii_alphanumeric());
            if hex && ends {
                in_text.push(text[i..i + 15].to_string());
                i += 15;
            } else {
                i += 1;
            }
        }
        // Within one message the later mention is the newer one.
        for c in in_text.into_iter().rev() {
            if !found.contains(&c) {
                found.push(c);
            }
        }
        if found.len() >= MAX_CITATIONS {
            break;
        }
    }
    found.truncate(MAX_CITATIONS);
    found
}

/// The text without the harness's reminders and control characters.
fn clean(text: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::with_capacity(text.len().min(64 * 1024));
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        match rest[start..].find(CLOSE) {
            Some(end) => rest = &rest[start + end + CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.retain(|c| !c.is_control() || c == '\n' || c == '\t');
    out.trim().to_string()
}

/// `text` cut to `limit` characters and set as a Markdown quote, so that nothing in it is
/// read as a heading, a locator line or a `[[link]]` of the note.
fn quote(text: &str, limit: usize) -> String {
    let n = text.chars().count();
    let mut t: String = text.chars().take(limit).collect();
    if n > limit {
        t.push_str(&format!(" … [{} more characters]", n - limit));
    }
    let t = t.replace("[[", "[ [").replace("]]", "] ]");
    let mut q = String::new();
    for line in t.lines() {
        q.push_str("> ");
        q.push_str(line);
        q.push('\n');
    }
    q
}

fn one_line(text: &str, limit: usize) -> String {
    let t: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit)
        .collect();
    t.replace("[[", "[ [")
}

/// The session id as part of a note name: lowercase letters and digits, single hyphens, at
/// most 12 characters.
fn slug(id: &str) -> String {
    let mut s = String::new();
    for c in id.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 12 {
            break;
        }
    }
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "session".into() } else { s }
}

fn truncate_bytes(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn citations_are_found_newest_first_and_only_whole() {
        let texts = [
            "see r2-a91f2c33e1bd and r1-000000000000",
            "xr2-a91f2c33e1bd r5-a91f2c33e1bd r2-a91f2c33e1bdz r3-ffffffffffff.",
        ];
        assert_eq!(
            citations_newest_first(texts.iter().copied()),
            vec!["r3-ffffffffffff", "r1-000000000000", "r2-a91f2c33e1bd"]
        );
    }

    #[test]
    fn the_session_id_makes_a_valid_name() {
        for id in [
            "0b5a7c1e-1234-5678-9abc-def012345678",
            "Sess_0123",
            "___",
            "a.b",
        ] {
            let name = format!("{TAG}-{}-1", slug(id));
            assert!(cyberbrain_core::validate_name(&name).is_ok(), "{name}");
        }
    }

    #[test]
    fn quoted_text_cannot_start_a_block_or_a_link() {
        let q = quote("# heading\n<!-- cyberbrain:locator 3 -->\n[[note]]", 100);
        assert!(q.lines().all(|l| l.starts_with("> ")), "{q}");
        assert!(!q.contains("[[") && !q.contains("]]"), "{q}");
    }
}
