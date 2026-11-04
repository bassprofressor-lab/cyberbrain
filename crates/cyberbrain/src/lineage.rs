//! Who started this process: an agent further up the tree is an agent here (audit
//! 2026-11-04, F1).
//!
//! The CLI tells an agent from the operator by the agent marker in its environment
//! (`CLAUDECODE`, `CYBERBRAIN_AGENT`, see `agent_from_env` in main.rs). An agent controls
//! its own environment, and `env -i bash -c 'cyber''brain write --ring 0 …'` or a Python
//! one-liner that pops the variable before `os.system` shed it past the pre-tool-use text
//! guard: the call was recorded as the operator, and a ring 0 note written that way is
//! attested and injected into every later session.
//!
//! What cannot be shed from inside is the past. `/proc/<pid>/environ` holds the environment
//! a process was started with and does not change afterwards, so the shell Claude Code
//! started for the command still shows `CLAUDECODE=1` whatever its children dropped. And
//! the Claude Code process itself, whose own environment does not carry the marker, is named
//! `claude`: a command that replaced the session's shell with `exec` still has it as parent.
//! So the walk goes from the parent up to PID 1, and the first ancestor that is an agent
//! makes this process one.
//!
//! What it does not cover, said plainly:
//!
//! - **A process that left the tree.** `setsid -f`, a double fork, `nohup … &` whose parent
//!   exited, `systemd-run`, `at`, cron: the process is re-parented to PID 1 or a subreaper and
//!   the chain to the session is gone. The pre-tool-use text guard still stands in front.
//! - **Something already running outside the session** that the agent asks to act: a
//!   `cyberbrain serve` or daemon the operator started, the operator's tmux, a systemd
//!   service. Those are the operator's processes, and what they start is the operator's.
//!   `serve` refuses resident writes for that reason; the daemon judges the process at the
//!   other end of its socket by the same walk, which covers a client inside the session.
//! - **Platforms without `/proc`** (macOS, Windows): the environment alone decides, as before.
//!
//! Cost: a few small reads per ancestor, once per CLI command. The hooks do not run it (they
//! are `Actor::Hook`), and nothing is added to any context an agent reads.

use cyberbrain_policy::Actor;

/// Debug builds only: skip the walk. The test harnesses (`tests/cli.rs`, `ui/e2e`) run the
/// binary as the operator from wherever they are started, which is often inside an agent
/// session. A release build ignores the variable, so it is no switch an agent can flip on the
/// binary that is installed.
pub const TEST_SWITCH: &str = "CYBERBRAIN_TEST_IGNORE_LINEAGE";

/// How far up to look. Real trees are a handful deep; this only bounds a loop.
const MAX_DEPTH: usize = 128;

/// The program name Claude Code runs under (`/proc/<pid>/comm`).
const CLAUDE_CODE_COMM: &str = "claude";

fn switched_off() -> bool {
    cfg!(debug_assertions) && std::env::var_os(TEST_SWITCH).is_some_and(|v| !v.is_empty())
}

/// The agent among the ancestors of this process, not counting itself.
pub fn agent_above_self() -> Option<Actor> {
    if switched_off() {
        return None;
    }
    let me = std::process::id();
    let parent = proc::parent(me)?;
    walk(parent, &proc::environ, &proc::comm, &proc::parent)
}

/// The agent that `pid` is or descends from.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn agent_at_or_above(pid: u32) -> Option<Actor> {
    if switched_off() {
        return None;
    }
    walk(pid, &proc::environ, &proc::comm, &proc::parent)
}

/// The walk, over readers that tests can replace. An ancestor whose files cannot be read
/// (another account's, or gone) is passed over, not taken as the end of the line.
pub(crate) fn walk(
    start: u32,
    environ: &dyn Fn(u32) -> Option<Vec<u8>>,
    comm: &dyn Fn(u32) -> Option<String>,
    parent: &dyn Fn(u32) -> Option<u32>,
) -> Option<Actor> {
    let mut pid = start;
    for _ in 0..MAX_DEPTH {
        if pid <= 1 {
            return None;
        }
        if let Some(a) = environ(pid).and_then(|raw| agent_in_environ(&raw)) {
            return Some(a);
        }
        if comm(pid).is_some_and(|c| c.trim_end() == CLAUDE_CODE_COMM) {
            return Some(Actor::Agent("claude-code".to_string()));
        }
        pid = parent(pid)?;
    }
    None
}

/// The agent a NUL-separated environment block names, as `agent_from_env` reads it.
pub(crate) fn agent_in_environ(raw: &[u8]) -> Option<Actor> {
    let vars: std::collections::HashMap<String, String> = raw
        .split(|b| *b == 0)
        .filter_map(|kv| {
            let kv = String::from_utf8_lossy(kv);
            kv.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect();
    crate::agent_from_env(|k| vars.get(k).cloned())
}

/// The parent pid from the text of `/proc/<pid>/stat`. The command name is in parentheses
/// and may itself contain spaces and parentheses, so the fields are counted from the last `)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parent_in_stat(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_os = "linux")]
mod proc {
    pub fn environ(pid: u32) -> Option<Vec<u8>> {
        std::fs::read(format!("/proc/{pid}/environ")).ok()
    }
    pub fn comm(pid: u32) -> Option<String> {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()
    }
    pub fn parent(pid: u32) -> Option<u32> {
        super::parent_in_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }
}

#[cfg(not(target_os = "linux"))]
mod proc {
    pub fn environ(_pid: u32) -> Option<Vec<u8>> {
        None
    }
    pub fn comm(_pid: u32) -> Option<String> {
        None
    }
    pub fn parent(_pid: u32) -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Tree {
        env: HashMap<u32, Vec<u8>>,
        comm: HashMap<u32, String>,
        parent: HashMap<u32, u32>,
    }

    impl Tree {
        fn run(&self, start: u32) -> Option<String> {
            walk(
                start,
                &|p| self.env.get(&p).cloned(),
                &|p| self.comm.get(&p).cloned(),
                &|p| self.parent.get(&p).copied(),
            )
            .map(|a| a.to_string())
        }
    }

    /// sshd(10) → bash(20) → claude(30) → bash(40, CLAUDECODE=1) → env -i python(50) →
    /// cyberbrain(60). The shell's environment and Claude Code's name both count.
    fn session() -> Tree {
        Tree {
            env: HashMap::from([
                (10, b"PATH=/usr/bin\0".to_vec()),
                (20, b"PATH=/usr/bin\0HOME=/root\0".to_vec()),
                (30, b"PATH=/usr/bin\0".to_vec()),
                (
                    40,
                    b"PATH=/usr/bin\0CLAUDECODE=1\0CLAUDE_CODE_SESSION_ID=8c734a31-x\0".to_vec(),
                ),
                (50, Vec::new()),
                (60, Vec::new()),
            ]),
            comm: HashMap::from([
                (10, "sshd\n".into()),
                (20, "bash\n".into()),
                (30, "claude\n".into()),
                (40, "bash\n".into()),
                (50, "python3\n".into()),
                (60, "cyberbrain\n".into()),
            ]),
            parent: HashMap::from([(60, 50), (50, 40), (40, 30), (30, 20), (20, 10), (10, 1)]),
        }
    }

    #[test]
    fn a_marker_shed_below_the_sessions_shell_is_still_found() {
        assert_eq!(
            session().run(50).as_deref(),
            Some("agent:claude-code:8c734a31")
        );
    }

    #[test]
    fn a_shell_replaced_with_exec_still_has_claude_code_as_parent() {
        let mut t = session();
        t.parent.insert(50, 30);
        assert_eq!(t.run(50).as_deref(), Some("agent:claude-code"));
    }

    #[test]
    fn a_terminal_outside_any_session_is_the_operator() {
        assert_eq!(session().run(20), None);
        // An ancestor nobody may read is passed over, not the end of the walk.
        let mut t = session();
        t.env.remove(&40);
        t.comm.remove(&40);
        assert_eq!(t.run(50).as_deref(), Some("agent:claude-code"));
    }

    #[test]
    fn a_loop_or_a_missing_parent_ends_the_walk() {
        let t = Tree {
            env: HashMap::new(),
            comm: HashMap::new(),
            parent: HashMap::from([(5, 6), (6, 5)]),
        };
        assert_eq!(t.run(5), None);
        assert_eq!(t.run(99), None);
    }

    #[test]
    fn the_parent_is_read_past_a_command_name_with_spaces_and_parentheses() {
        assert_eq!(parent_in_stat("123 (a) b) c) S 456 123 123 0"), Some(456));
        assert_eq!(parent_in_stat("1 (systemd) S 0 1 1"), Some(0));
        assert_eq!(parent_in_stat("garbage"), None);
    }

    /// This very test process, on Linux: whatever its ancestors say, the walk answers
    /// without error, and from inside Claude Code it answers "agent".
    #[cfg(target_os = "linux")]
    #[test]
    fn the_real_tree_is_readable() {
        let found = agent_at_or_above(std::process::id());
        if std::env::var_os("CLAUDECODE").is_some() && std::env::var_os(TEST_SWITCH).is_none() {
            assert!(found.is_some());
        }
    }
}
