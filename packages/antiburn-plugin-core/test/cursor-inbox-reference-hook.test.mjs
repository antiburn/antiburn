import assert from 'node:assert/strict'
import { test } from 'node:test'
import { spawnSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, realpathSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import {
  enrollInbox,
  readInboxReferences,
} from '../../../integrations/antiburn/cursor/inbox-bridge.mjs'
import { cursorSessionCheckOutput } from '../../../integrations/antiburn/cursor/session-check-hook.mjs'

test('separate reference hook emits no model output or receipt and context behavior is unchanged', async (t) => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'ab-hook-')))
  t.after(() => rmSync(root, { recursive: true, force: true }))
  const workspace = join(root, 'workspace')
  mkdirSync(workspace)
  const base = join(root, 'inbox-v1')
  const enrollment = enrollInbox(base, [workspace])
  const input = {
    hook_event_name: 'postToolUse',
    conversation_id: 'session-1',
    generation_id: 'turn-1',
    workspace_roots: [workspace],
  }
  const hook = resolve(
    import.meta.dirname,
    '../../../integrations/antiburn/cursor/inbox-reference-hook.mjs',
  )
  const run = spawnSync(process.execPath, [hook, '--inbox-dir', base], {
    input: JSON.stringify(input),
    encoding: 'utf8',
  })
  assert.equal(run.status, 0)
  assert.equal(run.stdout, '')
  assert.equal(run.stderr, '')
  assert.deepEqual(readInboxReferences(enrollment), ['session-1'])
  const calls = []
  const context = await cursorSessionCheckOutput(input, async (options) => {
    calls.push(options)
    return {
      message: 'canonical context',
      receipt: 'b'.repeat(64),
      updates: [],
      snapshot: [],
      state: 'ready',
      freshnessEvaluated: false,
      sessionScope: 'main',
    }
  })
  assert.deepEqual(context, { additional_context: 'canonical context' })
  assert.equal(calls[0].consumer, 'context')
  const absent = spawnSync(process.execPath, [hook, '--inbox-dir', join(root, 'missing')], {
    input: JSON.stringify(input),
    encoding: 'utf8',
  })
  assert.equal(absent.status, 0)
  assert.equal(absent.stdout, '')
})
