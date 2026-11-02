import assert from 'node:assert/strict'
import { test } from 'node:test'
import { readFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../../..')

async function load(relative) {
  let source = await readFile(resolve(root, relative), 'utf8')
  source = source.replaceAll(
    '../../../packages/antiburn-plugin-core/',
    pathToFileURL(resolve(root, 'packages/antiburn-plugin-core')).href + '/',
  )
  source = source.replaceAll(
    '../../packages/antiburn-plugin-core/',
    pathToFileURL(resolve(root, 'packages/antiburn-plugin-core')).href + '/',
  )
  return import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'))
}

const delivery = {
  message: 'Antiburn check details',
  updates: [],
  state: 'ready',
  freshnessEvaluated: false,
  receipt: 'b'.repeat(64),
}

test('Claude and Codex add context at user prompt submission and ignore empty sessions', async () => {
  const claude = await load('integrations/antiburn/claude/session-check-hook.mjs')
  const codex = await load('integrations/antiburn/codex/session-check-hook.mjs')
  const read = async (options) => ({
    ...delivery,
    host: options.host,
    sessionId: options.sessionId,
  })
  assert.deepEqual(
    await claude.claudeSessionCheckOutput(
      { session_id: 's1', hook_event_name: 'UserPromptSubmit', agent_id: 'child' },
      read,
    ),
    undefined,
  )
  assert.deepEqual(
    await claude.claudeSessionCheckOutput(
      { session_id: 's1', hook_event_name: 'PostToolUse', agent_type: 'Explore' },
      read,
    ),
    undefined,
  )
  assert.deepEqual(
    await claude.claudeSessionCheckOutput({ session_id: 's1', hook_event_name: 'Stop' }, read),
    undefined,
  )
  const claudeOutput = await claude.claudeSessionCheckOutput(
    { session_id: 's1', hook_event_name: 'UserPromptSubmit' },
    read,
  )
  assert.deepEqual(claudeOutput, {
    hookSpecificOutput: {
      hookEventName: 'UserPromptSubmit',
      additionalContext: delivery.message,
    },
  })
  assert.deepEqual(
    await claude.claudeSessionCheckOutput(
      { session_id: 's1', hook_event_name: 'PostToolUse' },
      read,
    ),
    {
      hookSpecificOutput: {
        hookEventName: 'PostToolUse',
        additionalContext: delivery.message,
      },
    },
  )
  assert.deepEqual(
    await codex.codexSessionCheckOutput(
      { session_id: 's2', hook_event_name: 'UserPromptSubmit' },
      read,
    ),
    {
      hookSpecificOutput: {
        hookEventName: 'UserPromptSubmit',
        additionalContext: delivery.message,
      },
    },
  )
  assert.deepEqual(await codex.codexSessionCheckOutput({}, read), undefined)
  for (const marker of [
    { agent_id: 'child' },
    { subagent_id: 'child' },
    { parent_session_id: 'parent' },
    { agent_type: 'worker' },
    { is_subagent: true },
  ])
    assert.equal(
      await codex.codexSessionCheckOutput(
        { session_id: 's2', hook_event_name: 'UserPromptSubmit', ...marker },
        read,
      ),
      undefined,
    )
  assert.equal(
    await codex.codexSessionCheckOutput(
      { session_id: 's2', hook_event_name: 'SubagentStart' },
      read,
    ),
    undefined,
  )
})

test('Cursor adds context after a tool and requires conversation and generation identity', async () => {
  const cursor = await load('integrations/antiburn/cursor/session-check-hook.mjs')
  const seen = []
  const read = async (options) => {
    seen.push(options)
    return { ...delivery, sessionId: options.sessionId }
  }
  assert.deepEqual(
    await cursor.cursorSessionCheckOutput({ conversation_id: 'c1', generation_id: 'g1' }, read),
    { additional_context: delivery.message },
  )
  assert.deepEqual(seen, [{ host: 'cursor', sessionId: 'c1', consumer: 'context' }])
  assert.deepEqual(
    await cursor.cursorSessionCheckOutput({ conversation_id: 'c1' }, read),
    undefined,
  )
  assert.equal(seen.length, 1)
})

test('Codex and Cursor status command validates arguments and uses a separate consumer', async () => {
  const status = await load('integrations/antiburn/status-command.mjs')
  const calls = []
  const read = async (options) => {
    calls.push(options)
    return {
      state: 'ready',
      sessionScope: 'main',
      freshnessEvaluated: false,
      snapshot: [],
    }
  }
  assert.equal(
    await status.renderStatusCommand(['--host', 'cursor', '--session', 's1'], read),
    'Antiburn session checks: connected (protocol v1); scope main; state ready; 0 checks (0 failed, 0 passed, 0 waiting). Freshness has not been evaluated.',
  )
  assert.deepEqual(calls, [{ host: 'cursor', sessionId: 's1', consumer: 'status' }])
  assert.equal(
    await status.renderStatusCommand(['--host', 'opencode', '--session', 's1'], read),
    undefined,
  )
  assert.equal(
    await status.renderStatusCommand(['--host', 'codex', '--session', 'bad\nvalue'], read),
    undefined,
  )
  assert.equal(calls.length, 1)
})
