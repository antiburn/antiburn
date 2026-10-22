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

test('OpenCode appends context at the session-bound system transform', async () => {
  const module = await loadAdapter('opencode/antiburn.mjs')
  const helpers = await loadAdapter('opencode/session-checks.mjs')
  assert.deepEqual(Object.keys(module), ['Antiburn'])
  assert.equal(Object.values(module).filter((value) => typeof value === 'function').length, 1)
  const { opencodeSessionCheckContext } = helpers
  const calls = []
  const output = { system: ['You are an agent.'] }
  const acknowledgements = []
  await opencodeSessionCheckContext(
    'session-1',
    output,
    async (options) => {
      calls.push(options)
      return { message: 'Antiburn context', receipt: 'd'.repeat(64) }
    },
    (options) => acknowledgements.push(options),
  )
  assert.deepEqual(output.system, ['You are an agent.', 'Antiburn context'])
  assert.deepEqual(calls, [{ host: 'opencode', sessionId: 'session-1', consumer: 'context' }])
  assert.deepEqual(acknowledgements, [
    { host: 'opencode', sessionId: 'session-1', consumer: 'context', receipt: 'd'.repeat(64) },
  ])
  await opencodeSessionCheckContext(undefined, output, async () => {
    throw new Error('must not poll without a session id')
  })
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
