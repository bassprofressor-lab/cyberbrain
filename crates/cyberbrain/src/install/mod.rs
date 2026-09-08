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
const EVENTS: [(&str, &str, &str); 6] = [
    ("SessionStart", "session-start", ""),
    ("UserPromptSubmit", "user-prompt-submit", ""),
    ("PreToolUse", "pre-tool-use", TOOL_MATCHER),
    ("PostToolUse", "post-tool-use", TOOL_MATCHER),
    ("Stop", "stop", ""),
    ("PreCompact", "pre-compact", ""),
];
const TOOL_MATCHER: &str = "Edit|Write|MultiEdit|NotebookEdit";

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

fn claude_code(opts: &Options, binary: &Path) -> Result<ClientReport> {
    let path = paths::claude_code(&opts.project);
    if opts.undo && !path.exists() {
        return Ok(ClientReport::absent(
            Client::ClaudeCode,
            format!("{} does not exist; nothing to undo", Slash(&path)),
        ));
    }

    let whose = "settings.json";
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
                    if opts.undo {
                        touched |= had;
                        continue;
                    }
                    let entry = hook_entry(binary, matcher, arg);
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
            Ok(match (opts.undo, touched, changed) {
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
        note: None,
        snippet: None,
    })
}

fn hook_entry(binary: &Path, matcher: &str, arg: &str) -> Value {
    json!({
        "matcher": matcher,
        "hooks": [{
            "type": "command",
            "command": binary.display().to_string(),
            "args": ["hook", arg],
        }],
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

    let note = (changes.len() > 1).then(|| {
        "This machine has more than one Claude Desktop installation, and only one of them \
         reads the file its own Edit Config button opens. Both were written, so it works \
         either way."
            .to_string()
    });
    Ok(ClientReport {
        client: Client::ClaudeDesktop.name(),
        found: true,
        changes,
        note,
        snippet: None,
    })
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
        .unwrap_or_else(|| PathBuf::from("cyberbrain"))
}

fn absolute(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}
