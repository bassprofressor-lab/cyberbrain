//! `cyberbrain install`: write this binary into the agent's own configuration.
//!
//! The hooks and the MCP server have existed since the first release. What did not exist
//! was a way to switch them on without being told which JSON file to edit and where it
//! lives — and "edit this file by hand" is not a setup step, it is a support ticket. That
//! is the whole of this module.
//!
//! It writes files that belong to other programs, so it holds to three rules everywhere:
//! anything that is not ours is copied through untouched, our own entries carry a marker so
//! a later run can find them again, and the previous file is kept beside the new one.
//!
//! **What it deliberately does not do.** ChatGPT gets no entry, because there is nothing to
//! write: its MCP support wants an HTTPS endpoint with OAuth, which means a server reachable
//! from the internet. Cyberbrain's promise is that a note never leaves the machine (SPEC
//! §12.1), so the honest answer there is a sentence, not a button. Codex CLI does speak
//! stdio and is configured by file, but that file is TOML the user has written by hand, with
//! their own comments in it; rather than reformat somebody's configuration we print the
//! three lines to paste. Both appear in the report, so a person learns the client is usable
//! rather than assuming it is not.

mod edit;
mod paths;
#[cfg(test)]
mod tests;

pub use edit::{Action, Change};
pub use paths::Env;

use cyberbrain_core::{Result, Slash};
use edit::edit;
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The marker on our own entries. Same string in a Claude Code settings file as in any
/// other, so one grep answers "did cyberbrain put this here".
pub const MANAGED_BY: &str = "cyberbrain";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    ClaudeDesktop,
    Codex,
}

impl Client {
    pub const ALL: [Client; 3] = [Client::ClaudeCode, Client::ClaudeDesktop, Client::Codex];

    pub fn name(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::ClaudeDesktop => "claude-desktop",
            Client::Codex => "codex",
        }
    }
}

pub struct Options {
    /// Empty means every client, which is the useful default: a person setting up a machine
    /// wants the ones they have, and does not yet know which those are.
    pub clients: Vec<Client>,
    /// The project whose `.claude/settings.json` gets the hooks.
    pub project: PathBuf,
    /// The store the MCP entries point at. Absolute: a desktop client has no working
    /// directory of ours, so the path in its configuration is the only thing that says
    /// which project it is talking about.
    pub store: PathBuf,
    /// The name of the MCP entry. One store, one entry; a second project on the same
    /// machine wants a second name, which is what this is for.
    pub name: String,
    pub undo: bool,
    pub dry_run: bool,
    pub env: Env,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub binary: PathBuf,
    #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
    pub store: PathBuf,
    pub name: String,
    pub undo: bool,
    pub dry_run: bool,
    pub clients: Vec<ClientReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientReport {
    pub client: &'static str,
    /// Whether this client is on the machine at all. A client that is not installed is not
    /// a failure, and saying "not found" beats writing a file nothing will ever read.
    pub found: bool,
    pub changes: Vec<Change>,
    /// What a person still has to do, or why nothing was done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Configuration to paste, for a client this command does not write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

impl ClientReport {
    fn absent(client: Client, note: impl Into<String>) -> ClientReport {
        ClientReport {
            client: client.name(),
            found: false,
            changes: Vec::new(),
            note: Some(note.into()),
            snippet: None,
        }
    }
}

/// The six lifecycle events, the CLI argument each one takes, and which tool calls it wants
/// to see.
///
/// The matcher on the two tool events is not a narrowing for speed: `Payload::edited_file`
/// answers for `Edit`, `Write`, `MultiEdit` and `NotebookEdit` and returns `None` for
/// everything else, so a wider matcher would start a process per tool call to be told there
/// is nothing to check. The two lists are the same list, and they have to stay that way —
/// `the_matcher_covers_every_tool_the_hook_reads` is what says so.
/// `matcher` is a regular expression over the tool name; empty means every call of the
/// event, which is what the four non-tool events want.
///
/// Session start runs as two hooks in one entry (0.7.10): Claude Code caps each hook's output
/// at 10,000 characters on its own, so rings 0/1 get the whole cap and the handoff and digest
/// another one, instead of sharing one and pushing the rings into a preview.
pub(crate) const EVENTS: [(&str, &[&str], &str); 6] = [
    ("SessionStart", &["session-rings", "session-context"], ""),
    ("UserPromptSubmit", &["user-prompt-submit"], ""),
    ("PreToolUse", &["pre-tool-use"], PRE_TOOL_MATCHER),
    ("PostToolUse", &["post-tool-use"], TOOL_MATCHER),
    ("Stop", &["stop"], ""),
    ("PreCompact", &["pre-compact"], ""),
];
/// Seconds before the harness gives up on a hook. Without one Claude Code waits 600 s: a hook
/// stuck on a lock would hold every edit for ten minutes. The hooks answer in milliseconds
/// (budget_ms); 30 s leaves room for a cold disk and a slow AgentGuard, which has its own
/// shorter timeout inside the hook.
pub(crate) const HOOK_TIMEOUT_SECS: u64 = 30;
const TOOL_MATCHER: &str = "Edit|Write|MultiEdit|NotebookEdit";
/// Pre-tool-use also reads `Bash` (2026-09-25): a shell command can do what the file tools
/// are refused, and it is where an agent would drop `CLAUDECODE` (`hook::events::bash_verdict`).
/// Post-tool-use has nothing to do for a shell command and keeps the narrow matcher.
/// WebFetch since the governance check (2026-09-25, `hook::governance::TOOLS`).
const PRE_TOOL_MATCHER: &str = "Edit|Write|MultiEdit|NotebookEdit|Bash|WebFetch";

pub fn run(opts: &Options) -> Result<Report> {
    let binary = current_binary();
    let store = absolute(&opts.store);
    let wanted = if opts.clients.is_empty() {
        Client::ALL.to_vec()
    } else {
        opts.clients.clone()
    };

    let mut clients = Vec::new();
    for client in wanted {
        let report = match client {
            Client::ClaudeCode => claude_code(opts, &binary)?,
            Client::ClaudeDesktop => claude_desktop(opts, &binary, &store)?,
            Client::Codex => codex(opts, &binary, &store),
        };
        clients.push(report);
    }

    Ok(Report {
        binary,
        store,
        name: opts.name.clone(),
        undo: opts.undo,
        dry_run: opts.dry_run,
        clients,
    })
}

// ---------------------------------------------------------------------------------------
// Claude Code: hooks, in the project.

/// Whether the Cyberbrain plugin is installed for Claude Code (2026-10-04). The plugin brings
/// its own hooks; ours in settings.json would make the harness run every event twice. Any
/// key `cyberbrain@<marketplace>` counts, whichever marketplace it came from. A file that is
/// missing or does not parse means "no plugin": the old behaviour, which is safe.
pub fn plugin_installed(env: &Env) -> Option<String> {
    let path = paths::claude_plugins(env)?;
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("plugins")?
        .as_object()?
        .keys()
        .find(|k| k.split('@').next() == Some("cyberbrain"))
        .cloned()
}

/// Whether session start runs as two hooks (`session-rings` + `session-context`, 0.7.10) for
/// this project: in its `.claude/settings*.json`, or in an installed Cyberbrain plugin's
/// `hooks/hooks.json`. `doctor` measures against the cap that applies: each half on its own
/// when split, the whole text when not. Unreadable files count as "not split", the stricter
/// answer.
pub fn session_start_split(project: &Path, env: &Env) -> bool {
    let mentions =
        |p: &Path| std::fs::read_to_string(p).is_ok_and(|t| t.contains("\"session-rings\""));
    let dir = project.join(".claude");
    if mentions(&dir.join("settings.json")) || mentions(&dir.join("settings.local.json")) {
        return true;
    }
    let Some(reg) = paths::claude_plugins(env) else {
        return false;
    };
    let Some(v) = std::fs::read_to_string(reg)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    else {
        return false;
    };
    let Some(plugins) = v.get("plugins").and_then(Value::as_object) else {
        return false;
    };
    plugins
        .iter()
        .filter(|(k, _)| k.split('@').next() == Some("cyberbrain"))
        .flat_map(|(_, list)| list.as_array().into_iter().flatten())
        .filter_map(|i| i.get("installPath").and_then(Value::as_str))
        .any(|p| mentions(&Path::new(p).join("hooks").join("hooks.json")))
}

fn claude_code(opts: &Options, binary: &Path) -> Result<ClientReport> {
    let path = paths::claude_code(&opts.project);
    // With the plugin installed, writing our hooks would double them; take ours out instead.
    let plugin = plugin_installed(&opts.env);
    let remove = opts.undo || plugin.is_some();
    if remove && !path.exists() {
        if let Some(id) = &plugin {
            return Ok(ClientReport {
                client: Client::ClaudeCode.name(),
                found: true,
                changes: Vec::new(),
                note: Some(format!(
                    "the plugin {id} provides the hooks; nothing to write here"
                )),
                snippet: None,
            });
        }
        return Ok(ClientReport::absent(
            Client::ClaudeCode,
            format!("{} does not exist; nothing to undo", Slash(&path)),
        ));
    }

    let whose = "settings.json";
    // In enforce mode the pre-tool-use hook is the gate in front of AgentGuard, so it fails
    // closed (0.8.2): a hook that cannot start or times out blocks the tool call instead of
    // letting it through. Shadow mode never stops anything, and a missing binary there should
    // not stop every edit either. An unreadable store config counts as shadow.
    let fail_closed = cyberbrain_core::config::Config::load(&opts.store)
        .map(|c| c.governance.mode == cyberbrain_core::config::GovernanceMode::Enforce)
        .unwrap_or(false);
    let change = edit(
        &path,
        "the hooks are per project, so this is the project you named",
        opts.dry_run,
        |root| {
            let mut touched = false;
            let mut changed = false;
            {
                let hooks = edit::object(root, "hooks", whose)?;
                for (event, arg, matcher) in EVENTS {
                    let list = edit::array(hooks, event, whose)?;
                    let ours =
                        |v: &Value| v.get("_managedBy").and_then(Value::as_str) == Some(MANAGED_BY);
                    let before = list.iter().find(|v| ours(v)).cloned();
                    let had = before.is_some();
                    list.retain(|v| !ours(v));
                    if remove {
                        touched |= had;
                        continue;
                    }
                    let entry =
                        hook_entry(binary, matcher, arg, fail_closed && event == "PreToolUse");
                    changed |= before.as_ref() != Some(&entry);
                    touched = true;
                    list.push(entry);
                }
                // An `"SessionStart": []` left behind by an undo is litter in somebody
                // else's file.
                hooks.retain(|_, v| !v.as_array().is_some_and(|a| a.is_empty()));
            }
            if root
                .get("hooks")
                .and_then(Value::as_object)
                .is_some_and(|h| h.is_empty())
            {
                root.remove("hooks");
            }
            Ok(match (remove, touched, changed) {
                (true, true, _) => Action::Removed,
                (true, false, _) => Action::NothingToUndo,
                (false, _, true) => Action::Added,
                (false, _, false) => Action::Unchanged,
            })
        },
    )?;

    Ok(ClientReport {
        client: Client::ClaudeCode.name(),
        found: true,
        changes: vec![change],
        note: plugin.filter(|_| !opts.undo).map(|id| {
            format!("the plugin {id} provides the hooks, so the entries in settings.json were taken out (they would run every event twice)")
        }),
        snippet: None,
    })
}

/// `fail_closed` adds `"onFailure": "block"` (Claude Code 2.1.295 and later): a hook that
/// cannot start, times out or prints output that does not parse then blocks the action, as
/// exit code 2 would. Our hooks always exit 0 and report problems on stdout, so a store that
/// cannot be opened still answers; what this catches is a binary that is gone or hangs.
/// Older Claude Code versions do not know the field.
fn hook_entry(binary: &Path, matcher: &str, args: &[&str], fail_closed: bool) -> Value {
    let hooks: Vec<Value> = args
        .iter()
        .map(|arg| {
            let mut h = json!({
                "type": "command",
                "command": binary.display().to_string(),
                "args": ["hook", arg],
                "timeout": HOOK_TIMEOUT_SECS,
            });
            if fail_closed {
                h["onFailure"] = json!("block");
            }
            h
        })
        .collect();
    json!({
        "matcher": matcher,
        "hooks": hooks,
        "_managedBy": MANAGED_BY,
    })
}

// ---------------------------------------------------------------------------------------
// Claude Desktop: one MCP entry, in as many files as this machine has installations of it.

fn claude_desktop(opts: &Options, binary: &Path, store: &Path) -> Result<ClientReport> {
    let places = paths::claude_desktop(&opts.env);
    let candidates = paths::resolve(&places);
    if candidates.is_empty() {
        return Ok(ClientReport::absent(
            Client::ClaudeDesktop,
            "Claude Desktop is not installed for this user; nothing was written. Install it, \
             then run this again."
                .to_string(),
        ));
    }

    let entry = mcp_entry(binary, store);
    let mut changes = Vec::new();
    for candidate in &candidates {
        let change = edit(&candidate.path, candidate.why, opts.dry_run, |root| {
            let servers = edit::object(root, "mcpServers", "claude_desktop_config.json")?;
            let before = servers.get(&opts.name).cloned();
            // The entry carries no marker (Claude Desktop's own format), so what it starts is
            // how it is recognised. Somebody else's server under this name is theirs.
            if before
                .as_ref()
                .is_some_and(|b| !is_our_mcp_entry(b) && b["command"] != entry["command"])
            {
                return Ok(Action::NotOurs);
            }
            if opts.undo {
                servers.remove(&opts.name);
                let empty = servers.is_empty();
                if empty {
                    root.remove("mcpServers");
                }
                return Ok(if before.is_some() {
                    Action::Removed
                } else {
                    Action::NothingToUndo
                });
            }
            servers.insert(opts.name.clone(), entry.clone());
            Ok(match before {
                Some(b) if b == entry => Action::Unchanged,
                Some(_) => Action::Updated,
                None => Action::Added,
            })
        })?;
        changes.push(change);
    }

    let mut note = (changes.len() > 1).then(|| {
        "This machine has more than one Claude Desktop installation, and only one of them \
         reads the file its own Edit Config button opens. Both were written, so it works \
         either way."
            .to_string()
    });
    if changes.iter().any(|c| c.action == Action::NotOurs) {
        let theirs = format!(
            "An MCP server named `{}` is already configured and it does not start cyberbrain, \
             so it was left as it is. Choose another name with --name.",
            opts.name
        );
        note = Some(match note {
            Some(n) => format!("{n} {theirs}"),
            None => theirs,
        });
    }
    Ok(ClientReport {
        client: Client::ClaudeDesktop.name(),
        found: true,
        changes,
        note,
        snippet: None,
    })
}

/// Whether an `mcpServers` entry is one `install` wrote: it runs a program called
/// `cyberbrain` with `mcp` as its first argument. A binary that moved, or another store, is
/// still ours to update; a different program under the same name is not. An entry that runs
/// this very binary is ours too, whatever it is called.
fn is_our_mcp_entry(entry: &Value) -> bool {
    let program = entry["command"]
        .as_str()
        .and_then(|c| c.rsplit(['/', '\\']).next())
        .map(|p| p.to_ascii_lowercase());
    let program = program
        .as_deref()
        .map(|p| p.strip_suffix(".exe").unwrap_or(p));
    program == Some("cyberbrain") && entry["args"][0].as_str() == Some("mcp")
}

fn mcp_entry(binary: &Path, store: &Path) -> Value {
    json!({
        "command": binary.display().to_string(),
        // `--store` and not a working directory: a desktop client starts this process
        // wherever it likes, and the store walk would find another project's memory or
        // none at all.
        "args": ["mcp", "--store", store.display().to_string()],
    })
}

// ---------------------------------------------------------------------------------------
// Codex CLI: reported and shown, not written. See the module documentation.

fn codex(opts: &Options, binary: &Path, store: &Path) -> ClientReport {
    let Some(config) = paths::codex_config(&opts.env) else {
        return ClientReport::absent(
            Client::Codex,
            "Codex CLI has not been run on this machine (no ~/.codex); nothing to set up."
                .to_string(),
        );
    };
    let b = binary.display();
    let s = store.display();
    let snippet = format!(
        "codex mcp add {name} -- {b} mcp --store {s}\n\n\
         or, in {config}:\n\n\
         [mcp_servers.{name}]\n\
         command = \"{b}\"\n\
         args = [\"mcp\", \"--store\", \"{s}\"]\n",
        name = opts.name,
        config = Slash(&config),
    );
    ClientReport {
        client: Client::Codex.name(),
        found: true,
        changes: Vec::new(),
        note: Some(format!(
            "{} is yours, with your comments in it, and this command does not rewrite it. \
             Paste one of these instead.",
            Slash(&config)
        )),
        snippet: Some(snippet),
    }
}

// ---------------------------------------------------------------------------------------

/// The absolute path of the running binary.
///
/// Absolute because the entry outlives the shell that made it: the client starts this
/// process with a `PATH` and a working directory that are not the ones here, and a bare
/// name would resolve to whichever cyberbrain that environment happens to have, or none.
fn current_binary() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok().or(Some(p)))
        .map(plain)
        .unwrap_or_else(|| PathBuf::from("cyberbrain"))
}

fn absolute(p: &Path) -> PathBuf {
    plain(p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
}

/// `canonicalize` on Windows hands back the extended-length form, `\\?\C:\x`. Every
/// Windows API accepts it, so nothing was broken — but it is what landed in
/// `settings.json`, a file a person reads and another program passes on, and a path there
/// should look like a path. Only the two shapes that name the same file without the prefix
/// are unwrapped; anything else keeps it, because for those the prefix is load-bearing.
#[cfg(windows)]
fn plain(p: PathBuf) -> PathBuf {
    match p.to_str().and_then(without_verbatim_prefix) {
        Some(s) => PathBuf::from(s),
        None => p,
    }
}

#[cfg(not(windows))]
fn plain(p: PathBuf) -> PathBuf {
    p
}

/// Compiled on Windows, and in every test build so the table below runs everywhere rather
/// than only on the platform where it is hardest to notice that it stopped.
#[cfg(any(windows, test))]
fn without_verbatim_prefix(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    let rest = path.strip_prefix(r"\\?\")?;
    // A drive letter, a colon, and a separator: `\\?\C:\x` is `C:\x` and nothing else.
    let b = rest.as_bytes();
    match (b.first(), b.get(1), b.get(2)) {
        (Some(d), Some(b':'), Some(b'\\')) if d.is_ascii_alphabetic() => Some(rest.to_string()),
        _ => None,
    }
}
