//! `cyberbrain hook <event>`: the agent-harness integration (SPEC §9.1).
//!
//! The harness pipes a JSON payload to stdin and reads stdout. Six events, two channels:
//! `session-start` and `user-prompt-submit` have their plain stdout added to the model's
//! context; `pre-tool-use` and `post-tool-use` reach the model only through a JSON
//! decision (`permissionDecision`, `additionalContext`); `stop` and `pre-compact` reach a
//! debug log and, via `systemMessage`, the human. Everything here is shaped by those
//! channels and by two rules that are requirements, not preferences:
//!
//! - **The budget is the design.** 15 ms p99 for every event but `session-start` (150 ms).
//!   No embedder (§6.5), no inference (§11), no full scan, and no work whose cost grows
//!   with the size of the store. What each event does is therefore bounded by the resident
//!   rings, which are size-capped (§3.2), and by the one payload in hand.
//! - **A hook never fails the harness.** [`run`] returns exit code 0 with empty stdout on
//!   any internal error or panic, after recording the error to the store when one is
//!   reachable. A memory tool that can break the agent it serves gets uninstalled.
//!
//! What each event does, and what it deliberately does not, is documented on
//! [`events`]. This file is the wrapper: parsing the switch, catching everything, and
//! announcing when the tool is standing down (§9.1: silent inaction is indistinguishable
//! from a broken hook).
//!
//! Nothing in this module opens the index, loads a model, or contacts an endpoint. It
//! uses [`App`] for the store handle, the policy (audit log) and the configuration, and
//! the file system for rings 0 and 1.

use crate::app::{App, discover_store};
use crate::cli::HookEvent;
use cyberbrain_core::{Error, Slash};
use cyberbrain_policy::Actor;
use serde_json::json;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Instant;

pub mod events;
pub mod paths;
pub mod payload;
pub mod resident;
pub mod session;
#[cfg(test)]
mod tests;

/// SPEC §9.1 / §15: every event but `session-start`.
pub const HOT_PATH_BUDGET_MS: u128 = 15;
/// SPEC §9.1 / §15: `session-start`.
pub const SESSION_START_BUDGET_MS: u128 = 150;

/// The per-session switch. Set to anything but empty, `0`, `false`, `no` or `off` and every
/// hook stands down; `session-start` says so on stdout. An environment variable rather than
/// a config key because "this session" is exactly the scope an environment has, and because
/// the config file is `deny_unknown_fields` and belongs to core.
pub const DISABLE_ENV: &str = "CYBERBRAIN_DISABLED";

/// What the process should do. `exit_code` is always 0; it is a field rather than a
/// constant so the caller's `std::process::exit(out.exit_code)` is visibly the contract
/// and the never-fail test can assert on it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookOutput {
    /// Exactly what goes to stdout. Empty means "nothing to add".
    pub stdout: String,
    /// Diagnostics. On exit 0 the harness keeps stderr in its debug log only, so this is
    /// where every declined action says why (SPEC §14.5) without taxing the context.
    pub stderr: String,
    pub exit_code: i32,
}

impl HookOutput {
    pub fn empty() -> Self {
        Self::default()
    }

    /// One stderr line, prefixed.
    pub fn note(&mut self, line: impl AsRef<str>) {
        self.stderr.push_str("cyberbrain: ");
        self.stderr.push_str(line.as_ref());
        self.stderr.push('\n');
    }

    /// Write both streams. Write errors are ignored on purpose: a closed pipe on the
    /// harness side is not a reason for a non-zero exit.
    pub fn emit(&self) {
        if !self.stdout.is_empty() {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(self.stdout.as_bytes());
            if !self.stdout.ends_with('\n') {
                let _ = out.write_all(b"\n");
            }
            let _ = out.flush();
        }
        if !self.stderr.is_empty() {
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(self.stderr.as_bytes());
            let _ = err.flush();
        }
    }
}

/// The kebab-case name the CLI accepts and the audit log stores (`hook:session-start`).
pub fn event_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "session-start",
        HookEvent::UserPromptSubmit => "user-prompt-submit",
        HookEvent::PreToolUse => "pre-tool-use",
        HookEvent::PostToolUse => "post-tool-use",
        HookEvent::Stop => "stop",
        HookEvent::PreCompact => "pre-compact",
    }
}

/// The harness's own name for the event, as it must appear in `hookSpecificOutput`.
pub fn harness_event_name(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "SessionStart",
        HookEvent::UserPromptSubmit => "UserPromptSubmit",
        HookEvent::PreToolUse => "PreToolUse",
        HookEvent::PostToolUse => "PostToolUse",
        HookEvent::Stop => "Stop",
        HookEvent::PreCompact => "PreCompact",
    }
}

pub fn budget_ms(event: HookEvent) -> u128 {
    match event {
        HookEvent::SessionStart => SESSION_START_BUDGET_MS,
        _ => HOT_PATH_BUDGET_MS,
    }
}

/// `Some(value)` when [`DISABLE_ENV`] switches the tool off for this session.
pub fn disabled_by_env() -> Option<String> {
    let v = std::env::var(DISABLE_ENV).ok()?;
    let t = v.trim();
    if t.is_empty() || ["0", "false", "no", "off"].contains(&t.to_ascii_lowercase().as_str()) {
        None
    } else {
        Some(v)
    }
}

/// Why there is no [`App`]. `session-start` prints this; the other events log it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandDown {
    /// [`DISABLE_ENV`] is set; carries its value.
    Disabled(String),
    /// No store from the working directory upwards (and none named by the environment).
    NoStore(String),
    /// A store is there but `App::open` failed; carries the reason when the caller passed
    /// it, else what could be found out from here.
    Unreadable(String),
}

impl StandDown {
    /// The sentence `session-start` puts on stdout. Says what is not happening, why, and
    /// what would change it, because the agent otherwise cannot tell this from a broken
    /// hook.
    pub fn announce(&self) -> String {
        match self {
            StandDown::Disabled(v) => format!(
                "cyberbrain: standing down for this session: {DISABLE_ENV}={v:?} is set. No \
                 memory is injected and nothing is recorded. Unset it and start a new session \
                 to re-enable."
            ),
            StandDown::NoStore(why) => format!(
                "cyberbrain: standing down: {why}. No memory is injected and nothing is \
                 recorded. `cyberbrain init` in the project root creates a store."
            ),
            StandDown::Unreadable(why) => format!(
                "cyberbrain: standing down: a store exists but could not be opened: {why}. No \
                 memory is injected and nothing is recorded. `cyberbrain status` and \
                 `cyberbrain doctor` say more."
            ),
        }
    }
}

/// Work out why the caller had no `App` to give. Cheap: a few `stat`s up the tree.
fn why_no_app(open_error: Option<&Error>) -> StandDown {
    if let Some(e) = open_error {
        return match e {
            // `discover_store` failing is Error::Config with "no .cyberbrain store found".
            Error::Config(msg) if msg.contains("store found") || msg.contains("is not a") => {
                StandDown::NoStore(msg.clone())
            }
            other => StandDown::Unreadable(other.to_string()),
        };
    }
    let explicit = std::env::var_os("CYBERBRAIN_STORE").map(std::path::PathBuf::from);
    match discover_store(explicit.as_deref()) {
        Err(e) => StandDown::NoStore(e.to_string()),
        Ok(p) => StandDown::Unreadable(format!(
            "{} exists but the caller could not open it (the reason was not passed to the \
             hook; wire `run_with` to see it)",
            Slash(&p)
        )),
    }
}

/// The entry point the CLI dispatches to. `app` is `None` when there is no store or it
/// could not be opened; that case is handled here, not by the caller.
///
/// Never returns a non-zero exit code. Never panics out. Reads nothing the harness did not
/// send, writes nothing the event does not own.
///
/// The CLI is expected to call [`run_with`] so that the reason a store failed to open
/// reaches `session-start`; this is the promised signature for a caller that has no error
/// to pass, and it is allowed to be unused so that the choice is the caller's.
#[allow(dead_code)]
pub fn run(app: Option<&App>, event: HookEvent, stdin: &str) -> HookOutput {
    run_with(app, None, event, stdin)
}

/// As [`run`], with the error `App::open` produced so `session-start` can say *why* it is
/// standing down instead of guessing. Prefer this in the CLI wiring.
pub fn run_with(
    app: Option<&App>,
    open_error: Option<&Error>,
    event: HookEvent,
    stdin: &str,
) -> HookOutput {
    let started = Instant::now();
    let name = event_name(event);

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let stand_down = match (disabled_by_env(), app) {
            (Some(v), _) => Some(StandDown::Disabled(v)),
            (None, Some(_)) => None,
            (None, None) => Some(why_no_app(open_error)),
        };
        events::dispatch(app, stand_down, event, stdin)
    }));

    let mut out = match outcome {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => failed(app, event, stdin, e.to_string()),
        Err(panic) => failed(
            app,
            event,
            stdin,
            format!("panic: {}", panic_message(&panic)),
        ),
    };

    let elapsed = started.elapsed().as_millis();
    if elapsed > budget_ms(event) {
        out.note(format!(
            "hook {name} took {elapsed} ms inside the hook, over its budget of {} ms \
             (SPEC §9.1); the harness was not delayed further by this message",
            budget_ms(event)
        ));
    }
    // The contract. Whatever happened above, the process exits 0.
    out.exit_code = 0;
    out
}

/// SPEC §9.1: the error is recorded to the store when one is reachable, stderr names it
/// (debug log only on exit 0), stdout stays empty, the exit code stays 0.
fn failed(app: Option<&App>, event: HookEvent, stdin: &str, error: String) -> HookOutput {
    let name = event_name(event);
    let mut out = HookOutput::empty();
    let recorded = match app {
        Some(app) => {
            let session = payload::Payload::parse(stdin).session_id;
            app.policy()
                .audit()
                .record_raw(
                    &Actor::Hook(name.into()).to_string(),
                    "hook.error",
                    format!("hook:{name}"),
                    json!({ "error": error, "session": session, "stdin_bytes": stdin.len() }),
                )
                .map(|_| "recorded in the audit log")
                .unwrap_or("could not be recorded either")
        }
        None => "no store to record it in",
    };
    out.note(format!(
        "hook {name} failed internally and stood down: {error} ({recorded}); exit 0, empty \
         output, the harness is unaffected (SPEC §9.1)"
    ));
    out
}

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unprintable panic payload".to_string()
    }
}

/// For the binary, before [`run`]. The release profile builds with `panic = "abort"`, under
/// which `catch_unwind` never sees the panic: the process aborts with a signal and the
/// harness sees a non-zero exit. A panic hook runs *before* the abort, so this one turns
/// the abort into an exit 0 with the panic on stderr. Stdout is untouched because nothing
/// is written to it until [`HookOutput::emit`], which the panic pre-empts.
///
/// Process-global; install it only on the hook code path.
pub fn install_never_fail_guard() {
    std::panic::set_hook(Box::new(|info| {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(
            err,
            "cyberbrain: hook panicked: {info}; exiting 0 with empty output so the harness is \
             unharmed (SPEC §9.1)"
        );
        let _ = err.flush();
        std::process::exit(0);
    }));
}
