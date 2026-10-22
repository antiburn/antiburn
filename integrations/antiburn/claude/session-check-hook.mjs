import { readSessionChecks } from '../../../packages/antiburn-plugin-core/session-checks.mjs'
import {
  isDirectExecution,
  runJsonHook,
  withSessionCheckReceipt,
} from '../../../packages/antiburn-plugin-core/native-hook.mjs'

/** @param {Record<string, unknown>} input @param {typeof readSessionChecks} read */
export async function claudeSessionCheckOutput(input, read = readSessionChecks) {
  const hookEventName = input.hook_event_name
  if (
    (hookEventName !== 'UserPromptSubmit' && hookEventName !== 'PostToolUse') ||
    typeof input.session_id !== 'string' ||
    input.agent_id ||
    input.agent_type ||
    input.subagent_id ||
    input.parent_session_id ||
    input.parent_agent_id ||
    input.is_subagent === true
  )
    return undefined
  const result = await read({
    host: 'claude-code',
    sessionId: input.session_id,
    consumer: 'context',
  })
  return result?.receipt
    ? withSessionCheckReceipt(
        {
          hookSpecificOutput: {
            hookEventName,
            additionalContext: result.message,
          },
        },
        {
          host: 'claude-code',
          sessionId: input.session_id,
          consumer: 'context',
          receipt: result.receipt,
        },
      )
    : undefined
}

if (isDirectExecution(import.meta.filename))
  await runJsonHook((input) => claudeSessionCheckOutput(input))
