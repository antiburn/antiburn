import assert from 'node:assert/strict'
import { test } from 'node:test'
import { mkdtemp, writeFile, symlink, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { spawnSync } from 'node:child_process'
import { pathToFileURL } from 'node:url'
import { handoffJsonHook, withSessionCheckReceipt } from '../native-hook.mjs'

test('acknowledges JSON hook receipts only after stdout handoff succeeds', async () => {
  const order = []
  const output = withSessionCheckReceipt(
    { additional_context: 'Finding' },
    {
      host: 'codex',
      sessionId: 'session-1',
      consumer: 'context',
      receipt: 'e'.repeat(64),
    },
  )
  await handoffJsonHook(
    output,
    (text, callback) => {
      assert.deepEqual(JSON.parse(text), { additional_context: 'Finding' })
      order.push('write')
      setTimeout(() => callback(), 0)
    },
    async (options) => {
      order.push('ack')
      assert.equal(options.receipt, 'e'.repeat(64))
      return true
    },
  )
  assert.deepEqual(order, ['write', 'ack'])
})

test('does not acknowledge when native stdout handoff fails', async () => {
  let acknowledged = false
  const output = withSessionCheckReceipt(
    { additional_context: 'Finding' },
    {
      host: 'cursor',
      sessionId: 'session-2',
      consumer: 'context',
      receipt: 'f'.repeat(64),
    },
  )
  await assert.rejects(
    handoffJsonHook(
      output,
      (_text, callback) => callback(new Error('pipe closed')),
      async () => {
        acknowledged = true
        return true
      },
    ),
    /pipe closed/,
  )
  assert.equal(acknowledged, false)
})

test('recognizes a directly executed entry through a symlink', async () => {
  const directory = await mkdtemp(join(tmpdir(), 'antiburn-direct-entry-'))
  try {
    const entry = join(directory, 'entry.mjs')
    const alias = join(directory, 'alias.mjs')
    const utility = new URL('../native-hook.mjs', import.meta.url).href
    await writeFile(
      entry,
      `import { isDirectExecution } from ${JSON.stringify(utility)}; console.log(isDirectExecution(import.meta.filename));`,
    )
    await symlink(entry, alias)
    const direct = spawnSync(process.execPath, [alias], { encoding: 'utf8' })
    assert.equal(direct.status, 0, direct.stderr)
    assert.equal(direct.stdout.trim(), 'true')
    const imported = spawnSync(
      process.execPath,
      ['--input-type=module', '-e', `import ${JSON.stringify(pathToFileURL(alias).href)}`],
      { encoding: 'utf8' },
    )
    assert.equal(imported.status, 0, imported.stderr)
    assert.equal(imported.stdout.trim(), 'false')
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})
