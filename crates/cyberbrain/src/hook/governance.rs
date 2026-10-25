//! Asks a governance service (AgentGuard) before a tool call runs (2026-09-25).
//!
//! Cyberbrain's own pre-tool-use checks guard the store. This one guards everything else the
//! agent does: before Bash, Edit, Write, MultiEdit, NotebookEdit or WebFetch runs, the tool
//! and its input go to `POST <url>/v1/tool-calls`, and the answer — allow, deny, ask — is
//! what the harness is told. Which kind of action a call is (a delete, an infrastructure
//! change, a file write) is decided by the service, not here, so the mapping can change
//! without a new binary on every machine.
//!
//! `[governance] mode`:
//! - `shadow`: the service's answer is recorded and never stops anything; an unreachable
//!   service stops nothing either. The service keeps a shadow agent at "allow" on its own;
//!   this is the second lock, so that a misconfigured service cannot block a session that
//!   was meant to be observed. What the service would have decided arrives beside that
//!   allow as `shadow_permission`/`shadow_reason` (AgentGuard from 2026-09-27) and is what
//!   the log line names; a service without the fields is logged as before.
//! - `enforce`: deny and ask are carried out. An unreachable service lets reads through and
//!   asks about everything else — a restart of the service must not stop every session, and
//!   must not wave a delete through either.
//!
//! The request is egress (`EgressPurpose::Governance`): through the gate, loopback or private
//! range only, one audit row per call. The key is never in the store's configuration.

use super::payload::Payload;
use crate::app::App;
use cyberbrain_core::config::{GovernanceConfig, GovernanceMode};
use cyberbrain_core::{EgressPurpose, Error, Result};
use cyberbrain_policy::Actor;
use cyberbrain_policy::egress::{Egress, Outcome};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// The tools whose calls are sent. The pre-tool-use matcher must name at least these.
pub const TOOLS: [&str; 6] = [
    "Bash",
    "Edit",
    "Write",
    "MultiEdit",
    "NotebookEdit",
    "WebFetch",
];

pub const KEY_ENV: &str = "CYBERBRAIN_AGENTGUARD_KEY";

/// What the hook should tell the harness.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Say nothing; the call runs. The string is for stderr.
    Proceed(String),
    /// `deny` or `ask`, with the reason the harness shows.
    Decide(&'static str, String),
}

/// `~/.config/cyberbrain/agentguard.key`, beside the identity file.
fn key_path() -> Option<PathBuf> {
    crate::identity::path().map(|p| p.with_file_name("agentguard.key"))
}

pub(crate) fn key() -> Option<String> {
    std::env::var(KEY_ENV)
        .ok()
        .or_else(|| std::fs::read_to_string(key_path()?).ok())
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
}

pub fn check(app: &App, payload: &Payload, actor: &Actor) -> Verdict {
    decide(
        &app.config().governance,
        app.policy().egress(),
        key(),
        payload,
        actor,
    )
}

/// The decision itself, from a configuration and a gate. `check` hands in the store's;
/// `cyberbrain guard` (the plugin, no store) hands in its own (2026-10-04).
pub fn decide(
    cfg: &GovernanceConfig,
    egress: &Egress,
    key: Option<String>,
    payload: &Payload,
    actor: &Actor,
) -> Verdict {
    let Some(url) = cfg.url.as_deref() else {
        return Verdict::Proceed("governance: not configured".into());
    };
    let Some(tool) = payload.tool_name.as_deref().filter(|t| TOOLS.contains(t)) else {
        return Verdict::Proceed("governance: tool not sent".into());
    };
    let answer = key
        .ok_or_else(|| {
            Error::Config(format!(
                "no key: set {KEY_ENV} or put one in ~/.config/cyberbrain/agentguard.key"
            ))
        })
        .and_then(|k| ask(egress, cfg, url, &k, tool, payload, actor));
    match (answer, cfg.mode) {
        (Ok(a), GovernanceMode::Shadow) => Verdict::Proceed(shadow_line(tool, &a)),
        (Ok(a), GovernanceMode::Enforce) => match a.permission.as_str() {
            "allow" => Verdict::Proceed(format!("governance: allow ({})", a.summary)),
            "deny" => Verdict::Decide("deny", format!("AgentGuard refused this: {}", a.summary)),
            _ => Verdict::Decide(
                "ask",
                format!("AgentGuard wants a person to decide: {}", a.summary),
            ),
        },
        (Err(e), GovernanceMode::Shadow) => {
            Verdict::Proceed(format!("governance (shadow): not reachable, allowed: {e}"))
        }
        (Err(e), GovernanceMode::Enforce) => {
            if reads_only(tool, payload) {
                Verdict::Proceed(format!("governance: not reachable, a read goes ahead: {e}"))
            } else {
                Verdict::Decide(
                    "ask",
                    format!(
                        "AgentGuard is not reachable ({e}); this call changes something, so a \
                         person decides"
                    ),
                )
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Answer {
    permission: String,
    summary: String,
    /// What the service would have decided, when it did not decide it: a shadow agent's
    /// call is always let through (`permission` is `allow`), and since the service reports
    /// the judgement it withheld (`shadow_permission`, `shadow_reason`, 2026-09-27) the log
    /// can say "would be deny" instead of "would be allow". Absent from a service that
    /// predates the fields, and absent when nothing would have been refused.
    shadow: Option<Shadow>,
}

#[derive(Debug, PartialEq, Eq)]
struct Shadow {
    permission: String,
    reason: Option<String>,
}

/// The shadow log line. With the withheld judgement it names that; without it, what the
/// service answered, as before the fields existed.
fn shadow_line(tool: &str, a: &Answer) -> String {
    match &a.shadow {
        Some(Shadow {
            permission,
            reason: Some(reason),
        }) => format!("governance (shadow): {tool} would be {permission} ({reason})"),
        Some(Shadow {
            permission,
            reason: None,
        }) => format!(
            "governance (shadow): {tool} would be {permission} ({})",
            a.summary
        ),
        None => format!(
            "governance (shadow): {tool} would be {} ({})",
            a.permission, a.summary
        ),
    }
}

/// Reads the service's answer. Every field but `permission` is optional; a missing
/// `permission` is read as `ask`, the answer that neither waves through nor refuses.
fn answer_of(v: &Value) -> Answer {
    let permission = v["permission"].as_str().unwrap_or("ask").to_string();
    let mut summary = v["action_type"].as_str().unwrap_or("?").to_string();
    if let Some(gate) = v["deciding_gate"].as_str() {
        summary.push_str(&format!(", gate {gate}"));
    }
    if let Some(reason) = v["reason"].as_str() {
        summary.push_str(&format!(": {reason}"));
    }
    // `shadow_outcome` (block, escalate, hold) is the service's own word for what
    // `shadow_permission` already says in the harness's words, so it is not repeated.
    let shadow = v["shadow_permission"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| Shadow {
            permission: p.to_string(),
            reason: v["shadow_reason"]
                .as_str()
                .filter(|r| !r.is_empty())
                .map(str::to_string),
        });
    Answer {
        permission,
        summary,
        shadow,
    }
}

fn ask(
    egress: &Egress,
    cfg: &GovernanceConfig,
    url: &str,
    key: &str,
    tool: &str,
    payload: &Payload,
    actor: &Actor,
) -> Result<Answer> {
    let endpoint = format!("{}/v1/tool-calls", url.trim_end_matches('/'));
    let ticket = egress.open(actor, EgressPurpose::Governance, &endpoint)?;
    let body = json!({
        "tenant_id": cfg.tenant,
        "agent_id": cfg.agent_id,
        "agent_name": "Claude Code",
        "session_id": payload.session_id,
        "tool": tool,
        "tool_input": payload.tool_input,
    })
    .to_string();
    let bearer = format!("Bearer {key}");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Index(format!("cannot start the async runtime: {e}")))?;
    let bytes_out = body.len() as u64;
    let sent = rt
        .block_on(async {
            tokio::time::timeout(
                Duration::from_millis(cfg.timeout_ms.max(1)),
                cyberbrain_policy::egress::transport::post_json(
                    &ticket,
                    &endpoint,
                    &[("authorization", bearer.as_str())],
                    body,
                    None,
                ),
            )
            .await
        })
        .map_err(|_| Error::Index(format!("no answer within {} ms", cfg.timeout_ms)))
        .and_then(|r| r);
    // The ticket is closed with what happened, or the gate records the call as abandoned
    // (the first day in use left one `egress.abandoned` row beside every call).
    let resp = match sent {
        Ok(resp) => {
            let outcome = if resp.status == 200 {
                Outcome::ok(Some(resp.status))
            } else {
                Outcome::failed(&format!("HTTP {}", resp.status))
            };
            ticket.close(outcome.bytes(bytes_out, resp.body.len() as u64))?;
            resp
        }
        Err(e) => {
            ticket.close(Outcome::failed(&e.to_string()))?;
            return Err(e);
        }
    };
    let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
    if resp.status != 200 {
        let detail = v["detail"].as_str().unwrap_or("no detail");
        return Err(Error::Index(format!("HTTP {}: {detail}", resp.status)));
    }
    Ok(answer_of(&v))
}

/// Programs that only look. A Bash command reads only when every simple command in it runs
/// one of these and nothing is redirected into a file.
/// Not `env`, `xargs`, `sudo`, `time` or `nohup`: they run whatever follows them.
const READERS: [&str; 32] = [
    "ls", "cat", "head", "tail", "less", "grep", "rg", "egrep", "wc", "echo", "printf", "pwd",
    "stat", "file", "du", "df", "ps", "ss", "date", "which", "whoami", "id", "uname", "printenv",
    "sort", "uniq", "cut", "tr", "diff", "jq", "tree", "realpath",
];

/// For an unreachable service in enforce mode: may this call go ahead without asking?
///
/// Fail-closed: a yes needs every part of the command to be understood as looking only. A
/// flag this does not know, on a program that has writing ones, is a no, and so is anything
/// it cannot take apart, which is why command substitution is refused outright rather than
/// split.
fn reads_only(tool: &str, payload: &Payload) -> bool {
    if tool != "Bash" {
        return false; // file tools write; WebFetch leaves the machine
    }
    let Some(cmd) = payload.bash_command() else {
        return false;
    };
    // `` `…` `` and `$(…)` run a second command inside the first. Splitting at `(` caught
    // the second form; nothing caught the backticks, so `echo `rm -rf x`` read as `echo`.
    if cmd.contains('`') || cmd.contains("$(") {
        return false;
    }
    let redirects = cmd.match_indices('>').any(|(i, _)| {
        let rest = cmd[i + 1..].trim_start_matches(['>', '&']).trim_start();
        !(rest.starts_with("/dev/null") || rest.starts_with(|c: char| c.is_ascii_digit()))
    });
    if redirects {
        return false;
    }
    cmd.split(|c: char| ";|&()\n".contains(c))
        .map(|s| s.split_whitespace().collect::<Vec<_>>())
        .filter(|w| !w.is_empty())
        .all(|w| {
            let prog = w[0].rsplit('/').next().unwrap_or(w[0]);
            let args = &w[1..];
            match prog {
                "git" => match args.first().copied() {
                    Some("status" | "rev-parse") => true,
                    // `--output` is a diff option, and `log` and `show` take it too.
                    Some("log" | "diff" | "show") => {
                        !args.iter().any(|a| a.starts_with("--output"))
                    }
                    Some("branch") => git_branch_lists(&args[1..]),
                    _ => false,
                },
                "sed" => sed_prints_only(args),
                // Every action GNU find has that writes or runs something.
                "find" => !args.iter().any(|a| {
                    ["-delete", "-exec", "-ok", "-fprint", "-fls"]
                        .iter()
                        .any(|p| a.starts_with(p))
                }),
                "sort" => sort_reads(args),
                "uniq" => uniq_reads(args),
                "tree" => !args.iter().any(|a| *a == "-o" || a.starts_with("-o")),
                "date" => !args
                    .iter()
                    .any(|a| a.starts_with("-s") || a.starts_with("--set")),
                "rg" => !args.iter().any(|a| a.starts_with("--pre")),
                "cyberbrain" => matches!(
                    args.first().copied(),
                    Some("recall" | "find" | "status" | "doctor")
                ),
                p => READERS.contains(&p),
            }
        })
}

/// `git branch` lists only with no operand, or with `--list`/`-l`, where an operand is a
/// pattern. Otherwise an operand is a branch to create, and `-D`, `-m`, `-c` and the like
/// change branches; anything outside the listing flags is a no.
fn git_branch_lists(args: &[&str]) -> bool {
    let listing = args.iter().any(|a| matches!(*a, "--list" | "-l"));
    args.iter().all(|a| {
        if !a.starts_with('-') {
            return listing;
        }
        matches!(
            *a,
            "-a" | "-r"
                | "-v"
                | "-vv"
                | "-l"
                | "-i"
                | "--list"
                | "--all"
                | "--remotes"
                | "--verbose"
                | "--show-current"
                | "--ignore-case"
                | "--no-color"
                | "--no-column"
        ) || [
            "--color",
            "--column",
            "--sort=",
            "--format=",
            "--contains",
            "--no-contains",
            "--merged",
            "--no-merged",
            "--points-at",
        ]
        .iter()
        .any(|p| a.starts_with(p))
    })
}

/// `sort` reads unless told to write (`-o`, `--output`) or to run a program
/// (`--compress-program`). Short flags are checked letter by letter, so `-uo f` is caught;
/// `k` and `t` take the rest of the word, or the next one, as their argument.
fn sort_reads(args: &[&str]) -> bool {
    const SHORT: &str = "bcCdfghiMnrRsuVz";
    const LONG: [&str; 18] = [
        "--reverse",
        "--numeric-sort",
        "--unique",
        "--key",
        "--field-separator",
        "--human-numeric-sort",
        "--version-sort",
        "--ignore-case",
        "--stable",
        "--general-numeric-sort",
        "--month-sort",
        "--random-sort",
        "--zero-terminated",
        "--check",
        "--dictionary-order",
        "--ignore-leading-blanks",
        "--ignore-nonprinting",
        "--help",
    ];
    let mut takes_next = false;
    for a in args {
        if takes_next {
            takes_next = false;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            if !LONG.contains(&format!("--{name}").as_str()) {
                return false;
            }
            takes_next = matches!(name, "key" | "field-separator") && !long.contains('=');
            continue;
        }
        let Some(short) = a.strip_prefix('-').filter(|s| !s.is_empty()) else {
            continue; // an input file
        };
        for (i, c) in short.char_indices() {
            if c == 'k' || c == 't' {
                takes_next = i + 1 == short.len();
                break;
            }
            if !SHORT.contains(c) {
                return false;
            }
        }
    }
    true
}

/// `uniq [OPTION]... [INPUT [OUTPUT]]`: a second operand is a file it writes.
fn uniq_reads(args: &[&str]) -> bool {
    const SHORT: &str = "cdDiuz";
    const LONG: [&str; 11] = [
        "--count",
        "--repeated",
        "--all-repeated",
        "--ignore-case",
        "--unique",
        "--zero-terminated",
        "--skip-fields",
        "--skip-chars",
        "--check-chars",
        "--group",
        "--help",
    ];
    let mut operands = 0;
    let mut takes_next = false;
    for a in args {
        if takes_next {
            takes_next = false;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let name = long.split('=').next().unwrap_or(long);
            if !LONG.contains(&format!("--{name}").as_str()) {
                return false;
            }
            takes_next =
                matches!(name, "skip-fields" | "skip-chars" | "check-chars") && !long.contains('=');
            continue;
        }
        match a.strip_prefix('-').filter(|s| !s.is_empty()) {
            None => operands += 1,
            Some(short) => {
                for (i, c) in short.char_indices() {
                    if matches!(c, 'f' | 's' | 'w') {
                        takes_next = i + 1 == short.len();
                        break;
                    }
                    if !SHORT.contains(c) {
                        return false;
                    }
                }
            }
        }
    }
    operands <= 1
}

/// `sed` reads only with `-n` and a script made of print commands. A script can write
/// (`w`, `W`, the `w` flag of `s`) or run a shell (`e`), and telling those apart in general
/// means parsing sed, so anything but an address followed by `p`, `P`, `=`, `l`, `q` or `Q`
/// is a no. So are `-i` in any spelling, `-f` (a script this cannot see) and a script split
/// over several words.
fn sed_prints_only(args: &[&str]) -> bool {
    let mut quiet = false;
    let mut scripts = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        match a {
            "-n" | "--quiet" | "--silent" => quiet = true,
            "-E" | "-r" | "-s" | "-u" | "-z" | "--posix" | "--regexp-extended" => {}
            "-e" => {
                let Some(s) = args.get(i + 1) else {
                    return false;
                };
                scripts.push(*s);
                i += 1;
            }
            _ if a.starts_with('-') => return false,
            _ => {
                // The first operand is the script unless `-e` gave one; the rest are files.
                if scripts.is_empty() {
                    scripts.push(a);
                }
            }
        }
        i += 1;
    }
    quiet && !scripts.is_empty() && scripts.iter().all(|s| sed_script_prints(s))
}

/// One sed script word: quoted whole or not at all, and every command in it an address and
/// a print command.
fn sed_script_prints(word: &str) -> bool {
    let script = match word.chars().next() {
        Some(q @ ('\'' | '"')) => {
            if word.len() < 2 || !word.ends_with(q) {
                return false;
            }
            &word[1..word.len() - 1]
        }
        _ => word,
    };
    if script.contains(['\'', '"']) {
        return false;
    }
    script.split([';', '\n']).all(|cmd| {
        let rest = skip_sed_address(cmd.trim());
        matches!(rest.trim(), "p" | "P" | "=" | "l" | "q" | "Q")
    })
}

/// What follows the address of a sed command: line numbers, `$`, `,`, `~`, `+`, `!` and
/// `/regex/`. A regex that does not close leaves nothing, which no print command matches.
fn skip_sed_address(cmd: &str) -> &str {
    let mut rest = cmd;
    loop {
        let Some(c) = rest.chars().next() else {
            return rest;
        };
        if c.is_ascii_digit() || matches!(c, '$' | ',' | '~' | '+' | '!' | ' ') {
            rest = &rest[c.len_utf8()..];
        } else if c == '/' {
            let body = &rest[1..];
            let mut escaped = false;
            let mut end = None;
            for (i, ch) in body.char_indices() {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '/' {
                    end = Some(i);
                    break;
                }
            }
            match end {
                Some(i) => rest = &body[i + 1..],
                None => return "",
            }
        } else {
            return rest;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(cmd: &str) -> Payload {
        Payload::parse(&json!({"tool_name": "Bash", "tool_input": {"command": cmd}}).to_string())
    }

    #[test]
    fn only_looking_counts_as_a_read() {
        for cmd in [
            "ls -la",
            "cat a | grep b | wc -l",
            "git status && git diff",
            "find . -name '*.rs'",
            "sed -n 1,20p x",
            "grep x y 2>/dev/null",
            "cyberbrain recall wolfpack",
        ] {
            assert!(reads_only("Bash", &bash(cmd)), "{cmd}");
        }
        for cmd in [
            "rm -rf x",
            "echo x > f",
            "cat a >> b",
            "git push",
            "sed -i s/a/b/ f",
            "find . -delete",
            "curl https://x",
            "ls; docker compose down",
            "cyberbrain write --ring 2 --name x --body y",
            "env rm -rf x",
            "xargs rm < list",
        ] {
            assert!(!reads_only("Bash", &bash(cmd)), "{cmd}");
        }
        let write = Payload::parse(r#"{"tool_name":"Write","tool_input":{"file_path":"/x"}}"#);
        assert!(!reads_only("Write", &write));
    }

    /// Commands that look like reading and write anyway. Each of these went through while
    /// AgentGuard was unreachable; the fallback has to say no when it cannot tell.
    #[test]
    fn writing_through_a_reader_is_not_a_read() {
        for cmd in [
            "echo `rm -rf x`",
            "ls `touch f`",
            "echo $(rm -rf x)",
            "sort -o f a",
            "sort -uo f a",
            "sort --output=f a",
            "sort --compress-program=sh a",
            "uniq in out",
            "uniq -c in out",
            "find . -fprint f",
            "find . -fprintf f %p",
            "find . -fls f",
            "find . -ok rm {} ;",
            "git branch -D x",
            "git branch new-branch",
            "git branch -m a b",
            "git diff --output=f",
            "git log -p --output=f",
            "sed -n 'w f' x",
            "sed -n '1w f' x",
            "sed -n 's/a/b/w f' x",
            "sed -n '1 w f' x",
            "sed -n -f script x",
            "sed -ni 1p x",
            "tree -o f",
            "date -s 2000-01-01",
            "rg --pre sh x",
        ] {
            assert!(!reads_only("Bash", &bash(cmd)), "{cmd}");
        }
        for cmd in [
            "sort a | uniq -c",
            "sort -k2,2 -t, -rn a",
            "uniq -c a",
            "git branch",
            "git branch -a -v",
            "git branch --list 'feat*'",
            "git branch --show-current",
            "sed -n '1,20p' x",
            "sed -n '/^fn/,/^}/p' x",
            "sed -n -e 1p -e '$p' x",
            "find . -name '*.rs' -print",
        ] {
            assert!(reads_only("Bash", &bash(cmd)), "{cmd}");
        }
    }

    /// An AgentGuard from before 27.09. sends no `shadow_*` fields: the line is what it
    /// always was.
    #[test]
    fn an_answer_without_shadow_fields_is_logged_as_before() {
        let a =
            answer_of(&json!({"permission": "allow", "action_type": "shell", "mode": "shadow"}));
        assert_eq!(a.shadow, None);
        assert_eq!(
            shadow_line("Bash", &a),
            "governance (shadow): Bash would be allow (shell)"
        );
    }

    /// Calibrated against the state before: the line said "would be allow" for a call the
    /// service would have refused, because a shadow agent's `permission` is always allow.
    #[test]
    fn an_answer_with_shadow_fields_says_what_would_have_happened() {
        let a = answer_of(&json!({
            "permission": "allow", "action_type": "file_delete", "mode": "shadow",
            "shadow_permission": "deny", "shadow_outcome": "block",
            "shadow_reason": "scope_nicht_mandatiert",
        }));
        assert_eq!(
            a.permission, "allow",
            "what the harness is told does not change"
        );
        assert_eq!(
            shadow_line("Bash", &a),
            "governance (shadow): Bash would be deny (scope_nicht_mandatiert)"
        );
        // Without a reason, the summary stands in for it.
        let a = answer_of(&json!({
            "permission": "allow", "action_type": "file_write", "shadow_permission": "ask",
        }));
        assert_eq!(
            shadow_line("Write", &a),
            "governance (shadow): Write would be ask (file_write)"
        );
    }
}
