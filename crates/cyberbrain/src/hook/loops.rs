//! The loop hint (0.9): telling an agent that it is searching in circles.
//!
//! Measured against a control group (`scripts/ab-messung.py`), the expensive runs were the
//! ones that searched on: 6 to 16 searches in a row, each round re-reading the whole context,
//! so the tenth search costs as much as the first nine together. Two patterns showed up, and
//! both are visible in one tool call's input and result:
//!
//! - **The same search again.** Batou in *Innocence* walks through the same house three times
//!   without noticing. A search nearly identical to two earlier ones will not return
//!   something new.
//! - **Search after search that finds nothing.** Most long runs were this: grep after grep
//!   for something that is not in the project, until the agent gave up anyway.
//!
//! The hint is a sentence of additional context after the call that completes the pattern,
//! once, then quiet for a while. Every other call it says nothing, so it costs no tokens
//! until it fires. It never blocks a call: the agent may have a reason.
//!
//! What counts as a search: Grep, Glob, `cyberbrain recall`/`find` over MCP, and Bash command
//! lines that run grep, rg, find and the like. Read is not a search, but finding something by
//! definition, it ends a dry streak. Hybrid recall
//! always returns its nearest neighbours, so it can repeat but never be "empty".
//!
//! State is a small file per session (and per subagent) next to the session state. Parallel
//! tool calls race for it and the last write wins; a lost entry delays a hint by one call,
//! which is cheaper than a lock in a hook with a 15 ms budget.

use cyberbrain_core::config::LoopsConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::payload::Payload;
use super::session::{safe_id, sessions_dir};

/// How many recent calls repetition is judged against.
const WINDOW: usize = 12;
/// Calls after a hint during which no further hint fires.
const QUIET_AFTER_HINT: u32 = 6;
/// Word sets this similar are the same search: `["trail","carry"]` and `["carry","trail",
/// "s2"]` are (2/3 = 0.67 is not), a different path or pattern is not.
const SAME: f64 = 0.75;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LoopState {
    recent: Vec<Vec<String>>,
    empty_streak: usize,
    quiet: u32,
}

/// One tool call as the loop hint sees it.
#[derive(Debug, PartialEq)]
pub struct Call {
    /// Normalised words of the search; empty for a call that never repeats (Read).
    pub key: Vec<String>,
    /// `Some(true)`: found nothing; `Some(false)`: found something; `None`: cannot tell.
    pub empty: Option<bool>,
    /// A short form of the input for the hint text.
    pub shown: String,
}

/// What the hint says, if anything.
#[derive(Debug, PartialEq)]
pub enum Hint {
    Repeat { count: usize, shown: String },
    Dry { count: usize },
}

pub fn path(root: &Path, session: &str, agent: Option<&str>) -> PathBuf {
    let name = match agent.and_then(safe_id) {
        Some(a) => format!("{session}.{a}.loops.json"),
        None => format!("{session}.loops.json"),
    };
    sessions_dir(root).join(name)
}

/// The call, if the loop hint has anything to say about this kind of tool.
pub fn classify(p: &Payload) -> Option<Call> {
    let tool = p.tool_name.as_deref()?;
    let input = &p.tool_input;
    let resp = &p.tool_response;
    let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    match tool {
        "Grep" => {
            let shown = s("pattern").to_string();
            let key = words(
                &format!("{} {} {}", s("pattern"), s("path"), s("glob")),
                &[],
            );
            let none = resp.get("numFiles").and_then(Value::as_u64) == Some(0)
                && resp.get("numLines").and_then(Value::as_u64).unwrap_or(0) == 0;
            let some = resp
                .get("numFiles")
                .and_then(Value::as_u64)
                .is_some_and(|n| n > 0)
                || resp
                    .get("numLines")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > 0);
            Some(Call {
                key,
                empty: if none {
                    Some(true)
                } else if some {
                    Some(false)
                } else {
                    None
                },
                shown,
            })
        }
        "Glob" => Some(Call {
            key: words(&format!("{} {}", s("pattern"), s("path")), &[]),
            empty: resp.get("numFiles").and_then(Value::as_u64).map(|n| n == 0),
            shown: s("pattern").to_string(),
        }),
        // Reading a file again after an edit is ordinary work, not a loop: Read only ends a
        // dry streak (it found a file by definition) and never counts as a repeat.
        "Read" => Some(Call {
            key: Vec::new(),
            empty: Some(false),
            shown: s("file_path").to_string(),
        }),
        "Bash" => {
            let cmd = p.bash_command()?;
            if !is_search(cmd) {
                return None;
            }
            let out = resp.get("stdout").and_then(Value::as_str);
            Some(Call {
                key: words(cmd, SEARCH_VERBS),
                empty: out.map(|o| {
                    let o = o.trim();
                    o.is_empty() || o == "no hits" || o.starts_with("no definition of")
                }),
                shown: short(cmd),
            })
        }
        t if t.starts_with("mcp__") && (t.ends_with("__recall") || t.ends_with("__find")) => {
            let q = input
                .get("query")
                .or_else(|| input.get("symbol"))
                .or_else(|| input.get("id"))
                .and_then(Value::as_str)
                .unwrap_or("");
            // The MCP result is the JSON text of the report; `"hits":[]` is nothing found.
            let text = match resp {
                Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            Some(Call {
                key: words(q, &[]),
                empty: if text.contains("\"hits\":[]") {
                    Some(true)
                } else if text.contains("\"hits\":[{") {
                    Some(false)
                } else {
                    None
                },
                shown: q.to_string(),
            })
        }
        _ => None,
    }
}

/// Programs whose command line is a search. Checked at the start of every pipeline stage.
const SEARCH_VERBS: &[&str] = &[
    "grep",
    "rg",
    "ag",
    "ack",
    "find",
    "fd",
    "locate",
    "cyberbrain",
];

fn is_search(cmd: &str) -> bool {
    cmd.split(['|', ';', '&']).any(|stage| {
        let mut w = stage.split_whitespace();
        match w.next() {
            Some("git") => w.next() == Some("grep"),
            Some("cyberbrain") => matches!(w.next(), Some("recall" | "find")),
            Some(first) => SEARCH_VERBS.contains(&first.rsplit('/').next().unwrap_or(first)),
            None => false,
        }
    })
}

/// Lower-cased words of at least two characters, flags and the given verbs left out, sorted
/// and without repeats: the same search typed twice in another order is the same search.
fn words(text: &str, drop: &[&str]) -> Vec<String> {
    let set: BTreeSet<String> = text
        .split_whitespace()
        .filter(|w| !w.starts_with('-'))
        .flat_map(|w| w.split(|c: char| !c.is_alphanumeric() && c != '_'))
        .map(str::to_lowercase)
        .filter(|w| w.chars().count() >= 2 && !drop.contains(&w.as_str()))
        .collect();
    set.into_iter().collect()
}

fn similar(a: &[String], b: &[String]) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let a: BTreeSet<&String> = a.iter().collect();
    let b: BTreeSet<&String> = b.iter().collect();
    let inter = a.intersection(&b).count() as f64;
    let union = a.union(&b).count() as f64;
    inter / union >= SAME
}

fn short(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s
    }
}

/// Fold one call into the state; the hint, when this call completes a pattern.
pub fn observe(state: &mut LoopState, call: Call, cfg: &LoopsConfig) -> Option<Hint> {
    let earlier = state
        .recent
        .iter()
        .filter(|k| similar(k, &call.key))
        .count();
    match call.empty {
        Some(true) => state.empty_streak += 1,
        Some(false) => state.empty_streak = 0,
        None => {}
    }
    state.recent.push(call.key);
    if state.recent.len() > WINDOW {
        state.recent.remove(0);
    }
    if state.quiet > 0 {
        state.quiet -= 1;
        return None;
    }
    let hint = if earlier + 1 >= cfg.repeat {
        Some(Hint::Repeat {
            count: earlier + 1,
            shown: call.shown,
        })
    } else if state.empty_streak >= cfg.empty_streak {
        Some(Hint::Dry {
            count: state.empty_streak,
        })
    } else {
        None
    };
    if hint.is_some() {
        state.quiet = QUIET_AFTER_HINT;
        state.empty_streak = 0;
    }
    hint
}

pub fn text(h: &Hint) -> String {
    match h {
        Hint::Repeat { count, shown } => format!(
            "cyberbrain: this is search number {count} for nearly the same thing (`{}`) in this \
             session; it will not return anything new. Work with what the earlier results gave \
             you (`cyberbrain recall --id <citation>` expands a hit), search for something \
             different, or say that you could not find it.",
            short(shown).replace('`', "'")
        ),
        Hint::Dry { count } => format!(
            "cyberbrain: {count} searches in a row found nothing. If it were in this project, \
             one of them would likely have hit. Ask `cyberbrain recall` if you have not, \
             otherwise say what you could not find instead of searching on."
        ),
    }
}

pub fn load(path: &Path) -> LoopState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, state: &LoopState) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(state).unwrap_or_default())?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(tool: &str, input: Value, resp: Value) -> Payload {
        Payload::parse(
            &json!({"tool_name": tool, "tool_input": input, "tool_response": resp}).to_string(),
        )
    }
    fn cfg() -> LoopsConfig {
        LoopsConfig::default()
    }

    // The shapes below are what Claude Code sent a PostToolUse hook on 2026-11-01.
    fn grep(pattern: &str, n: u64) -> Payload {
        payload(
            "Grep",
            json!({"pattern": pattern}),
            json!({"mode": "files_with_matches", "filenames": [], "numFiles": n, "totalFiles": n}),
        )
    }

    #[test]
    fn grep_and_glob_say_whether_they_found_something() {
        assert_eq!(classify(&grep("zzqqxx", 0)).unwrap().empty, Some(true));
        assert_eq!(classify(&grep("hello", 1)).unwrap().empty, Some(false));
        let glob = payload(
            "Glob",
            json!({"pattern": "*.nothing"}),
            json!({"filenames": [], "numFiles": 0, "truncated": false}),
        );
        assert_eq!(classify(&glob).unwrap().empty, Some(true));
    }

    #[test]
    fn only_bash_lines_that_search_count() {
        let bash = |cmd: &str, out: &str| {
            payload(
                "Bash",
                json!({"command": cmd}),
                json!({"stdout": out, "stderr": ""}),
            )
        };
        let c = classify(&bash("grep -rn TELEGRAM_ADMIN backend/", "")).unwrap();
        assert_eq!(c.empty, Some(true));
        assert_eq!(c.key, vec!["backend", "telegram_admin"]);
        assert!(classify(&bash("cat backend/x.py | grep foo", "x")).is_some());
        assert!(classify(&bash("git grep foo", "")).is_some());
        assert!(classify(&bash("cyberbrain recall \"carry trail\"", "1. r2-…")).is_some());
        assert!(classify(&bash("cargo build", "")).is_none());
        assert!(classify(&bash("cyberbrain status", "")).is_none());
    }

    #[test]
    fn mcp_recall_repeats_but_is_never_empty_by_its_nearest_neighbours() {
        let r = payload(
            "mcp__cyberbrain__recall",
            json!({"query": "zzqqxx wbvv"}),
            Value::String("{\"caveats\":[],\"hits\":[{\"citation\":\"r2-1\"}]}".into()),
        );
        let c = classify(&r).unwrap();
        assert_eq!(c.empty, Some(false));
        assert_eq!(c.key, vec!["wbvv", "zzqqxx"]);
    }

    #[test]
    fn the_third_nearly_identical_search_gets_the_hint_once() {
        let mut st = LoopState::default();
        let call = |q: &str| classify(&grep(q, 1)).unwrap();
        assert_eq!(observe(&mut st, call("carry trail"), &cfg()), None);
        assert_eq!(observe(&mut st, call("trail carry"), &cfg()), None);
        let h = observe(&mut st, call("carry  trail"), &cfg());
        assert!(matches!(h, Some(Hint::Repeat { count: 3, .. })), "{h:?}");
        // Then quiet, even though the fourth is the same again.
        assert_eq!(observe(&mut st, call("carry trail"), &cfg()), None);
    }

    #[test]
    fn different_searches_are_not_a_loop() {
        let mut st = LoopState::default();
        for q in [
            "carry trail",
            "stop cap",
            "liquidation",
            "trail datum s2",
            "engine carry",
        ] {
            assert_eq!(
                observe(&mut st, classify(&grep(q, 1)).unwrap(), &cfg()),
                None,
                "{q}"
            );
        }
    }

    #[test]
    fn five_empty_searches_in_a_row_get_the_hint_and_a_find_resets_the_count() {
        let mut st = LoopState::default();
        let qs = ["vordach", "awning", "canopy", "bakery", "roof"];
        for q in &qs[..4] {
            assert_eq!(
                observe(&mut st, classify(&grep(q, 0)).unwrap(), &cfg()),
                None
            );
        }
        // A find in between: the streak starts again.
        assert_eq!(
            observe(&mut st, classify(&grep("dach", 3)).unwrap(), &cfg()),
            None
        );
        for q in &qs[..4] {
            let q = format!("{q} 2");
            assert_eq!(
                observe(&mut st, classify(&grep(&q, 0)).unwrap(), &cfg()),
                None
            );
        }
        let h = observe(&mut st, classify(&grep("baeckerei", 0)).unwrap(), &cfg());
        assert_eq!(h, Some(Hint::Dry { count: 5 }));
    }

    #[test]
    fn the_hint_text_does_not_let_the_input_break_out_of_its_quotes() {
        let t = text(&Hint::Repeat {
            count: 3,
            shown: "a`b\nc".into(),
        });
        assert!(t.contains("`a'bc`"), "{t}");
    }

    #[test]
    fn state_survives_a_round_trip_and_a_missing_file_is_a_fresh_start() {
        let dir = tempfile::tempdir().unwrap();
        let p = path(dir.path(), "s1", Some("agent-7"));
        assert!(load(&p).recent.is_empty());
        let mut st = LoopState::default();
        observe(&mut st, classify(&grep("x1 y1", 0)).unwrap(), &cfg());
        save(&p, &st).unwrap();
        assert_eq!(load(&p).empty_streak, 1);
        assert!(
            p.to_string_lossy().ends_with("s1.agent-7.loops.json"),
            "{}",
            p.display()
        );
    }
}
