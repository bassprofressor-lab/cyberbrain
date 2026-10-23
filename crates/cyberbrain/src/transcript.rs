//! A Claude Code session transcript, read for `propose-session` (0.7.10).
//!
//! Claude Code keeps one JSON line per entry in `~/.claude/projects/<project>/<session>.jsonl`.
//! What is taken from it is what was said: the person's prompts, the assistant's text, and
//! the names of the tools it used. Left out on purpose:
//!
//! - **Tool results.** They are most of the bytes and rarely the point; what came of them is
//!   in the assistant's next words.
//! - **System reminders**, which is how hook output reaches the model. That includes the rings
//!   this store injected at session start: summarising them back into a proposal would
//!   restate what the store already holds.
//!
//! Long sessions keep their **end**, not their start: what was decided last and what is still
//! open is what a summary is for, and the model's input is capped
//! (`cyberbrain_llm::tasks::MAX_INPUT_CHARS`).

use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extract {
    /// `User: …` / `Assistant: …` paragraphs, oldest first.
    pub text: String,
    /// Paragraphs at the start were dropped to fit `max_chars`.
    pub omitted_start: bool,
    /// Prompts the person typed (a turn each).
    pub prompts: usize,
}

/// The conversation in `jsonl`, at most `max_chars` characters, keeping the end.
/// Lines that are not JSON, or not a user or assistant entry, are skipped.
pub fn extract(jsonl: &str, max_chars: usize) -> Extract {
    let mut paras: Vec<String> = Vec::new();
    let mut prompts = 0;
    for line in jsonl.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let role = match v.get("type").and_then(Value::as_str) {
            Some("user") => "User",
            Some("assistant") => "Assistant",
            _ => continue,
        };
        if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let content = &v["message"]["content"];
        let mut parts: Vec<String> = Vec::new();
        match content {
            Value::String(s) => parts.push(strip_reminders(s)),
            Value::Array(blocks) => {
                for b in blocks {
                    match b.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = b.get("text").and_then(Value::as_str) {
                                parts.push(strip_reminders(t));
                            }
                        }
                        Some("tool_use") => {
                            if let Some(n) = b.get("name").and_then(Value::as_str) {
                                parts.push(format!("[used {n}]"));
                            }
                        }
                        // tool_result, thinking, images: not what was said
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let said = parts
            .into_iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if said.is_empty() {
            continue;
        }
        if role == "User" && !said.starts_with("[used ") {
            prompts += 1;
        }
        paras.push(format!("{role}: {said}"));
    }

    // Keep the end: walk back until the cap, then restore order.
    let mut kept: Vec<String> = Vec::new();
    let mut size = 0usize;
    let mut omitted_start = false;
    for p in paras.iter().rev() {
        let n = p.chars().count() + 2;
        if size + n > max_chars {
            omitted_start = true;
            break;
        }
        size += n;
        kept.push(p.clone());
    }
    kept.reverse();
    let mut text = kept.join("\n\n");
    if omitted_start {
        text = format!("[earlier part of the session omitted]\n\n{text}");
    }
    Extract {
        text,
        omitted_start,
        prompts,
    }
}

/// `text` without `<system-reminder>…</system-reminder>` sections. An unclosed one runs to
/// the end of the text.
fn strip_reminders(text: &str) -> String {
    const OPEN: &str = "<system-reminder";
    const CLOSE: &str = "</system-reminder>";
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        out.push_str(&rest[..i]);
        match rest[i..].find(CLOSE) {
            Some(j) => rest = &rest[i + j + CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The directory name Claude Code gives a project under `~/.claude/projects/`: the path with
/// every character other than a letter or digit replaced by `-` (`/root/orderflow` →
/// `-root-orderflow`).
pub fn project_key(project: &Path) -> String {
    project
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The newest transcript of `project` under `home/.claude/projects`, by modification time.
pub fn latest(home: &Path, project: &Path) -> Option<PathBuf> {
    let dir = home
        .join(".claude")
        .join("projects")
        .join(project_key(project));
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: serde_json::Value) -> String {
        format!("{v}\n")
    }

    fn sample() -> String {
        let mut s = String::new();
        s += &line(
            serde_json::json!({"type": "user", "message": {"role": "user", "content":
            "<system-reminder>RING 0: never copy config.py</system-reminder>Bitte Ollama aktualisieren."}}),
        );
        s += &line(
            serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "text", "text": "Ich prüfe zuerst die Version."},
            {"type": "tool_use", "name": "Bash", "input": {"command": "docker ps"}}]}}),
        );
        s += &line(serde_json::json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "content": "cfetch-embeddings ollama/ollama:0.40.5 GEHEIMES-LOG"}]}}));
        s += &line(
            serde_json::json!({"type": "user", "isMeta": true, "message": {"content": "meta"}}),
        );
        s += "not json\n";
        s += &line(serde_json::json!({"type": "summary", "summary": "x"}));
        s += &line(
            serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "thinking", "thinking": "hmm"},
            {"type": "text", "text": "Auf 0.40.9 aktualisiert."}]}}),
        );
        s
    }

    #[test]
    fn keeps_what_was_said_and_drops_results_reminders_and_meta() {
        let e = extract(&sample(), 10_000);
        assert_eq!(
            e.text,
            "User: Bitte Ollama aktualisieren.\n\n\
             Assistant: Ich prüfe zuerst die Version. [used Bash]\n\n\
             Assistant: Auf 0.40.9 aktualisiert."
        );
        assert!(
            !e.text.contains("RING 0"),
            "injected rings are not summarised back"
        );
        assert!(!e.text.contains("GEHEIMES-LOG"), "tool results stay out");
        assert!(!e.text.contains("hmm") && !e.text.contains("meta"));
        assert_eq!(e.prompts, 1);
        assert!(!e.omitted_start);
    }

    #[test]
    fn a_long_session_keeps_its_end() {
        let mut s = String::new();
        for i in 0..200 {
            s += &line(
                serde_json::json!({"type": "user", "message": {"content": format!("Frage {i}")}}),
            );
        }
        let e = extract(&s, 300);
        assert!(e.omitted_start);
        assert!(e.text.starts_with("[earlier part of the session omitted]"));
        assert!(e.text.ends_with("User: Frage 199"), "{}", e.text);
        assert!(!e.text.contains("Frage 0\n"));
        assert!(e.text.chars().count() <= 300 + 40);
    }

    #[test]
    fn an_unclosed_reminder_hides_the_rest_rather_than_leaking_it() {
        assert_eq!(strip_reminders("a<system-reminder>secret"), "a");
        assert_eq!(
            strip_reminders("a<system-reminder x=1>s</system-reminder>b"),
            "ab"
        );
    }

    #[test]
    fn project_key_matches_claude_code() {
        assert_eq!(project_key(Path::new("/root/orderflow")), "-root-orderflow");
        assert_eq!(project_key(Path::new("/srv/my_app.v2")), "-srv-my-app-v2");
    }

    #[test]
    fn latest_picks_the_newest_jsonl_of_the_project() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".claude/projects/-work-p");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.jsonl"), "").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("b.jsonl"), "").unwrap();
        std::fs::write(dir.join("c.txt"), "").unwrap();
        assert_eq!(
            latest(home.path(), Path::new("/work/p"))
                .unwrap()
                .file_name()
                .unwrap(),
            "b.jsonl"
        );
        assert!(latest(home.path(), Path::new("/elsewhere")).is_none());
    }
}
