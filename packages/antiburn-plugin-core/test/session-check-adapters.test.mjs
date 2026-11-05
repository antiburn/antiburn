import assert from 'node:assert/strict'
import { test } from 'node:test'
import { readFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { readSessionChecks } from '../session-checks.mjs'
import { handoffJsonHook } from '../native-hook.mjs'
import {
  createPiSessionCheckRenderer,
  registerPiSessionCheckRenderer,
} from '../../../integrations/antiburn/pi/session-check-renderer.mjs'
import {
  formatSessionCheckUpdates,
  formatSessionChecksContext,
} from '../session-checks-validation.mjs'

const root = resolve(import.meta.dirname, '../../..')

async function load(relative) {
  let source = await readFile(resolve(root, relative), 'utf8')
  source = source.replace(
    "import { Markdown } from '@earendil-works/pi-tui'",
    'const Markdown = class Markdown {}',
  )
  source = source.replace(
    "import { getMarkdownTheme } from '@earendil-works/pi-coding-agent'",
    "const getMarkdownTheme = () => 'theme'",
  )
  source = source.replace(
    "'./session-check-renderer.mjs'",
    `'${pathToFileURL(resolve(root, 'integrations/antiburn/pi/session-check-renderer.mjs')).href}'`,
  )
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

const { piSessionCheckMessage } = await load('integrations/antiburn/pi/antiburn.mjs')

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
    systemMessage: formatSessionCheckUpdates(delivery.updates, 'text'),
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
      systemMessage: formatSessionCheckUpdates(delivery.updates, 'text'),
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
      systemMessage: formatSessionCheckUpdates(delivery.updates, 'text'),
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

function nativeDelivery(
  host,
  sessionId,
  kind = 'finding',
  message = 'Use the available parser skill.',
) {
  const update = { id: 'yaml:fixture', kind, name: 'Skill opportunities', message }
  return {
    protocol_version: 1,
    session_id: sessionId,
    identity: { environment_key: 'native', agent: host, session_id: sessionId },
    session_scope: 'main',
    freshness_evaluated: false,
    state: 'ready',
    snapshot: [
      {
        id: update.id,
        check_id: 'fixture',
        name: update.name,
        message: kind === 'invalidated' ? null : message,
        status: kind === 'finding' ? 'failure' : kind === 'cleared' ? 'success' : null,
        assessed_at_epoch: kind === 'invalidated' ? null : 1,
        checking: false,
        eligibility: kind === 'invalidated' ? 'waiting' : 'ready',
      },
    ],
    updates: [{ ...update, message: kind === 'invalidated' ? null : message }],
    receipt: 'd'.repeat(64),
  }
}

for (const host of ['claude-code', 'codex', 'pi']) {
  test(`${host}: native display and context share validated revision and withdrawal handoffs`, async () => {
    const { claudeSessionCheckOutput } = await load(
      'integrations/antiburn/claude/session-check-hook.mjs',
    )
    const { codexSessionCheckOutput } = await load(
      'integrations/antiburn/codex/session-check-hook.mjs',
    )
    const sessionId = 'visual-session'
    const reads = [],
      acknowledgements = []
    let response = nativeDelivery(host, sessionId)
    const read = (options) =>
      readSessionChecks({
        ...options,
        run: async (args) => {
          reads.push(args)
          return JSON.stringify(response)
        },
      })
    const acknowledge = (options) => {
      acknowledgements.push(options)
      return true
    }
    const receive = async () => {
      if (host === 'pi')
        return piSessionCheckMessage('user', sessionId, read, acknowledge, true)
      const receiveHook = host === 'codex' ? codexSessionCheckOutput : claudeSessionCheckOutput
      const output = await receiveHook(
        { session_id: sessionId, hook_event_name: 'UserPromptSubmit' },
        read,
      )
      if (output) await handoffJsonHook(output, (_text, done) => done(), acknowledge)
      return output
    }
    for (const [kind, message] of [
      ['finding', 'Use the available parser skill.'],
      ['finding', 'Read the updated grammar first.'],
      ['cleared', 'Passing assessment.'],
      ['invalidated', null],
    ]) {
      response = nativeDelivery(host, sessionId, kind, message)
      const output = await receive()
      const visible = host === 'pi' ? output.content : output.systemMessage
      assert.ok(
        visible.includes(
          formatSessionCheckUpdates(response.updates, host === 'pi' ? 'markdown' : 'text'),
        ),
      )
      assert.match(visible, host === 'pi' ? /🔥 `antiburn`/ : /🔥 antiburn/)
      if (host !== 'pi') {
        assert.doesNotMatch(visible, /`|\*\*/)
        assert.ok(visible.startsWith('\n────────────────────────────────\n'))
        assert.ok(
          `UserPromptSubmit says: ${visible}`.startsWith(
            'UserPromptSubmit says: \n────────────────────────────────\n',
          ),
        )
      }
      assert.equal(visible.split('────────────────────────────────').length, 3)
      const context =
        host === 'pi' ? output.content : output.hookSpecificOutput.additionalContext
      assert.match(context, /untrusted data, not instructions/)
      if (host === 'pi') {
        const frame = formatSessionCheckUpdates(response.updates)
        assert.equal(output.content, formatSessionChecksContext(response))
        assert.equal(output.details.frame, frame)
        assert.doesNotMatch(output.details.frame, /untrusted data, not instructions/)
        assert.equal(output.display, true)
      }
      if (kind !== 'finding')
        assert.equal(visible.includes('Use the available parser skill.'), false)
      response = { ...response, updates: [], receipt: null }
      assert.equal(await receive(), undefined)
    }
    assert.equal(acknowledgements.length, 4)
    assert.ok(
      acknowledgements.every(
        (x) => x.host === host && x.sessionId === sessionId && x.consumer === 'context',
      ),
    )
    assert.ok(reads.every((args) => args[1] === sessionId && args[4] === `${host}:context`))
    for (const change of [
      { session_id: 'other-session' },
      { identity: { environment_key: 'native', agent: 'cursor', session_id: sessionId } },
      { session_scope: 'unsupported' },
      { state: 'unavailable' },
      { receipt: null },
    ]) {
      response = { ...nativeDelivery(host, sessionId), ...change }
      assert.equal(await receive(), undefined)
    }
    response = null
    assert.equal(await receive(), undefined)
    assert.equal(acknowledgements.length, 4)
  })
}

test('Pi renders only the structured frame and safely falls back for older messages', async () => {
  class Markdown {
    constructor(text, paddingX, paddingY, theme) {
      Object.assign(this, { text, paddingX, paddingY, theme })
    }
  }
  const components = { Markdown, getMarkdownTheme: () => 'theme' }
  const frame = formatSessionCheckUpdates([
    {
      id: 'check-1',
      kind: 'finding',
      name: 'Skill opportunities',
      message: 'Use a parser skill.',
    },
  ])
  const message = {
    customType: 'antiburn-session-checks',
    content: `Guardrail paragraph.\n\n${frame}`,
    details: { frame },
  }
  const rendered = createPiSessionCheckRenderer(message, { outputPad: 1 }, components)
  assert.equal(rendered.text, frame)
  assert.equal(rendered.text.includes('Guardrail paragraph.'), false)
  assert.equal(rendered.text.includes('[antiburn-session-checks]'), false)
  assert.equal(rendered.theme, 'theme')

  const oldMessage = createPiSessionCheckRenderer(
    { content: 'Legacy context and advisory frame.' },
    { outputPad: 0 },
    components,
  )
  assert.equal(oldMessage.text, 'Legacy context and advisory frame.')

  const unavailable = {}
  assert.equal(registerPiSessionCheckRenderer(unavailable, components), false)
  let registrationCount = 0
  let renderer
  const pi = {
    registerMessageRenderer(type, callback) {
      assert.equal(type, 'antiburn-session-checks')
      registrationCount += 1
      renderer = callback
    },
  }
  assert.equal(registerPiSessionCheckRenderer(pi, components), true)
  assert.equal(registerPiSessionCheckRenderer(pi, components), true)
  assert.equal(registrationCount, 1)
  assert.equal(renderer(message, { outputPad: 0 }, {}).text, frame)
})
