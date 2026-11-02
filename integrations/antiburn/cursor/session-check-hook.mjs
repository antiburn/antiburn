import { readSessionChecks } from '../../../packages/antiburn-plugin-core/session-checks.mjs'
import {
  isDirectExecution,
  runJsonHook,
  withSessionCheckReceipt,
} from '../../../packages/antiburn-plugin-core/native-hook.mjs'

/** @param {Record<string, unknown>} input @param {typeof readSessionChecks} read */
export async function cursorSessionCheckOutput(input, read = readSessionChecks) {
  if (typeof input.conversation_id !== 'string' || typeof input.generation_id !== 'string')
    return undefined
  const result = await read({
    host: 'cursor',
    sessionId: input.conversation_id,
    consumer: 'context',
  })
  return result?.receipt
    ? withSessionCheckReceipt(
        { additional_context: result.message },
        {
          host: 'cursor',
          sessionId: input.conversation_id,
          consumer: 'context',
          receipt: result.receipt,
        },
      )
    : undefined
}

if (isDirectExecution(import.meta.filename))
  await runJsonHook((input) => cursorSessionCheckOutput(input))
