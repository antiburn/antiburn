import assert from 'node:assert/strict'
import { mkdtemp, mkdir, writeFile, chmod, symlink, rm, access } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'
import {
  parseCommand,
  describe,
  renderNotice,
  renderStatus,
  renderHelp,
  presentation,
  sessionOnce,
} from '../core.mjs'
import { discover } from '../discovery.mjs'

test('bounded parser accepts only help/status and explicit presentation overrides', () => {
  assert.deepEqual(parseCommand(''), { command: 'help', style: 'plain' })
  assert.deepEqual(parseCommand('status --ascii'), { command: 'status', style: 'ascii' })
  assert.equal(parseCommand('help --flame').style, 'flame')
  for (const input of [
    'status; echo bad',
    '--version',
    'status extra',
    'help --ascii --flame',
    'a'.repeat(513),
  ])
    assert.equal(parseCommand(input).command, 'usage')
})

test('presentation is truthful, bounded, and contains no terminal controls', () => {
  const meta = describe('pi', '/antiburn', 'native', 'tui')
  assert.equal(meta.host.version, 'unknown')
  assert.equal(meta.certification, 'unverified')
  for (const state of ['found', 'not-found', 'unavailable']) {
    const text = renderStatus(meta, { state }, 'plain')
    assert.match(text, /App connection: Not checked/)
    assert.doesNotMatch(text, /Connected|\x1b|\/(Users|home)\//)
    assert.ok(text.length < 1024)
  }
  assert.equal(renderNotice({ state: 'found' }), '🔥 `antiburn` · CLI found')
  assert.equal(
    renderNotice({ state: 'unavailable' }, 'ascii'),
    '`antiburn` CLI check unavailable',
  )
  assert.match(renderNotice({ state: 'not-found' }, 'markdown'), /🔥 `antiburn`/)
  assert.equal(renderNotice({ state: 'found' }, 'flame'), renderNotice({ state: 'found' }))
  assert.doesNotMatch(renderHelp(meta), /♨/)
  assert.equal(presentation({ nativeColor: true, unicode: true }), 'plain')
  assert.equal(presentation({ markdown: true, unicode: true }), 'markdown')
  assert.equal(presentation({ unicode: false }), 'ascii')
})

test('real executable discovery follows links but never executes its candidate', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'antiburn-core-plugin-test-'))
  try {
    const bin = join(dir, 'bin with spaces;$(never)')
    await mkdir(bin)
    const marker = join(dir, 'executed')
    const exe = join(bin, 'antiburn')
    await writeFile(exe, `#!/bin/sh\ntouch '${marker}'\n`)
    await chmod(exe, 0o600)
    assert.equal((await discover({ env: { PATH: bin } })).state, 'not-found')
    await chmod(exe, 0o700)
    assert.equal((await discover({ env: { PATH: bin } })).state, 'found')
    const link = join(dir, 'linked')
    await symlink(exe, link)
    assert.equal((await discover({ env: { ANTIBURN_EXECUTABLE: link } })).state, 'found')
    await assert.rejects(access(marker))
    assert.equal((await discover({ env: { ANTIBURN_EXECUTABLE: dir } })).state, 'not-found')
  } finally {
    await rm(dir, { recursive: true, force: true })
  }
})

test('discovery preserves uncertainty and bounds search', async () => {
  assert.equal((await discover({ env: {} })).state, 'unavailable')
  assert.equal((await discover({ env: { PATH: '' } })).state, 'unavailable')
  assert.equal(
    (await discover({ env: { PATH: '/missing' }, probe: async () => false })).state,
    'not-found',
  )
  assert.equal(
    (
      await discover({
        env: { PATH: '/denied' },
        probe: async () => {
          throw new Error('private data')
        },
      })
    ).state,
    'unavailable',
  )
  assert.equal(
    (
      await discover({
        env: { PATH: '/denied:/good' },
        probe: async (p) => {
          if (p.startsWith('/denied')) throw new Error()
          return true
        },
      })
    ).state,
    'found',
  )
  assert.equal(
    (await discover({ env: { PATH: Array(130).fill('/a').join(':') } })).state,
    'unavailable',
  )
  assert.equal(
    (await discover({ env: { ANTIBURN_EXECUTABLE: 'relative' } })).state,
    'unavailable',
  )
  assert.equal((await discover({ env: { PATH: '//remote/share' } })).state, 'unavailable')
  assert.equal(
    (await discover({ env: { PATH: '/a' }, timeoutMs: 5, probe: () => new Promise(() => {}) }))
      .state,
    'unavailable',
  )
})

test('Windows searches PATHEXT as files without executing candidates', async () => {
  const paths = []
  const result = await discover({
    platform: 'win32',
    cwd: 'C:\\work',
    env: { PATH: 'C:\\bin', PATHEXT: '.COM;.EXE;.BAT;.CMD' },
    probe: async (path) => {
      paths.push(path)
      return path.endsWith('.EXE')
    },
  })
  assert.equal(result.state, 'found')
  assert.deepEqual(paths, ['C:\\bin\\antiburn-cli.COM', 'C:\\bin\\antiburn-cli.EXE'])
  assert.equal(
    (await discover({ platform: 'win32', env: { PATH: '\\\\server\\share' } })).state,
    'unavailable',
  )
})

test('discovery accepts the runtime name and the legacy compatibility export', async () => {
  for (const name of ['antiburn-cli', 'antiburn']) {
    const expected = `/compatibility/bin/${name}`
    assert.equal(
      (
        await discover({
          platform: 'darwin',
          env: { PATH: '/compatibility/bin' },
          probe: async (candidate) => candidate === expected,
        })
      ).state,
      'found',
    )
  }
})

test('session guard reserves before async work and persists across reload owners', async () => {
  const state = new Map()
  let count = 0
  const run = () =>
    sessionOnce(state, 'session-a', async () => {
      count++
      await Promise.resolve()
    })
  await Promise.all([run(), run()])
  await sessionOnce(state, 'session-a', async () => count++)
  await sessionOnce(state, 'session-b', async () => count++)
  assert.equal(count, 2)
  await sessionOnce(state, 'failure', async () => {
    throw new Error('denied')
  })
  assert.equal(await sessionOnce(state, 'failure', async () => count++), false)
  for (let i = 0; i < 140; i++) await sessionOnce(state, `session-${i}`, async () => {})
  assert.ok(state.size <= 128)
})
