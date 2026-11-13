// `claude plugin test` for the compaction mod. Not installed: lay the mod out as
// `install --compact-mod` does, copy this file into `hooks/` beside register.ts,
// and run `claude plugin test <folder>`.
//
// The kit's own `$.session.compact` carries no conversation, so the hook is called
// here directly, with a `$` that answers the three calls it makes.

import { expect, test } from 'claude-code/testing'
import { steer } from './register'

type Ran = { argv: readonly string[]; stdin: string }

const conversation = [
  { role: 'user' as const, text: 'see r2-a91f2c33e1bd', toolUses: [] },
  {
    role: 'assistant' as const,
    text: 'x'.repeat(30000),
    toolUses: [{ tool_use_id: 't1', tool: 'Bash', input: { command: 'secret' }, text: 'out' }],
  },
]

function engine(run: (argv: readonly string[], stdin: string) => unknown, ran: Ran[]) {
  return {
    session: { id: async () => 's1', cwd: async () => '/p' },
    process: {
      run: async (argv: readonly string[], init?: { stdin?: string }) => {
        ran.push({ argv, stdin: init?.stdin ?? '' })
        return run(argv, init?.stdin ?? '')
      },
    },
  }
}

const ok = (stdout: string) => ({
  exitCode: 0,
  stdout,
  stderr: '',
  isStdoutTruncated: false,
  isStderrTruncated: false,
})

async function compact(
  run: (argv: readonly string[], stdin: string) => unknown,
  input: Record<string, unknown>,
) {
  const ran: Ran[] = []
  const passed: Record<string, unknown>[] = []
  const next = async (e: Record<string, unknown>) => {
    passed.push(e)
    return { messages: [] }
  }
  const call = steer as unknown as (
    $: unknown,
    e: unknown,
    next: unknown,
  ) => Promise<unknown>
  await call(engine(run, ran), { trigger: 'auto', messages: conversation, ...input }, next)
  return { ran, passed }
}

test('the summary is told what cyberbrain answers, after what the person typed', async () => {
  const { ran, passed } = await compact(
    () => ok(JSON.stringify({ instructions: 'KEEP r2-a91f2c33e1bd' })),
    { instructions: 'focus on the plan' },
  )
  expect(passed.length).toBe(1)
  expect(passed[0]?.instructions).toBe('focus on the plan\n\nKEEP r2-a91f2c33e1bd')
  expect(ran[0]?.argv.slice(1)).toEqual(['hook', 'pre-compact-snapshot'])
  const sent = JSON.parse(ran[0]?.stdin ?? "")
  expect(sent.session_id).toBe('s1')
  expect(sent.trigger).toBe('auto')
  // Text cut, tool names only: no input, no output.
  expect(sent.messages[1].text.length).toBe(20000)
  expect(sent.messages[1].tools).toEqual(['Bash'])
  expect(ran[0]?.stdin.includes('secret')).toBe(false)
})

test('a failing cyberbrain never blocks the compaction', async () => {
  for (const run of [
    () => {
      throw new Error('no such binary')
    },
    () => ok('not json'),
    () => ok(''),
    () => ({ ...ok('{"instructions": 7}') }),
    () => ({ ...ok('{"instructions":"x"}'), exitCode: 1 }),
  ]) {
    const { passed } = await compact(run, { instructions: 'as typed' })
    expect(passed.length).toBe(1)
    expect(passed[0]?.instructions).toBe('as typed')
  }
})

test('a subagent is left alone', async () => {
  const { ran, passed } = await compact(() => ok('{"instructions":"x"}'), { agentId: 'a1' })
  expect(ran.length).toBe(0)
  expect(passed.length).toBe(1)
})
