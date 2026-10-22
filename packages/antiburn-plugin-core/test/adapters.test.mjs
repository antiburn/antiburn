import assert from 'node:assert/strict'
import { test } from 'node:test'
import { readFile } from 'node:fs/promises'
import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'

const root = resolve(import.meta.dirname, '../../..')
async function loadAdapter(name, stub = '', denyDiscovery = false) {
  let source = await readFile(resolve(root, `integrations/antiburn/${name}`), 'utf8')
  source = source.replace("import { atom, update } from 'claude-code'", stub)
  if (denyDiscovery)
    source = source.replace(
      "import { discover } from '../../../packages/antiburn-plugin-core/discovery.mjs'",
      typeof denyDiscovery === 'string'
        ? denyDiscovery
        : "const discover = async () => { throw new Error('Unexpected discovery') }",
    )
  source = source.replaceAll(
    '../../../packages/antiburn-plugin-core/',
    pathToFileURL(resolve(root, 'packages/antiburn-plugin-core')).href + '/',
  )
  source = source.replaceAll(
    "'./session-checks.mjs'",
    `'${pathToFileURL(resolve(root, 'integrations/antiburn/opencode/session-checks.mjs')).href}'`,
  )
  return import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'))
}

test('Pi registers without discovery, uses session_start and no model messages', async () => {
  const mod = await loadAdapter('pi/antiburn.mjs')
  const events = new Map(),
    commands = new Map(),
    notices = []
  const pi = {
    registerCommand: (name, command) => commands.set(name, command),
    on: (event, handler) => events.set(event, handler),
  }
  mod.default(pi)
  assert.deepEqual([...events.keys()], ['session_start', 'before_agent_start'])
  assert.equal(notices.length, 0)
  const ctx = {
    mode: 'tui',
    hasUI: true,
    ui: { notify: (text) => notices.push(text) },
    sessionManager: { getSessionId: () => 'fixture-pi-one' },
  }
  await events.get('session_start')({}, ctx)
  mod.default(pi)
  await events.get('session_start')({}, ctx)
  assert.equal(notices.length, 1)
  await commands.get('antiburn').handler('help --ascii', ctx)
  assert.match(notices.at(-1), /^`antiburn`/)
  assert.match(notices.at(-1), /App connection: Not checked/)
})

test('Pi adds hidden context only while preparing a user-started run', async () => {
  const mod = await loadAdapter('pi/antiburn.mjs')
  const calls = []
  const read = async (options) => {
    calls.push(options)
    return { message: 'Antiburn context', receipt: 'c'.repeat(64) }
  }
  const acknowledgements = []
  const acknowledge = (options) => acknowledgements.push(options)
  assert.deepEqual(await mod.piSessionCheckMessage('custom', 'session-1', read), undefined)
  assert.deepEqual(await mod.piSessionCheckMessage('user', 'session-1', read, acknowledge), {
    customType: 'antiburn-session-checks',
    content: 'Antiburn context',
    display: false,
  })
  assert.deepEqual(acknowledgements, [
    { host: 'pi', sessionId: 'session-1', consumer: 'context', receipt: 'c'.repeat(64) },
  ])
  assert.deepEqual(calls, [{ host: 'pi', sessionId: 'session-1', consumer: 'context' }])
})

test('OpenCode candidate only handles root session creation on loopback and deduplicates', async () => {
  const { Antiburn } = await loadAdapter('opencode/antiburn.mjs')
  const notices = []
  const client = { tui: { showToast: async (value) => notices.push(value) } }
  const hooks = await Antiburn({ client, serverUrl: new URL('http://127.0.0.1:1234') })
  assert.equal(notices.length, 0)
  const event = {
    type: 'session.created',
    properties: { info: { id: 'fixture-opencode-one' } },
  }
  await hooks.event({ event: { type: 'session.updated' } })
  await hooks.event({
    event: { ...event, properties: { info: { id: 'subagent', parentID: 'parent' } } },
  })
  assert.equal(notices.length, 0)
  await hooks.event({ event })
  const reload = await Antiburn({ client, serverUrl: new URL('http://localhost:1234') })
  await reload.event({ event })
  assert.equal(notices.length, 1)
  const remote = await Antiburn({ client, serverUrl: new URL('https://example.com') })
  await remote.event({ event: { ...event, properties: { info: { id: 'remote' } } } })
  assert.equal(notices.length, 1)
})

test('OpenCode appends context to the primary user message before model conversion', async () => {
  const module = await loadAdapter('opencode/antiburn.mjs')
  const helpers = await loadAdapter('opencode/session-checks.mjs')
  assert.deepEqual(Object.keys(module), ['Antiburn'])
  assert.equal(Object.values(module).filter((value) => typeof value === 'function').length, 1)
  const { opencodeSessionCheckMessages } = helpers
  const calls = []
  const messages = [
    {
      info: { id: 'user-1', sessionID: 'session-1', role: 'user', agent: 'build' },
      parts: [{ id: 'part-1', type: 'text', text: 'Please inspect this fixture.' }],
    },
  ]
  const originalMessage = messages[0]
  const originalPart = messages[0].parts[0]
  const output = { messages }
  const acknowledgements = []
  await opencodeSessionCheckMessages(
    messages,
    output,
    new Set(),
    async (options) => {
      calls.push(options)
      return { message: 'Antiburn context', receipt: 'd'.repeat(64) }
    },
    (options) => acknowledgements.push(options),
  )
  assert.equal(output.messages, messages)
  assert.equal(output.messages[0].parts.length, 2)
  assert.notEqual(output.messages[0], originalMessage)
  assert.equal(output.messages[0].parts[0], originalPart)
  assert.equal(output.messages[0].parts[1].synthetic, true)
  assert.match(output.messages[0].parts[1].text, /untrusted data/)
  assert.match(output.messages[0].parts[1].text, /Antiburn context/)
  assert.deepEqual(calls, [{ host: 'opencode', sessionId: 'session-1', consumer: 'context' }])
  assert.deepEqual(acknowledgements, [
    { host: 'opencode', sessionId: 'session-1', consumer: 'context', receipt: 'd'.repeat(64) },
  ])
})

test('OpenCode retains handed-off advice through tool steps without another receipt', async () => {
  const { opencodeSessionCheckMessages } = await loadAdapter('opencode/session-checks.mjs')
  const handoffs = new Map()
  let reads = 0
  let acks = 0
  const read = async () => {
    reads += 1
    return reads === 1
      ? { message: 'Antiburn retained context', receipt: 'h'.repeat(64) }
      : undefined
  }
  const acknowledge = () => {
    acks += 1
  }
  const user = {
    info: { id: 'retained-user', sessionID: 'retained-session', role: 'user', agent: 'build' },
    parts: [{ type: 'text', text: 'Read the fixture.' }],
  }
  const first = { messages: [JSON.parse(JSON.stringify(user))] }
  await opencodeSessionCheckMessages(
    first.messages,
    first,
    new Set(),
    read,
    acknowledge,
    handoffs,
  )
  await opencodeSessionCheckMessages(
    [],
    { messages: [] },
    new Set(),
    read,
    acknowledge,
    handoffs,
  )
  const next = {
    messages: [
      JSON.parse(JSON.stringify(user)),
      {
        info: {
          id: 'tool-step',
          sessionID: 'retained-session',
          role: 'assistant',
          agent: 'build',
        },
        parts: [{ type: 'tool', tool: 'read' }],
      },
    ],
  }
  await opencodeSessionCheckMessages(
    next.messages,
    next,
    new Set(),
    read,
    acknowledge,
    handoffs,
  )
  assert.match(next.messages[0].parts[1]?.text ?? '', /Antiburn retained context/)
  await opencodeSessionCheckMessages(
    next.messages,
    next,
    new Set(),
    read,
    acknowledge,
    handoffs,
  )
  assert.equal(next.messages[0].parts.length, 2)
  assert.equal(reads, 1)
  assert.equal(acks, 1)
  const later = {
    messages: [
      ...next.messages,
      {
        info: { id: 'next-user', sessionID: 'retained-session', role: 'user', agent: 'build' },
        parts: [{ type: 'text', text: 'Continue.' }],
      },
    ],
  }
  await opencodeSessionCheckMessages(
    later.messages,
    later,
    new Set(),
    read,
    acknowledge,
    handoffs,
  )
  assert.equal(later.messages.at(-1).parts.length, 1)
  assert.equal(handoffs.size, 0)
  assert.equal(reads, 2)
  assert.equal(acks, 1)
})

test('OpenCode bounds retained turns and discards context during compaction', async () => {
  const { opencodeSessionCheckMessages } = await loadAdapter('opencode/session-checks.mjs')
  const handoffs = new Map()
  const read = async () => ({ message: 'Bounded advice', receipt: 'i'.repeat(64) })
  const makeMessages = (sessionID) => [
    {
      info: { id: 'user', sessionID, role: 'user', agent: 'build' },
      parts: [{ type: 'text', text: 'Read the fixture.' }],
    },
  ]
  for (let i = 0; i < 257; i += 1) {
    const messages = makeMessages(`bounded-${i}`)
    await opencodeSessionCheckMessages(
      messages,
      { messages },
      new Set(),
      read,
      () => {},
      handoffs,
    )
  }
  assert.equal(handoffs.size, 256)
  assert.equal(handoffs.has('bounded-0'), false)
  const messages = makeMessages('bounded-256')
  await opencodeSessionCheckMessages(
    messages,
    { messages },
    new Set(['bounded-256']),
    read,
    () => {},
    handoffs,
  )
  assert.equal(handoffs.has('bounded-256'), false)
  assert.equal(messages[0].parts.length, 1)
})

test('OpenCode does not read title requests or synthetic-only messages', async () => {
  const { opencodeSessionCheckMessages } = await loadAdapter('opencode/session-checks.mjs')
  let reads = 0
  const read = async () => {
    reads += 1
    return { message: 'Antiburn context', receipt: 'e'.repeat(64) }
  }
  const titleOutput = { messages: [] }
  // OpenCode title generation does not call experimental.chat.messages.transform.
  await opencodeSessionCheckMessages([], titleOutput, new Set(), read)
  const syntheticOnly = [
    {
      info: { id: 'user-2', sessionID: 'session-2', role: 'user', agent: 'build' },
      parts: [{ id: 'part-2', type: 'text', text: 'prior synthetic advice', synthetic: true }],
    },
  ]
  await opencodeSessionCheckMessages(
    syntheticOnly,
    { messages: syntheticOnly },
    new Set(),
    read,
  )
  assert.equal(reads, 0)
})

test('OpenCode skips a pending compaction transform and clears its session guard', async () => {
  const { opencodeSessionCheckMessages } = await loadAdapter('opencode/session-checks.mjs')
  const compactingSessions = new Set(['session-3'])
  const read = async () => ({ message: 'Antiburn context', receipt: 'g'.repeat(64) })
  const messages = [
    {
      info: { id: 'user-3', sessionID: 'session-3', role: 'user', agent: 'build' },
      parts: [{ id: 'part-3', type: 'text', text: 'normal prompt' }],
    },
  ]
  const output = { messages }
  await opencodeSessionCheckMessages(messages, output, compactingSessions, read)
  assert.equal(output.messages, messages)
  assert.equal(compactingSessions.size, 0)
  await opencodeSessionCheckMessages(messages, output, compactingSessions, read)
  assert.equal(output.messages, messages)
  assert.equal(output.messages[0].parts.length, 2)
})

test('OpenCode rejects mixed-session prompt messages before reading or acknowledging', async () => {
  const { opencodeSessionCheckMessages } = await loadAdapter('opencode/session-checks.mjs')
  const messages = [
    { info: { id: 'old', sessionID: 'old-session', role: 'assistant' }, parts: [] },
    {
      info: { id: 'user-4', sessionID: 'new-session', role: 'user', agent: 'build' },
      parts: [{ id: 'part-4', type: 'text', text: 'prompt' }],
    },
  ]
  let reads = 0
  let acks = 0
  await opencodeSessionCheckMessages(
    messages,
    { messages },
    new Set(),
    async () => {
      reads += 1
      return { message: 'Antiburn context', receipt: 'f'.repeat(64) }
    },
    () => {
      acks += 1
    },
  )
  assert.equal(reads, 0)
  assert.equal(acks, 0)
})

test('OpenCode leaves subagent sessions to the CLI root-session gate', async () => {
  const { opencodeSessionCheckMessages } = await loadAdapter('opencode/session-checks.mjs')
  const acknowledgements = []
  const read = async (options) => {
    assert.deepEqual(options, {
      host: 'opencode',
      sessionId: 'session-5',
      consumer: 'context',
    })
    return undefined
  }
  const makeUser = (id, agent) => ({
    info: { id, sessionID: 'session-5', role: 'user', agent },
    parts: [{ id: `${id}-part`, type: 'text', text: 'prompt' }],
  })
  const subagent = [makeUser('subagent-user', 'explore')]
  const subagentOutput = { messages: subagent }
  await opencodeSessionCheckMessages(subagent, subagentOutput, new Set(), read, (receipt) =>
    acknowledgements.push(receipt),
  )
  assert.equal(subagentOutput.messages, subagent)
  assert.equal(acknowledgements.length, 0)
})

test('Pi RPC notifies without stdout and unsupported print avoids discovery', async () => {
  const mod = await loadAdapter('pi/antiburn.mjs', '', true)
  const commands = new Map(),
    notices = []
  mod.default({ registerCommand: (name, command) => commands.set(name, command), on: () => {} })
  const ctx = {
    mode: 'rpc',
    hasUI: true,
    ui: { notify: (text) => notices.push(text) },
    sessionManager: { getSessionId: () => 'fixture-pi-denied' },
  }
  await commands.get('antiburn').handler('help', ctx)
  assert.equal(notices.length, 1)
  assert.match(notices[0], /Usage: \/antiburn/)
  await commands.get('antiburn').handler('status', { ...ctx, mode: 'print', hasUI: false })
  assert.equal(notices.length, 1)
})

test('Pi first RPC help reports availability once and status always rechecks', async () => {
  const mod = await loadAdapter(
    'pi/antiburn.mjs',
    '',
    "let checks = 0; export const countChecks = () => checks; const discover = async () => { checks++; return {state: 'found'} }",
  )
  const commands = new Map(),
    notices = []
  mod.default({ registerCommand: (name, command) => commands.set(name, command), on: () => {} })
  const ctx = {
    mode: 'rpc',
    hasUI: true,
    ui: { notify: (text) => notices.push(text) },
    sessionManager: { getSessionId: () => 'fixture-pi-first-help' },
  }
  const run = (args) => commands.get('antiburn').handler(args, ctx)
  await run('help --ascii')
  assert.equal(mod.countChecks(), 1)
  assert.match(notices[0], /^`antiburn` CLI found\n/)
  await run('help')
  assert.equal(mod.countChecks(), 1)
  assert.doesNotMatch(notices[1], /CLI found/)
  await run('status')
  await run('status')
  assert.equal(mod.countChecks(), 3)
  assert.match(notices.at(-1), /CLI: Found/)
  const second = { ...ctx, sessionManager: { getSessionId: () => 'fixture-pi-first-status' } }
  await commands.get('antiburn').handler('status', second)
  await commands.get('antiburn').handler('help', second)
  assert.equal(mod.countChecks(), 4)
  assert.doesNotMatch(notices.at(-1), /CLI found/)
})
