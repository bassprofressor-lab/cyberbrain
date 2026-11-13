// cyberbrain-compact: a Claude Code mod (function hooks), written by
// `cyberbrain install --client claude-code --compact-mod`.
//
// On `session.compact` it hands the conversation to the local cyberbrain binary
// (`cyberbrain hook pre-compact-snapshot`, over stdin, nothing over the network),
// which keeps a bounded snapshot in the project's store (ring 3, PII gate and
// audit chain as for any write) and answers with instructions for the summary:
// which citations, open ring-1 points and snapshot to keep. The mod adds those
// to the summarizer's instructions and always calls `next`: whatever happens on
// cyberbrain's side, the compaction goes ahead as it would have without the mod.
//
// What leaves the session: each message's role, its text (cut to TEXT_LIMIT
// characters) and the names of the tools it called. No tool inputs, no tool
// results.

import type { Hook, Register, SessionMessage } from 'claude-code'

// The installer writes the absolute path of the binary that installed it here.
const BINARY = 'cyberbrain'
const TEXT_LIMIT = 20000
const TIMEOUT_MS = 20000

/** The hook itself, exported so a test can hand it a conversation. */
export const steer: Hook<'session.compact'> = async ($, e, next) => {
  // A subagent's own transcript is not the session the store keeps.
  if (e.agentId !== undefined) return next(e)
  // Always a list from the engine; the test kit's own compaction carries none.
  const messages: readonly SessionMessage[] = Array.isArray(e.messages) ? e.messages : []
  let extra = ''
  try {
    const payload = JSON.stringify({
      hook_event_name: 'PreCompact',
      session_id: await $.session.id(),
      cwd: await $.session.cwd(),
      trigger: e.trigger,
      messages: messages.map(m => ({
        role: m.role,
        text: m.text.length > TEXT_LIMIT ? m.text.slice(0, TEXT_LIMIT) : m.text,
        tools: m.toolUses.map(t => t.tool),
      })),
    })
    const ran = await $.process.run([BINARY, 'hook', 'pre-compact-snapshot'], {
      stdin: payload,
      timeoutMs: TIMEOUT_MS,
    })
    if (ran.exitCode === 0 && ran.stdout.trim() !== '') {
      const answer: unknown = JSON.parse(ran.stdout)
      if (
        typeof answer === 'object' &&
        answer !== null &&
        'instructions' in answer &&
        typeof answer.instructions === 'string'
      ) {
        extra = answer.instructions
      }
    }
  } catch {
    // Never block a compaction: no binary, a timeout or a bad answer means
    // the summary is made as it would have been without this mod.
  }
  if (extra === '') return next(e)
  const instructions =
    e.instructions !== undefined && e.instructions.trim() !== ''
      ? `${e.instructions}\n\n${extra}`
      : extra
  return next({ ...e, instructions })
}

export const register: Register = on => {
  // Replay-safe: a hook that failed after `next` gets that result back, one that failed
  // before it runs the compaction now. Either way it is never refused.
  on('session.compact', steer).catch(($, e, next) => next(e))
}
