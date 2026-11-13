//! The compaction mod for Claude Code (`install --compact-mod`).
//!
//! A Claude Code mod is a plugin folder with a hooks module in TypeScript. This one hooks
//! `session.compact`, hands the conversation to `cyberbrain hook pre-compact-snapshot` and
//! adds what that prints to the summarizer's instructions (`hook::snapshot`). It is written
//! into the project's `.claude/skills/cyberbrain-compact`, a folder Claude Code loads plugins
//! from, because the hooks are per project too.
//!
//! The same three rules as for settings.json: a folder that is not ours is left as it is, ours
//! is recognised by its manifest's name, and an undo takes out our three files and nothing
//! else. Opt-in, because function hooks are an early-access interface: the shell `PreCompact`
//! hook keeps counting with or without it.

use super::edit::{Action, Change};
use cyberbrain_core::{Error, Result, Slash};
use std::path::{Path, PathBuf};

pub const NAME: &str = "cyberbrain-compact";
const PLUGIN_JSON: &str = include_str!("compact_mod/plugin.json");
const HOOKS_JSON: &str = include_str!("compact_mod/hooks.json");
const REGISTER_TS: &str = include_str!("compact_mod/register.ts");
/// The line the binary's path replaces. The file in the repository runs `cyberbrain` from
/// `PATH`, so it also works when loaded by hand with `claude --plugin-dir`.
const BINARY_LINE: &str = "const BINARY = 'cyberbrain'";
const WHY: &str = "the compaction mod (--compact-mod), in the project you named";

pub fn folder(project: &Path) -> PathBuf {
    project.join(".claude").join("skills").join(NAME)
}

/// The three files, relative to [`folder`], with `binary` written in.
fn files(binary: &Path) -> Vec<(PathBuf, String)> {
    // A JSON string is a JavaScript string: quotes and backslashes (a Windows path) escaped.
    let quoted = serde_json::to_string(&binary.display().to_string())
        .unwrap_or_else(|_| "'cyberbrain'".to_string());
    vec![
        (
            Path::new(".claude-plugin").join("plugin.json"),
            PLUGIN_JSON.to_string(),
        ),
        (
            Path::new("hooks").join("hooks.json"),
            HOOKS_JSON.to_string(),
        ),
        (
            Path::new("hooks").join("register.ts"),
            REGISTER_TS.replacen(BINARY_LINE, &format!("const BINARY = {quoted}"), 1),
        ),
    ]
}

/// Whether the folder holds a plugin, and whether it is ours.
fn owner(dir: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(dir.join(".claude-plugin").join("plugin.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(v.get("name").and_then(|n| n.as_str()) == Some(NAME))
}

/// Write or remove the mod. `None` when there is nothing to report: not asked for, or an undo
/// with nothing of ours there.
pub fn apply(
    project: &Path,
    binary: &Path,
    wanted: bool,
    undo: bool,
    dry_run: bool,
) -> Result<Option<Change>> {
    let dir = folder(project);
    let change = |action| Change {
        path: dir.clone(),
        why: WHY,
        action,
        backup: None,
    };
    let ours = owner(&dir);
    if ours == Some(false) {
        return Ok((wanted || undo).then(|| change(Action::NotOurs)));
    }
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |e| Error::Io { path, source: e }
    };
    if undo {
        if ours.is_none() {
            return Ok(None);
        }
        if !dry_run {
            for (rel, _) in files(binary) {
                let p = dir.join(&rel);
                match std::fs::remove_file(&p) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(io(&p)(e)),
                }
            }
            // Directories only when empty: anything else in them is somebody's.
            for sub in [".claude-plugin", "hooks", ""] {
                let _ = std::fs::remove_dir(dir.join(sub));
            }
        }
        return Ok(Some(change(Action::Removed)));
    }
    if !wanted {
        return Ok(None);
    }
    let files = files(binary);
    let same = files
        .iter()
        .all(|(rel, text)| std::fs::read_to_string(dir.join(rel)).is_ok_and(|t| t == *text));
    if same {
        return Ok(Some(change(Action::Unchanged)));
    }
    if !dry_run {
        for (rel, text) in &files {
            let p = dir.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).map_err(io(parent))?;
            }
            std::fs::write(&p, text).map_err(io(&p))?;
        }
    }
    Ok(Some(change(if ours.is_some() {
        Action::Updated
    } else {
        Action::Added
    })))
}

/// What a person has to know after the mod was written.
pub fn note(dir: &Path) -> String {
    format!(
        "The compaction mod is in {d}, a folder Claude Code loads plugins from at session \
         start. Function hooks are early access: if your Claude Code does not load it, start \
         with `claude --plugin-dir {d}`. Before each compaction it keeps \
         a snapshot of the conversation in ring 3 and keeps citations in the summary; nothing \
         leaves this machine. The PreCompact hook in settings.json stays as the fallback.",
        d = Slash(dir)
    )
}
