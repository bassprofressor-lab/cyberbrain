//! Installation drift, reported at session start (0.9).
//!
//! Two ways an update leaves a machine half on the old version, both seen on our own server
//! before this check existed:
//!
//! - **Hook entries from an older `cyberbrain install`.** The binary is replaced, the entries
//!   in `.claude/settings.json` are not: a hook added in the new version (0.8.5 brought
//!   `SubagentStart`) never runs, and nothing says so, because a hook that is not configured
//!   cannot complain about it. The entries are compared with what `install` writes today.
//! - **A long-running process on a replaced binary.** `cyberbrain mcp --http` and `daemon`
//!   keep the old program in memory after the file is swapped (31.10.2026: the MCP service
//!   answered as 0.8.2 next to a 0.8.4 binary). On Linux the kernel marks such a process's
//!   executable ` (deleted)`; that mark is what is read here. Other systems have no cheap
//!   equivalent, so there the check stays silent rather than guessing.
//!
//! Idea from ProjectAtlas (styler-ai, MIT), whose session hook refuses to call an integration
//! ready until runtime, plugin and generated configuration report one version. Ours is
//! smaller: it names what is out of step and the command that repairs it, and never repairs
//! anything itself. A hook writes no settings file.
//!
//! Budget: one or two small JSON files and, on Linux, one `readlink` per process. Nothing
//! here grows with the store.

use crate::install::{self, EVENTS, Env, MANAGED_BY};
use cyberbrain_core::Slash;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Everything out of step, one line each, ready for the digest. Empty when nothing is.
pub(crate) fn report(store_root: &Path, cwd: Option<&Path>, fail_closed: bool) -> Vec<String> {
    let env = Env::current();
    let mut lines = Vec::new();
    if install::plugin_installed(&env).is_none() {
        for project in projects(store_root, cwd) {
            let path = project.join(".claude").join("settings.json");
            if let Some(line) = hook_drift(&path, fail_closed) {
                lines.push(line);
            }
        }
    }
    if let Some(line) = replaced_processes(&running_processes()) {
        lines.push(line);
    }
    lines
}

/// The project that holds the store, and the session's working directory when that is a
/// different one. `install` writes into the project it was run in, which is usually both.
fn projects(store_root: &Path, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in [store_root.parent(), cwd].into_iter().flatten() {
        let p = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// What `install` writes for one event, without the `command`: the path of the binary is
/// whatever it was on the machine that ran `install`, and the hook that is running proves
/// it resolves. What has to match is everything else.
fn expected(args: &[&str], matcher: &str, fail_closed: bool) -> Value {
    strip_command(&install::hook_entry(
        Path::new(""),
        matcher,
        args,
        fail_closed,
    ))
}

fn strip_command(entry: &Value) -> Value {
    let mut e = entry.clone();
    if let Some(hooks) = e.get_mut("hooks").and_then(Value::as_array_mut) {
        for h in hooks {
            if let Some(o) = h.as_object_mut() {
                o.remove("command");
            }
        }
    }
    e
}

/// Compares our entries in one settings file with what this binary would write. A file
/// without any entry of ours is not drift: the hooks then come from somewhere else (user
/// settings, a managed file) or are not installed, and neither is this check's business.
pub(crate) fn hook_drift(path: &Path, fail_closed: bool) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let root: Value = serde_json::from_str(&text).ok()?;
    let hooks = root.get("hooks")?.as_object()?;
    let ours = |v: &&Value| v.get("_managedBy").and_then(Value::as_str) == Some(MANAGED_BY);

    let mut found = 0usize;
    let (mut missing, mut changed, mut unused) = (Vec::new(), Vec::new(), Vec::new());
    let known: BTreeSet<&str> = EVENTS.iter().map(|(e, _, _)| *e).collect();
    for (event, list) in hooks {
        let n = list.as_array().map_or(0, |a| a.iter().filter(ours).count());
        found += n;
        if n > 0 && !known.contains(event.as_str()) {
            unused.push(event.clone());
        }
    }
    if found == 0 {
        return None;
    }
    for (event, args, matcher) in EVENTS {
        let entry = hooks
            .get(event)
            .and_then(Value::as_array)
            .and_then(|a| a.iter().find(ours));
        match entry {
            None => missing.push(event.to_string()),
            Some(e) => {
                let want = expected(args, matcher, fail_closed && event == "PreToolUse");
                if strip_command(e) != want {
                    changed.push(event.to_string());
                }
            }
        }
    }
    if missing.is_empty() && changed.is_empty() && unused.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !missing.is_empty() {
        parts.push(format!("missing {}", missing.join(", ")));
    }
    if !changed.is_empty() {
        parts.push(format!("different {}", changed.join(", ")));
    }
    if !unused.is_empty() {
        parts.push(format!("no longer used {}", unused.join(", ")));
    }
    let project = path
        .parent()
        .and_then(Path::parent)
        .map(|p| Slash(p).to_string())
        .unwrap_or_default();
    Some(format!(
        "**hook entries out of date** in `{}`: not what `cyberbrain install` of this version \
         ({}) writes: {}. Until repaired those hooks run the old way or not at all. Repair: \
         `cyberbrain install --client claude-code --project {project}`. Tell the operator.",
        Slash(path),
        env!("CARGO_PKG_VERSION"),
        parts.join("; ")
    ))
}

/// One running process: its id, where its executable link points, its command line.
pub(crate) struct Proc {
    pub pid: u32,
    pub exe: String,
    pub cmdline: String,
}

/// The processes of a cyberbrain binary whose file has been replaced or removed since they
/// started.
pub(crate) fn replaced_processes(procs: &[Proc]) -> Option<String> {
    let me = std::process::id();
    let stale: Vec<&Proc> = procs
        .iter()
        .filter(|p| p.pid != me)
        .filter(|p| {
            p.exe.strip_suffix(" (deleted)").is_some_and(|exe| {
                Path::new(exe)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("cyberbrain"))
            })
        })
        .collect();
    if stale.is_empty() {
        return None;
    }
    let shown: Vec<String> = stale
        .iter()
        .take(3)
        .map(|p| format!("pid {} `{}`", p.pid, short(&p.cmdline)))
        .collect();
    Some(format!(
        "**{} cyberbrain process(es) still run a replaced binary**: {}{}. They answer as the \
         old version until restarted (a service: restart it; otherwise end the process). \
         Tell the operator.",
        stale.len(),
        shown.join(", "),
        if stale.len() > shown.len() {
            ", …"
        } else {
            ""
        }
    ))
}

/// A command line fit for the model's context: no control characters, at most 70 characters.
fn short(cmd: &str) -> String {
    let s: String = cmd
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let s = s.trim();
    if s.chars().count() > 70 {
        format!("{}…", s.chars().take(69).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(target_os = "linux")]
fn running_processes() -> Vec<Proc> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            // Processes of other users answer EACCES; they are skipped, not reported.
            let exe = std::fs::read_link(e.path().join("exe")).ok()?;
            let exe = exe.to_string_lossy().into_owned();
            if !exe.contains("cyberbrain") {
                return None;
            }
            let raw = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
            let cmdline = String::from_utf8_lossy(&raw).replace('\0', " ");
            Some(Proc { pid, exe, cmdline })
        })
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn running_processes() -> Vec<Proc> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn current_settings(fail_closed: bool) -> Value {
        let mut hooks = serde_json::Map::new();
        for (event, args, matcher) in EVENTS {
            let e = install::hook_entry(
                Path::new("/usr/local/bin/cyberbrain"),
                matcher,
                args,
                fail_closed && event == "PreToolUse",
            );
            hooks.insert(event.to_string(), json!([e]));
        }
        json!({ "hooks": hooks })
    }

    fn write(dir: &Path, v: &Value) -> PathBuf {
        let p = dir.join(".claude").join("settings.json");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, serde_json::to_string_pretty(v).unwrap()).unwrap();
        p
    }

    #[test]
    fn entries_as_install_writes_them_are_not_drift_whatever_the_binary_path() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), &current_settings(false));
        assert_eq!(hook_drift(&p, false), None);
        let p = write(d.path(), &current_settings(true));
        assert_eq!(hook_drift(&p, true), None);
    }

    #[test]
    fn a_hook_added_since_the_install_is_named_as_missing() {
        // The real case: entries from 0.8.4, binary 0.8.5, no SubagentStart.
        let d = tempfile::tempdir().unwrap();
        let mut v = current_settings(false);
        v["hooks"].as_object_mut().unwrap().remove("SubagentStart");
        let p = write(d.path(), &v);
        let line = hook_drift(&p, false).expect("drift");
        assert!(line.contains("missing SubagentStart"), "{line}");
        assert!(
            line.contains("cyberbrain install --client claude-code"),
            "{line}"
        );
    }

    #[test]
    fn a_changed_entry_and_an_event_we_no_longer_use_are_named() {
        let d = tempfile::tempdir().unwrap();
        let mut v = current_settings(false);
        // Before 0.7.10 session start was one hook; before the timeout there was none.
        v["hooks"]["SessionStart"][0]["hooks"] = json!([{
            "type": "command", "command": "/old/cyberbrain", "args": ["hook", "session-start"]
        }]);
        v["hooks"]["Notification"] =
            json!([{ "matcher": "", "hooks": [], "_managedBy": "cyberbrain" }]);
        let p = write(d.path(), &v);
        let line = hook_drift(&p, false).expect("drift");
        assert!(line.contains("different SessionStart"), "{line}");
        assert!(line.contains("no longer used Notification"), "{line}");
        assert!(!line.contains("missing"), "{line}");
    }

    #[test]
    fn enforce_mode_without_fail_closed_entry_is_drift() {
        let d = tempfile::tempdir().unwrap();
        let p = write(d.path(), &current_settings(false));
        let line = hook_drift(&p, true).expect("drift");
        assert!(line.contains("different PreToolUse"), "{line}");
    }

    #[test]
    fn a_file_without_our_entries_or_without_a_file_says_nothing() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(hook_drift(&d.path().join("none.json"), false), None);
        let p = write(
            d.path(),
            &json!({ "hooks": { "Stop": [{ "matcher": "", "hooks": [] }] } }),
        );
        assert_eq!(hook_drift(&p, false), None);
        let p = write(d.path(), &json!({ "permissions": {} }));
        assert_eq!(hook_drift(&p, false), None);
    }

    #[test]
    fn a_process_on_a_deleted_cyberbrain_binary_is_reported_others_are_not() {
        let procs = vec![
            Proc {
                pid: 11,
                exe: "/usr/local/bin/cyberbrain (deleted)".into(),
                cmdline: "/usr/local/bin/cyberbrain --store /p/.cyberbrain mcp --http 127.0.0.1:8"
                    .into(),
            },
            Proc {
                pid: 12,
                exe: "/usr/local/bin/cyberbrain".into(),
                cmdline: "cyberbrain daemon".into(),
            },
            Proc {
                pid: 13,
                exe: "/usr/bin/python3 (deleted)".into(),
                cmdline: "python3 cyberbrain.py".into(),
            },
        ];
        let line = replaced_processes(&procs).expect("one stale");
        assert!(line.starts_with("**1 cyberbrain process(es)"), "{line}");
        assert!(line.contains("pid 11"), "{line}");
        assert!(
            !line.contains("pid 12") && !line.contains("pid 13"),
            "{line}"
        );
        assert_eq!(replaced_processes(&procs[1..]), None);
    }

    #[test]
    fn long_command_lines_are_cut_and_control_characters_dropped() {
        let s = short(&format!("cyberbrain\u{1b}[31m {}", "x".repeat(200)));
        assert!(s.chars().count() <= 70 && !s.contains('\u{1b}'), "{s}");
    }
}
