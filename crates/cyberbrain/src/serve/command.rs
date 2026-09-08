//! `POST /command`: the command line, in the window.
//!
//! Every other endpoint here is a shape the page understands. This one is the command line
//! itself, and it is the real one: the line is tokenised, checked, and handed to **this same
//! binary** as arguments. What comes back is what a terminal would have shown, exit code
//! included.
//!
//! A subprocess rather than a call into `App`, and that is the whole design decision. The
//! alternative is a second dispatch beside the one in `main.rs` — a second place where
//! `write` decides what the PII gate means, a second place where an exit code is chosen, and
//! a second place to forget when a command grows an argument. SPEC §8 already refuses that
//! trade for `--dry-run`; it is the same trade. Two processes on one store is the condition
//! this store was built for anyway: the index and the audit log are WAL with a five-second
//! busy timeout, because `cyberbrain scan` at a prompt while `serve` is running has always
//! been allowed.
//!
//! **No shell.** The line is split here and the pieces become `argv` directly, so there is
//! nothing for a quote or a semicolon to escape into.

use super::error::{ApiError, ApiResult};
use super::{ServeState, blocking};
use crate::cli::{Cli, Command};
use axum::Json;
use axum::extract::State;
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Long enough for a full rescan of a large store, short enough that a wedged child is not
/// a wedged page. A command that hits it is killed and says so.
const TIMEOUT: Duration = Duration::from_secs(120);

/// Output past this is cut. `recall -n 100` or an `export` of a long note can be large, and
/// what the page cannot draw it does not need to be sent.
const MAX_OUTPUT: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
pub struct CommandRequest {
    /// The line as typed, without the leading `cyberbrain`.
    pub line: String,
}

#[derive(Debug, Serialize)]
pub struct CommandResult {
    /// What was actually run, after tokenising. Shown back so a person can see how their
    /// quotes were read.
    pub argv: Vec<String>,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// True when output was cut at [`MAX_OUTPUT`].
    pub truncated: bool,
}

pub async fn run(
    State(st): State<Arc<ServeState>>,
    Json(req): Json<CommandRequest>,
) -> ApiResult<Json<CommandResult>> {
    let argv = tokenise(&req.line)
        .map_err(ApiError::bad_request)?
        .ok_or_else(|| ApiError::bad_request("there is no command in that line"))?;

    // The store is this window's, and saying otherwise is the one way this endpoint could
    // reach a store the person is not looking at.
    if argv
        .iter()
        .any(|a| a == "--store" || a.starts_with("--store="))
    {
        return Err(ApiError::bad_request(
            "`--store` is not accepted here: a command typed in this window runs against \
             this window's store. Open the other project in its own window.",
        ));
    }

    // Parsed here as well as by the child, because this is the check. A line that does not
    // parse gets clap's own message, which is the one a terminal would have given.
    let parsed = Cli::try_parse_from(std::iter::once("cyberbrain".to_string()).chain(argv.clone()))
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    if let Some(why) = refuse(&parsed.command) {
        return Err(ApiError::bad_request(why));
    }

    let exe = st.self_exe.clone();
    let root = st.app.root().to_path_buf();
    let args = argv.clone();

    let out = blocking(move || {
        let mut child = std::process::Command::new(&exe)
            .arg("--store")
            .arg(&root)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| ApiError::internal(format!("the command could not be started: {e}")))?;
        wait_with_timeout(&mut child)
    })
    .await?;

    let (stdout, cut_out) = cut(out.stdout);
    let (stderr, cut_err) = cut(out.stderr);
    Ok(Json(CommandResult {
        argv,
        stdout,
        stderr,
        exit_code: out.code,
        truncated: cut_out || cut_err,
    }))
}

struct Finished {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: i32,
}

/// Wait for the child, and kill it if it outstays [`TIMEOUT`].
///
/// The pipes are drained on threads of their own rather than after the wait: a child that
/// fills a pipe nobody is reading blocks on the write, and a wait for a process that is
/// blocked on us is a wait that never ends.
fn wait_with_timeout(child: &mut std::process::Child) -> ApiResult<Finished> {
    use std::io::Read;
    let mut out = child.stdout.take();
    let mut err = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = out.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = err.as_mut() {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = std::time::Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {}
            Err(e) => {
                return Err(ApiError::internal(format!(
                    "the command could not be waited for: {e}"
                )));
            }
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout = out.join().unwrap_or_default();
    let mut stderr = err.join().unwrap_or_default();
    match status {
        Some(s) => Ok(Finished {
            stdout,
            stderr,
            // A signal leaves no code. 128 + signal is what a shell reports, and the number
            // matters less than not claiming success.
            code: s.code().unwrap_or(-1),
        }),
        None => {
            stderr.extend_from_slice(
                format!(
                    "\ncyberbrain: no answer after {} seconds, so the command was stopped.\n",
                    TIMEOUT.as_secs()
                )
                .as_bytes(),
            );
            Ok(Finished {
                stdout,
                stderr,
                code: -1,
            })
        }
    }
}

fn cut(bytes: Vec<u8>) -> (String, bool) {
    let cut = bytes.len() > MAX_OUTPUT;
    let mut text = String::from_utf8_lossy(&bytes).into_owned();
    if cut {
        // On a char boundary, because the cut is in bytes and the text is not.
        let mut end = MAX_OUTPUT.min(text.len());
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n…output cut here.\n");
    }
    (text, cut)
}

/// Which commands this surface will not run, and why in each case.
///
/// Written as an exhaustive match on purpose. A command added to the CLI stops this
/// compiling until somebody decides whether it belongs in a window, which is the only way
/// a list like this stays true.
fn refuse(command: &Command) -> Option<&'static str> {
    match command {
        // Fine here: they read or change this store, which is what this window is.
        Command::Scan { .. }
        | Command::Recall { .. }
        | Command::Find { .. }
        | Command::Write { .. }
        | Command::Forget { .. }
        | Command::Doctor
        | Command::Status
        | Command::Export { .. }
        // The review workflow belongs here more than anywhere: it is this store's own
        // business, and the window is where the person reviewing already is.
        | Command::Propose { .. }
        | Command::Review { .. } => None,

        // Not the whole of `policy`: one of its subcommands writes a file wherever it is
        // pointed, and this surface has no authentication.
        Command::Policy { command } => refuse_policy(command),

        Command::Serve { .. } => {
            Some("`serve` is what is answering this: the page you are reading is a running one.")
        }
        Command::Mcp => {
            Some("`mcp` speaks a protocol over stdin and stdout, and there is no terminal here.")
        }
        Command::Hook { .. } => {
            Some("`hook` is called by an agent harness, with the session's payload on stdin.")
        }
        Command::Init { .. } => {
            Some("`init` creates a store, and this window already has one open.")
        }
        Command::Install { .. } => Some(
            "`install` writes into other programs' configuration files, which is not this \
             store's business. It is in the launcher's menu, and at a prompt.",
        ),
        // Both of these read files from anywhere on disk by name. This surface is loopback
        // and has no authentication because nothing it holds leaves the machine — turning it
        // into a way to read any file would make that sentence false.
        Command::Import { .. } => Some(
            "`import` reads a plan and every file it names from anywhere on disk. This \
             surface has no authentication, so it is a prompt's job.",
        ),
        Command::VerifyExport { .. } => Some(
            "`verify-export` reads a file somebody handed you, from wherever you put it. \
             This surface has no authentication, so it is a prompt's job.",
        ),
        Command::Hub { .. } => Some(
            "`hub` is a different surface with an authentication of its own, and this is not it.",
        ),
    }
}

/// The `policy` subcommands, and the one that names a path.
///
/// The reasoning that kept `import` and `verify-export` out was right and half-sized: it
/// said those two read files from anywhere on disk, and stopped there. `policy audit
/// --export <path>` is the same class in the other direction, and it was the only argument
/// of its kind among the commands that were let through — so a `POST /command` from any
/// local process wrote a file wherever it liked, over whatever was already there, including
/// this store's own hash-chained audit log.
///
/// Exhaustive on `PolicyCommand`, so a subcommand added later has to be decided here rather
/// than inherited.
fn refuse_policy(command: &crate::cli::PolicyCommand) -> Option<&'static str> {
    use crate::cli::PolicyCommand as P;
    match command {
        P::Audit {
            export: Some(_), ..
        } => Some(
            "`policy audit --export` writes a file wherever it is pointed, and this surface \
             has no authentication. Read the log without it, or export it at a prompt.",
        ),
        P::Audit { .. }
        | P::Egress
        | P::Obligations
        | P::Subject { .. }
        | P::Retention { .. }
        | P::ModelCard
        | P::Consent { .. } => None,
    }
}

/// Split a typed line into arguments the way a person expects, and no further.
///
/// Single and double quotes group, a backslash escapes the next character, and whitespace
/// separates. Deliberately not a shell: there is no variable expansion, no glob, no `&&`,
/// no pipe and no redirection, because none of those would be honoured downstream and a
/// half-shell is worse than none. An unclosed quote is an error, not a guess.
pub fn tokenise(line: &str) -> Result<Option<Vec<String>>, String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars();

    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') => match chars.next() {
                Some(next) => {
                    cur.push(next);
                    has = true;
                }
                None => return Err("the line ends in a backslash with nothing after it".into()),
            },
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => {
                cur.push(c);
                has = true;
            }
            (None, '\'') | (None, '"') => {
                quote = Some(c);
                // An empty quoted string is an argument: `--body ""` means something.
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if quote.is_some() {
        return Err("there is a quote in that line that is never closed".into());
    }
    if has {
        out.push(cur);
    }
    Ok((!out.is_empty()).then_some(out))
}
