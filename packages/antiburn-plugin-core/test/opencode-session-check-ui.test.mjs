import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtemp, readFile, realpath, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  createOpenCodeUiController,
  loopbackOrigin,
  openCodeMarkdownStyles,
  resolveOpenCodeUiSession,
} from '../../../integrations/antiburn/opencode/session-check-ui.mjs'
import { opencodeSessionCheckMessages } from '../../../integrations/antiburn/opencode/session-checks.mjs'
import { readSessionChecks } from '../session-checks.mjs'
import { formatSessionCheckUpdates } from '../session-checks-validation.mjs'
import { assemble } from '../../../scripts/assemble-antiburn-core-plugin.mjs'
import { assembleAgentIntegrations } from '../../../scripts/assemble-agent-integrations.mjs'

const tick = () => new Promise((resolve) => setTimeout(resolve, 0))
function deferred() {
  let resolve
  const promise = new Promise((accept) => {
    resolve = accept
  })
  return { promise, resolve }
}
function delivery(overrides = {}) {
  return {
    message: '',
    sessionScope: 'main',
    freshnessEvaluated: false,
    state: 'ready',
    snapshot: [
      {
        id: 'yaml:fixture',
        check_id: 'fixture',
        name: 'A title',
        message: 'A message',
        status: 'failure',
        eligibility: 'ready',
        assessed_at_epoch: 1,
        checking: false,
      },
    ],
    updates: [{ id: 'yaml:fixture', kind: 'finding', name: 'A title', message: 'A message' }],
    receipt: 'a'.repeat(64),
    ...overrides,
  }
}
function harness(options = {}) {
  let route = 'session-a'
  let result = delivery()
  const frames = []
  const reads = []
  const acks = []
  const timers = new Set()
  const controller = createOpenCodeUiController({
    currentSessionId: () => route,
    resolveSession: async () => 'http://127.0.0.1:1234',
    read: async (options) => {
      reads.push(options)
      return result
    },
    acknowledge: async (options) => {
      acks.push(options)
      return true
    },
    onFrame: (frame) => frames.push(frame),
    schedule: (fn) => {
      timers.add(fn)
      return fn
    },
    cancel: (fn) => timers.delete(fn),
    ...options,
  })
  return {
    controller,
    frames,
    reads,
    acks,
    frame: () => frames.at(-1),
    result(value) {
      result = value
    },
    route(value, activate = true) {
      route = value
      if (activate) controller.activate(value)
    },
    async flush() {
      for (const fn of [...timers]) {
        timers.delete(fn)
        fn()
      }
      await tick()
    },
    async start() {
      controller.activate(route)
      await tick()
    },
  }
}

test('UI formats the complete snapshot with canonical plain rails and waits for component mount', async () => {
  const h = harness()
  const message = 'Paragraph one.\n\n' + 'Long authored text '.repeat(200) + '\nEND'
  const value = delivery()
  value.snapshot[0].message = message
  value.updates[0].message = message
  h.result(value)
  await h.start()
  assert.equal(h.frame().text, formatSessionCheckUpdates(value.updates, 'text'))
  assert.equal(h.frame().markdown, formatSessionCheckUpdates(value.updates, 'markdown'))
  assert.ok(h.frame().text.includes(message))
  assert.equal(h.acks.length, 0)
  await h.flush()
  assert.equal(h.acks.length, 0)
  h.controller.mounted(h.frame())
  await h.flush()
  assert.deepEqual(h.acks, [
    { host: 'opencode', sessionId: 'session-a', consumer: 'ui', receipt: 'a'.repeat(64) },
  ])
  h.controller.dispose()
})

test('preferred Markdown preserves authored formatting and keeps a complete plain-text fallback', async () => {
  const h = harness()
  const name = 'A [literal] *title*'
  const message =
    '**Bold advice** with [a link](https://example.invalid/read).\n\nSecond paragraph.\nEND-MARKDOWN'
  const value = delivery()
  value.snapshot[0] = { ...value.snapshot[0], name, message }
  value.updates[0] = { ...value.updates[0], name, message }
  h.result(value)
  await h.start()
  assert.equal(h.frame().markdown, formatSessionCheckUpdates(value.updates))
  assert.equal(h.frame().text, formatSessionCheckUpdates(value.updates, 'text'))
  assert.ok(h.frame().markdown.includes(message))
  assert.ok(h.frame().text.includes(message))
  assert.ok(h.frame().text.includes(name))
  assert.ok(h.frame().markdown.includes('A \\[literal\\] \\*title\\*'))
  h.controller.dispose()
})

test('Markdown styles use public host theme tokens with semantic emphasis', () => {
  const theme = {
    markdownText: 'body',
    markdownHeading: 'heading',
    markdownStrong: 'strong',
    markdownEmph: 'emphasis',
    markdownCode: 'code',
    markdownLink: 'link',
    markdownLinkText: 'link-label',
    markdownListItem: 'list',
    markdownBlockQuote: 'quote',
  }
  const styles = openCodeMarkdownStyles(theme)
  assert.deepEqual(styles.default, { fg: theme.markdownText })
  assert.deepEqual(styles['markup.strong'], { fg: theme.markdownStrong, bold: true })
  assert.deepEqual(styles['markup.italic'], { fg: theme.markdownEmph, italic: true })
  assert.deepEqual(styles['markup.link.label'], { fg: theme.markdownLinkText, underline: true })
  assert.equal(styles['markup.raw'].fg, theme.markdownCode)
  assert.equal(styles['markup.heading'].fg, theme.markdownHeading)
  assert.equal(styles['markup.quote'].fg, theme.markdownBlockQuote)
})

test('message updates are throttled while activation and idle refresh immediately', async () => {
  let now = 0
  const h = harness({ now: () => now })
  await h.start()
  for (const time of [0, 100, 200, 300]) {
    now = time
    await h.controller.refreshFromMessage()
  }
  assert.equal(h.reads.length, 2)
  await h.controller.refresh()
  assert.equal(h.reads.length, 3)
  h.route('session-b')
  await tick()
  assert.equal(h.reads.length, 4)
  assert.equal(h.reads.at(-1).sessionId, 'session-b')
  await h.controller.refreshFromMessage()
  assert.equal(h.reads.length, 5)
  now = 1300
  await h.controller.refreshFromMessage()
  assert.equal(h.reads.length, 6)
  h.controller.dispose()
})

test('routing A/B/A rejects stale reads and restores only freshly read session frames', async () => {
  const first = deferred()
  const reads = []
  const h = harness({
    read: async ({ sessionId }) => {
      reads.push(sessionId)
      return reads.length === 1
        ? first.promise
        : delivery({ snapshot: [{ ...delivery().snapshot[0], name: sessionId }], updates: [] })
    },
  })
  h.controller.activate('session-a')
  await tick()
  h.route('session-b')
  first.resolve(delivery())
  await tick()
  assert.deepEqual(reads, ['session-a', 'session-b'])
  assert.match(h.frame().text, /session-b/)
  assert.doesNotMatch(h.frame().text, /A title/)
  h.route('session-a')
  assert.equal(h.frame(), undefined)
  await tick()
  assert.match(h.frame().text, /session-a/)
  assert.equal(h.acks.length, 0)
  h.controller.dispose()
})

test('route changes before the activation effect and stale component mounts cannot acknowledge', async () => {
  const h = harness()
  await h.start()
  const old = h.frame()
  h.controller.mounted(old)
  h.route('session-b', false)
  await h.flush()
  assert.equal(h.acks.length, 0)
  h.controller.mounted(old)
  await h.flush()
  assert.equal(h.acks.length, 0)
  h.controller.dispose()
})

test('unmount and disposal cancel scheduled acknowledgements and ignore late reads', async () => {
  const h = harness()
  await h.start()
  const cleanup = h.controller.mounted(h.frame())
  cleanup()
  await h.flush()
  assert.equal(h.acks.length, 0)
  h.controller.mounted(h.frame())
  h.controller.dispose()
  await h.flush()
  assert.equal(h.frame(), undefined)
  assert.equal(h.acks.length, 0)
  const pending = deferred()
  const late = harness({ read: () => pending.promise })
  late.controller.activate('session-a')
  await tick()
  late.controller.dispose()
  pending.resolve(delivery())
  await tick()
  assert.equal(late.frame(), undefined)
})

test('same revision stays mounted and is acknowledged once; failed ack retries on next refresh', async () => {
  let attempts = 0
  const h = harness({ acknowledge: async () => ++attempts > 1 })
  await h.start()
  const frame = h.frame()
  h.controller.mounted(frame)
  await h.flush()
  assert.equal(attempts, 1)
  await h.controller.refresh()
  assert.equal(h.frame(), frame)
  await h.flush()
  assert.equal(attempts, 2)
  await h.controller.refresh()
  await h.flush()
  assert.equal(attempts, 2)
  h.controller.dispose()
})

test('receipt removal after acknowledgement preserves the mounted frame', async () => {
  const h = harness()
  await h.start()
  const frame = h.frame()
  h.controller.mounted(frame)
  await h.flush()
  const count = h.frames.length
  h.result(delivery({ receipt: null, updates: [] }))
  await h.controller.refresh()
  await h.flush()
  assert.equal(h.frame(), frame)
  assert.equal(h.frames.length, count)
  assert.equal(h.acks.length, 1)
  h.result(delivery({ receipt: 'b'.repeat(64) }))
  await h.controller.refresh()
  assert.notEqual(h.frame(), frame)
  await h.flush()
  assert.equal(h.acks.length, 1)
  h.controller.mounted(h.frame())
  await h.flush()
  assert.equal(h.acks[1].receipt, 'b'.repeat(64))
  h.controller.dispose()
})

test('a changed revision requires a new mounted component and cannot ack the old revision', async () => {
  const h = harness()
  await h.start()
  const old = h.frame()
  h.controller.mounted(old)
  const next = delivery({ receipt: 'b'.repeat(64) })
  next.snapshot[0].message = 'Changed message'
  h.result(next)
  await h.controller.refresh()
  await h.flush()
  assert.equal(h.acks.length, 0)
  assert.notEqual(h.frame(), old)
  h.controller.mounted(old)
  await h.flush()
  assert.equal(h.acks.length, 0)
  h.controller.mounted(h.frame())
  await h.flush()
  assert.equal(h.acks[0].receipt, 'b'.repeat(64))
  h.controller.dispose()
})

test('restart restores acknowledged findings from full snapshot; pending UI receipt replays separately', async () => {
  for (const receipt of [null, 'a'.repeat(64)]) {
    const h = harness()
    h.result(delivery({ receipt, updates: [] }))
    await h.start()
    assert.match(h.frame().text, /A message/)
    h.controller.mounted(h.frame())
    await h.flush()
    assert.equal(h.acks.length, receipt ? 1 : 0)
    h.controller.dispose()
  }
})

test('clear and withdrawal replace stale findings and persist for unchanged active snapshot only', async () => {
  for (const kind of ['cleared', 'invalidated']) {
    const h = harness()
    await h.start()
    const row = {
      ...delivery().snapshot[0],
      status: kind === 'cleared' ? 'success' : null,
      message: null,
      eligibility: kind === 'cleared' ? 'ready' : 'unavailable',
    }
    const next = delivery({
      state: kind === 'cleared' ? 'ready' : 'disabled',
      snapshot: [row],
      updates: [{ id: row.id, name: row.name, message: null, kind }],
      receipt: 'b'.repeat(64),
    })
    h.result(next)
    await h.controller.refresh()
    assert.match(h.frame().text, kind === 'cleared' ? /cleared/ : /withdrawn/)
    assert.doesNotMatch(h.frame().text, /A message/)
    h.controller.mounted(h.frame())
    await h.flush()
    h.result({ ...next, updates: [], receipt: null })
    await h.controller.refresh()
    assert.match(h.frame().text, kind === 'cleared' ? /cleared/ : /withdrawn/)
    h.route(undefined)
    h.route('session-a')
    await tick()
    assert.equal(h.frame(), undefined)
    h.controller.dispose()
  }
})

test('unavailable reads remove stale frames and leave receipts pending', async () => {
  const h = harness()
  await h.start()
  h.controller.mounted(h.frame())
  h.result(undefined)
  await h.controller.refresh()
  await h.flush()
  assert.equal(h.frame(), undefined)
  assert.equal(h.acks.length, 0)
  h.result(delivery())
  await h.controller.refresh()
  assert.match(h.frame().text, /A message/)
  h.controller.dispose()
})

test('home, custom, unsafe IDs and unsupported session scope do not hand off UI', async () => {
  for (const id of [undefined, '', 'bad\nidentity', 'x'.repeat(513)]) {
    const h = harness()
    h.route(id)
    await tick()
    assert.equal(h.reads.length, 0)
    assert.equal(h.acks.length, 0)
    h.controller.dispose()
  }
  const h = harness()
  h.result(delivery({ sessionScope: 'unsupported' }))
  await h.start()
  assert.equal(h.frame(), undefined)
  h.controller.dispose()
})

test('remote or unknown bindings fail closed before local CLI reads; changed server clears the frame', async () => {
  let binding
  const h = harness({ resolveSession: async () => binding })
  await h.start()
  assert.equal(h.reads.length, 0)
  binding = 'http://127.0.0.1:1234'
  await h.controller.refresh()
  assert.ok(h.frame())
  binding = 'http://127.0.0.1:5678'
  await h.controller.refresh()
  assert.equal(h.frame(), undefined)
  assert.equal(h.reads.length, 1)
  binding = 'http://127.0.0.1:1234'
  await h.controller.refresh()
  assert.equal(h.frame(), undefined)
  assert.equal(h.reads.length, 1)
  h.controller.dispose()
})

test('the same OpenCode server recovers after a failed session lookup', async () => {
  let binding = 'http://127.0.0.1:1234'
  const h = harness({ resolveSession: async () => binding })
  await h.start()
  assert.ok(h.frame())

  binding = undefined
  await h.controller.refresh()
  assert.equal(h.frame(), undefined)
  assert.equal(h.reads.length, 1)

  binding = 'http://127.0.0.1:1234'
  await h.controller.refresh()
  assert.ok(h.frame())
  assert.equal(h.reads.length, 2)
  h.controller.dispose()
})

function apiResult(overrides = {}) {
  return {
    request: { url: 'http://127.0.0.1:1234/session/session-a' },
    response: { ok: true, redirected: false, url: 'http://127.0.0.1:1234/session/session-a' },
    data: { id: 'session-a' },
    ...overrides,
  }
}
function api(result, version = '1.19.3') {
  return { app: { version }, client: { session: { get: async () => result } } }
}
test('SDK public request/response URLs and exact main session prove only loopback binding', async () => {
  let requestOptions
  const connected = api(apiResult())
  connected.client.session.get = async (_parameters, options) => {
    requestOptions = options
    return apiResult()
  }
  assert.equal(await resolveOpenCodeUiSession(connected, 'session-a'), 'http://127.0.0.1:1234')
  assert.equal(requestOptions.redirect, 'error')
  assert.ok(requestOptions.signal instanceof globalThis.AbortSignal)
  assert.equal(
    await resolveOpenCodeUiSession(api(apiResult()), 'session-a'),
    'http://127.0.0.1:1234',
  )
  for (const result of [
    apiResult({ request: { url: 'http://remote.example/session/session-a' } }),
    apiResult({ request: { url: 'http://opencode.internal/session/session-a' } }),
    apiResult({ request: {} }),
    apiResult({ response: { ok: true, redirected: true, url: 'http://127.0.0.1:1234/' } }),
    apiResult({ response: { ok: true, redirected: false, url: 'https://remote.example/' } }),
    apiResult({ data: { id: 'session-b' } }),
    apiResult({ data: { id: 'session-a', parentID: 'session-root' } }),
    apiResult({ error: 'unavailable' }),
  ])
    assert.equal(await resolveOpenCodeUiSession(api(result), 'session-a'), undefined)
  assert.equal(
    await resolveOpenCodeUiSession(api(apiResult(), '1.19.2'), 'session-a'),
    undefined,
  )
  assert.equal(await resolveOpenCodeUiSession(api(apiResult()), 'bad\nidentity'), undefined)
  for (const value of [
    'file:///tmp/x',
    'http://localhost.evil/',
    'http://user@localhost/',
    'garbage',
    undefined,
  ])
    assert.equal(loopbackOrigin(value), undefined)
})

test('malformed, remote, wrong session and child deliveries fail shared validation without acknowledgement', async () => {
  const valid = {
    protocol_version: 1,
    session_id: 'session-a',
    identity: { environment_key: 'native', agent: 'opencode', session_id: 'session-a' },
    session_scope: 'main',
    freshness_evaluated: false,
    state: 'ready',
    snapshot: delivery().snapshot,
    updates: delivery().updates,
    receipt: 'a'.repeat(64),
  }
  for (const value of [
    '{',
    { ...valid, identity: { ...valid.identity, environment_key: 'remote' } },
    { ...valid, session_id: 'session-b' },
    { ...valid, session_scope: 'unsupported' },
    { ...valid, snapshot: null },
    { ...valid, receipt: 'bad' },
  ]) {
    const h = harness({
      read: (options) =>
        readSessionChecks({
          ...options,
          run: async () => (typeof value === 'string' ? value : JSON.stringify(value)),
        }),
    })
    await h.start()
    assert.equal(h.frame(), undefined)
    await h.flush()
    assert.equal(h.acks.length, 0)
    h.controller.dispose()
  }
})

test('headless context remains independent of UI absence, failure and acknowledgements', async () => {
  const consumers = []
  const read = async ({ consumer }) => {
    consumers.push(consumer)
    return consumer === 'ui' ? undefined : delivery({ message: 'Context advice' })
  }
  const messages = [
    {
      info: { id: 'message-a', sessionID: 'session-a', role: 'user', agent: 'build' },
      parts: [{ type: 'text', text: 'Ordinary request' }],
    },
  ]
  const output = { messages }
  const acks = []
  await opencodeSessionCheckMessages(messages, output, new Set(), read, (value) =>
    acks.push(value),
  )
  assert.match(output.messages[0].parts.at(-1).text, /Context advice/)
  assert.deepEqual(consumers, ['context'])
  assert.equal(acks[0].consumer, 'context')
  const h = harness({ read })
  await h.start()
  assert.equal(h.frame(), undefined)
  assert.equal(h.acks.length, 0)
  h.controller.dispose()
  const ui = harness()
  await ui.start()
  ui.controller.mounted(ui.frame())
  await ui.flush()
  assert.equal(ui.acks[0].consumer, 'ui')
  ui.controller.dispose()
})

test('separate clients reconstruct frames when another client has consumed the global UI revision', async () => {
  const first = harness()
  await first.start()
  first.controller.mounted(first.frame())
  await first.flush()
  const second = harness()
  second.result(delivery({ updates: [], receipt: null }))
  await second.start()
  assert.equal(first.frame().text, second.frame().text)
  assert.equal(second.acks.length, 0)
  first.controller.dispose()
  second.controller.dispose()
})

test('public and managed packages keep optional target-exclusive TUI module separate from server', async () => {
  const temp = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-opencode-ui-')))
  try {
    const standalone = join(temp, 'standalone')
    const managed = join(temp, 'managed')
    await assemble(standalone)
    await assembleAgentIntegrations(managed)
    const manifest = JSON.parse(
      await readFile(join(standalone, 'opencode/package.json'), 'utf8'),
    )
    assert.deepEqual(manifest.exports, {
      '.': './runtime/antiburn.mjs',
      './tui': './runtime/session-check-ui.tsx',
    })
    assert.deepEqual(manifest.peerDependencies, { '@opencode-ai/plugin': '1.19.2 || 1.19.3' })
    const readme = await readFile(join(standalone, 'opencode/README.md'), 'utf8')
    assert.match(
      readme,
      /OpenCode 1\.19\.3 with host-provided @opentui\/core and @opentui\/solid 0\.4\.5 and solid-js 1\.9\.12/,
    )
    for (const root of [standalone, managed]) {
      const source = await readFile(join(root, 'opencode/runtime/session-check-ui.tsx'), 'utf8')
      assert.match(source, /import type .*TuiPluginModule/)
      assert.match(source, /app_bottom/)
      assert.doesNotMatch(source, /session_prompt|server:/)
      const controller = await readFile(
        join(root, 'opencode/runtime/session-check-ui.mjs'),
        'utf8',
      )
      assert.match(controller, /\.\.\/lib\/session-checks.mjs/)
      const server = await readFile(join(root, 'opencode/runtime/antiburn.mjs'), 'utf8')
      assert.doesNotMatch(server, /session-check-ui|opentui|solid-js/)
    }
  } finally {
    await rm(temp, { recursive: true, force: true })
  }
})
