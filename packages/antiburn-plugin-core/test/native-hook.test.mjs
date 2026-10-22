import assert from 'node:assert/strict'
import { test } from 'node:test'
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
