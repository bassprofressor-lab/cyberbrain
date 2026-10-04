//! `cyberbrain guard`: the AgentGuard check without a memory store (2026-10-04).
//!
//! The pre-tool-use hook has asked AgentGuard about every Bash, Edit, Write and WebFetch
//! call since 2026-09-25, but only inside a project with a Cyberbrain store, because the
//! configuration and the audited network gate live in the store. The AgentGuard plugin for
//! Claude Code is for teams that want the guard and not the memory, so this entry point
//! takes its configuration from the environment instead:
//!
//! | what       | plugin option (Claude Code exports it)  | otherwise          | default       |
//! |------------|-----------------------------------------|--------------------|---------------|
//! | service    | `CLAUDE_PLUGIN_OPTION_URL`              | `AGENTGUARD_URL`   | none: no call |
//! | key        | `CLAUDE_PLUGIN_OPTION_KEY`              | `CYBERBRAIN_AGENTGUARD_KEY`, then `~/.config/cyberbrain/agentguard.key` | |
//! | mode       | `CLAUDE_PLUGIN_OPTION_MODE`             | `AGENTGUARD_MODE`  | `shadow`      |
//! | tenant     | `CLAUDE_PLUGIN_OPTION_TENANT`           | `AGENTGUARD_TENANT`| empty         |
//! | agent id   | `CLAUDE_PLUGIN_OPTION_AGENT_ID`         | `AGENTGUARD_AGENT_ID` | `claude-code` |
//!
//! Everything else is the store's hook unchanged: the same decision (`governance::decide`),
//! the same rule that the service must be loopback or private-range (tool calls carry file
//! contents and commands; there is no switch for a public address), and in `enforce` the
//! same answer when the service is down: reads go ahead, everything else asks a person.
//! The network gate audits into memory, because there is no store to audit into; the
//! service keeps its own signed record of every call it was asked about.

use super::HookOutput;
use super::governance::{self, Verdict};
use super::payload::Payload;
use cyberbrain_core::config::{GovernanceConfig, GovernanceMode};
use cyberbrain_policy::{Actor, AuditLog, Egress, PolicyConfig};
use serde_json::json;

fn opt(plugin: &str, fallback: &str) -> Option<String> {
    std::env::var(format!("CLAUDE_PLUGIN_OPTION_{plugin}"))
        .ok()
        .or_else(|| std::env::var(fallback).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// The configuration the environment describes. `None` url means nothing is sent.
pub fn config_from_env() -> GovernanceConfig {
    let mut cfg = GovernanceConfig {
        url: opt("URL", "AGENTGUARD_URL"),
        ..GovernanceConfig::default()
    };
    if let Some(t) = opt("TENANT", "AGENTGUARD_TENANT") {
        cfg.tenant = t;
    }
    if let Some(a) = opt("AGENT_ID", "AGENTGUARD_AGENT_ID") {
        cfg.agent_id = a;
    }
    cfg.mode = match opt("MODE", "AGENTGUARD_MODE").as_deref() {
        Some("enforce") => GovernanceMode::Enforce,
        _ => GovernanceMode::Shadow,
    };
    cfg
}

/// Run the guard for one pre-tool-use payload. Never fails the harness: whatever happens,
/// the answer is either a decision on stdout or nothing.
pub fn run(stdin: &str) -> HookOutput {
    let mut out = HookOutput::empty();
    let payload = Payload::parse(stdin);
    for n in &payload.notes {
        out.note(n);
    }
    let cfg = config_from_env();
    if cfg.url.is_none() {
        out.note("guard: no AgentGuard address configured (plugin option `url`); nothing sent");
        return out;
    }
    let key = opt("KEY", governance::KEY_ENV).or_else(governance::key);
    let policy = PolicyConfig {
        governance_endpoint: cfg.url.clone(),
        ..PolicyConfig::default()
    };
    let actor = Actor::Hook("guard".into());
    let (audit, _sink) = AuditLog::in_memory();
    let egress = Egress::new(policy, audit, actor.clone());
    match governance::decide(&cfg, &egress, key, &payload, &actor) {
        Verdict::Proceed(note) => out.note(format!("guard: {note}")),
        Verdict::Decide(decision, reason) => {
            out.note(format!("guard: {decision}"));
            out.stdout = json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": decision,
                    "permissionDecisionReason": reason,
                }
            })
            .to_string();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// One HTTP answer from a local listener, so the guard talks to something real.
    fn serve_once(body: &'static str) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut buf = [0u8; 8192];
                let _ = s.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    fn payload(cmd: &str) -> String {
        json!({"session_id":"s","tool_name":"Bash","tool_input":{"command":cmd}}).to_string()
    }

    // The environment is process-wide; these run one after another inside one test.
    #[test]
    fn guard_follows_the_service_in_enforce_and_fails_safe_when_it_is_gone() {
        // SAFETY: single test touches these variables; no other test in this module reads them.
        unsafe {
            std::env::set_var("CLAUDE_PLUGIN_OPTION_KEY", "agk_test");
            std::env::set_var("CLAUDE_PLUGIN_OPTION_MODE", "enforce");
        }

        // 1. Not configured: nothing is sent, nothing decided.
        unsafe { std::env::remove_var("CLAUDE_PLUGIN_OPTION_URL") };
        let out = run(&payload("rm -rf build"));
        assert!(out.stdout.is_empty(), "unconfigured guard must not decide");

        // 2. The service says deny: the harness is told deny.
        let url = serve_once(
            r#"{"permission":"deny","action_type":"file_delete","reason":"not in scope"}"#,
        );
        unsafe { std::env::set_var("CLAUDE_PLUGIN_OPTION_URL", &url) };
        let out = run(&payload("rm -rf build"));
        assert!(
            out.stdout.contains(r#""permissionDecision":"deny""#),
            "{}",
            out.stdout
        );

        // 3. The service is gone: a write asks a person, a read goes ahead.
        unsafe { std::env::set_var("CLAUDE_PLUGIN_OPTION_URL", "http://127.0.0.1:9") };
        let out = run(&payload("rm -rf build"));
        assert!(
            out.stdout.contains(r#""permissionDecision":"ask""#),
            "{}",
            out.stdout
        );
        let out = run(&payload("ls -la"));
        assert!(out.stdout.is_empty(), "a read goes ahead: {}", out.stdout);

        // 4. A public address is refused by the gate, and in enforce that means: ask.
        unsafe { std::env::set_var("CLAUDE_PLUGIN_OPTION_URL", "https://example.com") };
        let out = run(&payload("rm -rf build"));
        assert!(
            out.stdout.contains(r#""permissionDecision":"ask""#),
            "{}",
            out.stdout
        );

        // 5. Shadow never stops anything, even when the service would deny.
        let url = serve_once(r#"{"permission":"deny","action_type":"file_delete"}"#);
        unsafe {
            std::env::set_var("CLAUDE_PLUGIN_OPTION_URL", &url);
            std::env::set_var("CLAUDE_PLUGIN_OPTION_MODE", "shadow");
        }
        let out = run(&payload("rm -rf build"));
        assert!(
            out.stdout.is_empty(),
            "shadow must not decide: {}",
            out.stdout
        );

        unsafe {
            for k in ["URL", "KEY", "MODE"] {
                std::env::remove_var(format!("CLAUDE_PLUGIN_OPTION_{k}"));
            }
        }
    }
}
