import { test } from 'node:test'
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdtemp, mkdir, writeFile, readFile, realpath, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { promisify } from 'node:util'
import { managedExecutable } from '../managed-connection.mjs'
import { assembleAgentIntegrations } from '../../../scripts/assemble-agent-integrations.mjs'

test(
  'all desktop vendor runtimes deliver through the internal CLI with empty PATH and no terminal shortcut',
  { skip: process.platform === 'win32' },
  async () => {
    const root = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-internal-delivery-')))
    try {
      const payload = join(root, 'payload')
      await assembleAgentIntegrations(payload)
      const internal = join(root, 'internal runtime é')
      await mkdir(internal)
      const executable = join(internal, 'antiburn-cli')
      const fixture = join(internal, 'fixture.mjs')
      const shellQuote = (value) => `'${value.replaceAll("'", "'\\''")}'`
      await writeFile(
        executable,
        `#!/bin/sh\nexec ${shellQuote(process.execPath)} ${shellQuote(fixture)} "$@"\n`,
        { mode: 0o700 },
      )
      await writeFile(
        fixture,
        `
      import assert from 'node:assert/strict'
      import { appendFileSync, existsSync } from 'node:fs'
      const args = process.argv.slice(2)
      assert.equal(process.env.PATH, '')
      assert.equal(process.env.ANTIBURN_EXECUTABLE, ${JSON.stringify(executable)})
      assert.equal(existsSync(${JSON.stringify(join(root, '.local/bin/antiburn'))}), false)
      assert.equal(existsSync(${JSON.stringify(join(root, '.local/bin/antiburn-cli'))}), false)
      appendFileSync(${JSON.stringify(join(root, 'calls.jsonl'))}, JSON.stringify(args) + '\\n')
      const host = args[4].split(':')[0]
      console.log(JSON.stringify(args.includes('--ack')
        ? { protocol_version: 1, acknowledged: true }
        : {
          protocol_version: 1, session_id: args[1],
          identity: { environment_key: 'native', agent: host, session_id: args[1] },
          session_scope: 'main', freshness_evaluated: false, state: 'ready',
          snapshot: [{ id: 'fixture', check_id: 'fixture', name: 'Internal delivery',
            message: 'Internal CLI finding.', status: 'failure', assessed_at_epoch: 1,
            checking: false, eligibility: 'ready' }],
          updates: [{ id: 'fixture', kind: 'finding', name: 'Internal delivery', message: 'Internal CLI finding.' }],
          receipt: 'a'.repeat(64),
        }))
    `,
      )
      const routes = [
        ['claude', 'claude-code'],
        ['codex', 'codex'],
        ['cursor', 'cursor'],
        ['opencode', 'opencode'],
        ['pi', 'pi'],
      ]
      for (const [vendor] of routes) {
        await writeFile(
          join(payload, vendor, 'lib/connection.json'),
          JSON.stringify({
            schema: 1,
            executable,
            deliveryProtocols: ['delivery-v1'],
          }),
        )
      }
      const driver = join(root, 'driver.mjs')
      await writeFile(
        driver,
        `
      import assert from 'node:assert/strict'
      import { pathToFileURL } from 'node:url'
      import { join } from 'node:path'
      for (const [vendor, host] of ${JSON.stringify(routes)}) {
        const root = join(${JSON.stringify(payload)}, vendor)
        const sessionId = 'internal-' + vendor
        if (vendor === 'opencode') {
          const { Antiburn } = await import(pathToFileURL(join(root, 'runtime/antiburn.mjs')))
          const plugin = await Antiburn({ client: {}, serverUrl: new URL('http://localhost') })
          const output = { messages: [{ info: { id: 'message', sessionID: sessionId, role: 'user', agent: 'build' }, parts: [{ type: 'text', text: 'fixture' }] }] }
          await plugin['experimental.chat.messages.transform']({}, output)
          assert.match(output.messages[0].parts.at(-1).text, /Internal CLI finding/)
        } else if (vendor === 'pi') {
          const { default: register } = await import(pathToFileURL(join(root, 'extensions/session-checks.mjs')))
          let handler
          register({ on: (name, callback) => { assert.equal(name, 'before_agent_start'); handler = callback } })
          const result = await handler({ messageType: 'user' }, { sessionManager: { getSessionId: () => sessionId } })
          assert.match(result?.message.content ?? '', /Internal CLI finding/)
        } else {
          const adapter = await import(pathToFileURL(join(root, 'hooks/session-check-hook.mjs')))
          const input = { session_id: sessionId, hook_event_name: 'UserPromptSubmit', conversation_id: sessionId, generation_id: 'generation' }
          const result = await adapter[vendor + 'SessionCheckOutput'](input)
          assert.match(JSON.stringify(result), /Internal CLI finding/)
          const { handoffJsonHook } = await import(pathToFileURL(join(root, 'lib/native-hook.mjs')))
          await handoffJsonHook(result, (_text, done) => done())
        }
      }
    `,
      )
      for (const override of [executable, join(root, 'unrelated-override')]) {
        await writeFile(join(root, 'calls.jsonl'), '')
        await promisify(execFile)(process.execPath, [driver], {
          env: { HOME: root, PATH: '', ANTIBURN_EXECUTABLE: override },
          timeout: 30000,
        })
        const calls = (await readFile(join(root, 'calls.jsonl'), 'utf8'))
          .trim()
          .split('\n')
          .map((line) => JSON.parse(line))
        assert.equal(calls.length, 10)
        for (const [vendor, host] of routes) {
          const deliveries = calls.filter((args) => args[4] === `${host}:context`)
          assert.equal(deliveries.length, 2)
          assert.deepEqual(deliveries[0], [
            'checks',
            `internal-${vendor}`,
            '--delivery-v1',
            '--consumer',
            `${host}:context`,
          ])
          assert.deepEqual(deliveries[1], [...deliveries[0], '--ack', 'a'.repeat(64)])
        }
      }
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  },
)

test('managed resolution fails closed and never falls back to PATH or an override', async () => {
  const root = await mkdtemp(join(tmpdir(), 'antiburn-connection-'))
  try {
    const connectionUrl = pathToFileURL(join(root, 'connection.json'))
    assert.deepEqual(managedExecutable({ managed: true, connectionUrl }), { managed: true })
    await writeFile(
      connectionUrl,
      JSON.stringify({
        schema: 1,
        executable: process.execPath,
        deliveryProtocols: ['delivery-v1'],
      }),
    )
    assert.equal(
      managedExecutable({ managed: true, connectionUrl }).executable,
      process.execPath,
    )
    for (const connection of [
      { schema: 2, executable: process.execPath, deliveryProtocols: ['delivery-v1'] },
      { schema: 1, executable: 'relative', deliveryProtocols: ['delivery-v1'] },
      { schema: 1, executable: process.execPath, deliveryProtocols: [] },
    ]) {
      await writeFile(connectionUrl, JSON.stringify(connection))
      assert.deepEqual(managedExecutable({ managed: true, connectionUrl }), { managed: true })
    }
    assert.deepEqual(managedExecutable({ managed: false, connectionUrl }), { managed: false })
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test('desktop payload contains only complete runtime adapters, without public Claude payload or help skills', async () => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-desktop-payload-')))
  try {
    const one = await assembleAgentIntegrations(join(root, 'one'))
    const two = await assembleAgentIntegrations(join(root, 'two'))
    assert.deepEqual(one, two)
    assert.equal(one.files.length, 44)
    assert.ok(
      one.files.every(
        ({ path }) => !/candidate|debug|\.ts$|SKILL|marketplace|commands\//.test(path),
      ),
    )
    for (const host of ['claude', 'codex', 'cursor', 'opencode', 'pi']) {
      const source = await readFile(
        join(root, 'one', host, 'lib/managed-connection.mjs'),
        'utf8',
      )
      assert.ok(source.includes('const MANAGED_BY_DESKTOP = true'))
      await import(pathToFileURL(join(root, 'one', host, 'lib/session-checks.mjs')).href)
    }
    const pi = JSON.parse(await readFile(join(root, 'one/pi/package.json'), 'utf8'))
    const adapter = await import(pathToFileURL(join(root, 'one/pi', pi.pi.extensions[0])).href)
    const registrations = []
    adapter.default({
      on: (event) => registrations.push(event),
      registerCommand: () => assert.fail('optional command installed'),
    })
    assert.deepEqual(registrations, ['before_agent_start'])
    await assert.rejects(assembleAgentIntegrations(join(root, 'one')))
    await mkdir(join(root, 'unrelated'))
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})

test('desktop assembly rejects a missing or repeated managed marker before publishing', async () => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'antiburn-managed-marker-')))
  try {
    await mkdir(join(root, 'scripts'))
    await mkdir(join(root, 'packages/antiburn-plugin-core'), { recursive: true })
    for (const name of ['assemble-agent-integrations.mjs', 'artifact-files.mjs'])
      await writeFile(
        join(root, 'scripts', name),
        await readFile(new URL(`../../../scripts/${name}`, import.meta.url)),
      )
    for (const name of [
      'core.mjs',
      'discovery.mjs',
      'probe.mjs',
      'session-checks.mjs',
      'session-checks-validation.mjs',
      'native-hook.mjs',
    ])
      await writeFile(join(root, 'packages/antiburn-plugin-core', name), 'export {}')
    const { assembleAgentIntegrations: assemble } = await import(
      pathToFileURL(join(root, 'scripts/assemble-agent-integrations.mjs'))
    )
    const marker = 'const MANAGED_BY_DESKTOP = false'
    for (const source of ['const MANAGED_BY_DESKTOP=false', `${marker}\n${marker}`]) {
      await writeFile(
        join(root, 'packages/antiburn-plugin-core/managed-connection.mjs'),
        source,
      )
      const output = join(root, 'payload')
      await assert.rejects(assemble(output), /Expected exactly one desktop managed marker/)
      await assert.rejects(readFile(join(output, 'manifest.json')), { code: 'ENOENT' })
    }
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
