---
layout: ../layouts/Doc.astro
title: "Claude Code: hooks, MCP, guard"
description: "Connect Cyberbrain to Claude Code: cyberbrain install, the six hook events, the store guard before every tool call, AgentGuard in shadow or enforce mode, and the MCP server for Claude Desktop and others."
slug: "claude-code/"
---

# Claude Code: hooks, MCP, guard

Cyberbrain reaches an agent two ways. The **hooks** run at fixed points of a Claude Code session: they put rings 0 and 1 into the session and check each tool call before it runs. The **MCP server** lets an agent search and write while it works, and it is the way in for clients that have no hooks, such as Claude Desktop.

## cyberbrain install

```console
$ cyberbrain install --client claude-code
Setting up /usr/local/bin/cyberbrain
  store: /home/you/project/.cyberbrain

claude-code
  added           /home/you/project/.claude/settings.json
                  the hooks are per project, so this is the project you named

Restart the client for it to read this. `cyberbrain install --undo` takes it back out.
```

It writes one entry per event into the project's `.claude/settings.json`. Each entry carries `"_managedBy": "cyberbrain"` and names the absolute path of the binary that ran `install`, with a 30-second timeout:

| Event | Command | Matcher |
|---|---|---|
| `SessionStart` | `hook session-rings`, then `hook session-context` | all |
| `UserPromptSubmit` | `hook user-prompt-submit` | all |
| `PreToolUse` | `hook pre-tool-use` | `Edit\|Write\|MultiEdit\|NotebookEdit\|Bash\|WebFetch` |
| `PostToolUse` | `hook post-tool-use` | `Edit\|Write\|MultiEdit\|NotebookEdit` |
| `Stop` | `hook stop` | all |
| `PreCompact` | `hook pre-compact` | all |

Everything in the file that is not ours is copied through untouched, and the previous file is kept beside the new one. Since 0.8.3 the file is replaced whole, so an interrupted write cannot leave a truncated settings file; a settings file that is a link stays a link. Move or reinstall the binary and run `install` again. `--dry-run` says what would change.

Without `--client`, `install` sets up every client it finds on this machine: Claude Code for the project, an MCP entry for Claude Desktop. For Codex CLI it does not edit your hand-written `config.toml` but prints the lines to paste.

## The hooks

A hook never fails the harness. Any internal error is recorded and the process exits 0 with empty output. The budget in the specification is a p99 of 15 ms for every event except session start (150 ms); the README of 0.8.4 reports 7 to 9 ms, and 17 ms for session start, on a store of 1,300 notes, process start included.

**Session start** prints rings 0 and 1, each block with its citation, followed by the newest handoff note, a short digest of the store and how to use it. Claude Code shows each hook's output in full only up to 10,000 characters, so since 0.7.10 this is split over two hooks: `session-rings` first and alone, then `session-context`. Installs from before 0.7.10 call `session-start`, which does both in one.

The **handoff** is the newest note in ring 2 or 3 whose name contains `handoff` (configurable in `[handoff]`), at most 7 days old. It is shown as what it is: dated, and outranked by rings 0 and 1. A session can then find what the previous one left for it without knowing to ask.

**User prompt submit** compares rings 0 and 1 with what was injected. If the operator changed them during the session, the new text is injected with the next prompt. If no session start ever ran, it injects them now.

**Post tool use** notices a raw edit of a store file and tells the model what follows: the index answers from the old text until `cyberbrain scan`, a broken `cyberbrain.toml` stops every command.

**Stop** and **pre compact** count turns and compactions. Pre compact cannot inject anything, because Claude Code gives that event no channel to the model; what carries knowledge across a compaction is the session start that follows it.

`CYBERBRAIN_DISABLED` makes the hooks stand down for a session, and session start says so, so silence is never mistaken for a broken hook.

## The store guard

Before each `Edit`, `Write`, `MultiEdit`, `NotebookEdit` and `Bash` call, the pre-tool-use hook checks whether it touches the store. These are real answers of 0.8.4:

| Call | Answer |
|---|---|
| `Edit .cyberbrain/notes/r0/a.md` | **deny**: a ring 0 note, operator invariants |
| `Write .cyberbrain/notes/r2/a.md` | **ask**: a raw edit skips the PII check, the audit row and the reindex; prefer `cyberbrain write` |
| `echo x >> .cyberbrain/cyberbrain.toml` | **ask**: the configuration holds consent, egress and governance settings, and a line that does not parse stops every command |
| `rm .cyberbrain/keys/audit.ed25519` | **deny**: the signing key is evidence |
| `rm .cyberbrain/audit.db` | **deny**: the audit log is append-only evidence |
| `cat .cyberbrain/notes/r2/….md` | allowed: reading is fine |
| a command that drops `CLAUDECODE` around a `cyberbrain` call | **deny**: it would be recorded as the operator |

Every refusal is an audit row. Since 0.8.3 the guard also catches the usual ways around it: path spellings such as `notes/./r0` or globs like `n*/r0`, a `cd` into the store first, `git checkout|restore|rm|mv`, `find -delete`, `xargs rm`, inline code of python, perl, node, ruby and php, `sh -c` scripts, and links to the store. On macOS it ignores case, as the file system does.

When a store cannot be opened, for example because `cyberbrain.toml` does not parse, every call that changes something is put to a person, and reading goes on. Before 0.8.3 such a store left the hook without an answer, which Claude Code reads as "allow".

It is still a heuristic over the command text, not a sandbox.

### Decoys

If the store has [planted decoys](../compliance/#decoys), the same hook refuses any tool call that uses one outside its own note, before AgentGuard is asked, and records it as `canary.hit`.

## AgentGuard: shadow and enforce

With a `[governance]` section, the pre-tool-use hook also sends each Bash, Edit, Write, MultiEdit, NotebookEdit and WebFetch call to an AgentGuard service and carries out its answer (allow, deny, ask). The store's own guard decides first.

```toml
[governance]
url = "http://127.0.0.1:<port>"  # the AgentGuard service; loopback or private range only
tenant = "…"
agent_id = "claude-code"
mode = "shadow"                 # or "enforce"
timeout_ms = 1000
```

The key comes from `CYBERBRAIN_AGENTGUARD_KEY` or `~/.config/cyberbrain/agentguard.key`, never from the store.

- `shadow` records the answer and stops nothing, not even when the service is unreachable. The log line says what AgentGuard would have decided.
- `enforce` carries out deny and ask. With the service unreachable, plain reads go through and everything else is asked about.

Since 0.8.2, in an `enforce` project `cyberbrain install` writes `"onFailure": "block"` on the pre-tool-use hook (Claude Code 2.1.295 and later): a hook that cannot start, times out or prints something unparsable blocks the call instead of letting it through. Run `install` again in an existing enforce project to get the field. Shadow mode and the other hooks keep the default, so a missing binary there does not stop every edit.

## The MCP server

`cyberbrain mcp` speaks the Model Context Protocol over stdio. It offers five tools:

| Tool | Arguments | |
|---|---|---|
| `recall` | `query`, `n`, `ring`, `bereich`, `stand` | hybrid search with citations and caveats |
| `recall_id` | `citation` | the cited block and its whole note |
| `find` | `symbol`, `limit` | line ranges from the code index |
| `write` | `ring`, `kind`, `name`, `body`, `tags`, `bereich`, `retention`, `supersedes`, `valid_from`, `invalid_at`, `force`, `choice`, `expected_updated`, `dry_run` | write a note; answers `written`, `held` or `conflict` |
| `status` | | the same report as `cyberbrain status --json` |

A held write (PII found) or a refusal comes back as a tool result with `isError` set and the findings attached, so the agent can act on it. Caveats travel in `structuredContent.caveats` and in the text. The `write` tool cannot reach rings 0 and 1.

For Claude Code in a project:

```console
$ claude mcp add cyberbrain -- cyberbrain mcp
```

In a project with the hooks installed this is optional: the hooks already give the agent the rings, and it can run `cyberbrain recall` through Bash.

### Claude Desktop and other clients without hooks

`cyberbrain install --client claude-desktop` writes an MCP entry that starts `cyberbrain mcp --store <path>` into every Claude Desktop installation it finds. A desktop client has no project, so a second store on the same machine needs a second entry: `--name`.

Such an entry has no `--client`, and the audit log records it as `mcp`. That is deliberate: **recall over MCP without a client name returns rings 0 and 1 like any other ring.** An agent in Claude Code gets those rings at session start and recall leaves them out to avoid a second copy. Claude Desktop has no session-start hook, so without them in recall it would never see the operator's rules. If you name the client (`--client <name>`), it is recorded as `agent:mcp:<name>` and can be limited by `[provenance] untrusted_clients` and `[clients.bereiche]`.

### MCP over HTTP

For a client that cannot start a process on this machine, such as n8n in a container:

```console
$ cyberbrain mcp-client add n8n --expires P90D
$ cyberbrain mcp --http 172.17.0.1:7790
```

`mcp-client add` prints the token once and keeps only its hash, in your configuration directory, never in the store. Each request needs `Authorization: Bearer <token>` and runs as `agent:mcp:<name>`. The address must be loopback or private, `0.0.0.0` is refused, there is no TLS, and a request carrying an `Origin` header is refused, so browsers cannot reach it. An expired token is refused with a 401 naming the client, and the refusal is an audit row.
