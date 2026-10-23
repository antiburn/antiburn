import { readSessionChecks } from '../../../packages/antiburn-plugin-core/session-checks.mjs'
import {
  isDirectExecution,
  runJsonHook,
  withSessionCheckReceipt,
} from '../../../packages/antiburn-plugin-core/native-hook.mjs'

/** @param {Record<string, unknown>} input @param {typeof readSessionChecks} read */
export async function codexSessionCheckOutput(input, read = readSessionChecks) {
  if (
    typeof input.session_id !== 'string' ||
    input.hook_event_name !== 'UserPromptSubmit' ||
    input.agent_id ||
    input.subagent_id ||
    input.agent_type ||
    input.subagent_type ||
    input.parent_session_id ||
    input.parent_thread_id ||
    input.parent_agent_id ||
    input.is_subagent === true ||
    input.subagent === true
  )
    return undefined
  const result = await read({ host: 'codex', sessionId: input.session_id, consumer: 'context' })
  return result?.receipt
    ? withSessionCheckReceipt(
        {
          hookSpecificOutput: {
            hookEventName: 'UserPromptSubmit',
            additionalContext: result.message,
          },
        },
        {
          host: 'codex',
          sessionId: input.session_id,
          consumer: 'context',
          receipt: result.receipt,
        },
      )
    : undefined
}

if (isDirectExecution(import.meta.filename))
  await runJsonHook((input) => codexSessionCheckOutput(input))
