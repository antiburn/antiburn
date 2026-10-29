import {
  describe,
  parseCommand,
  renderHelp,
  renderNotice,
  renderStatus,
  sessionOnce,
} from '../../../packages/antiburn-plugin-core/core.mjs'
import { discover } from '../../../packages/antiburn-plugin-core/discovery.mjs'
import {
  acknowledgeAfterHandoff,
  formatSessionCheckStatus,
  readSessionChecks,
} from '../../../packages/antiburn-plugin-core/session-checks.mjs'

const key = Symbol.for('antiburn.shell.sessions.v1')
const shared = /** @type {typeof globalThis & {[key: symbol]: Map<string, boolean>}} */ (
  globalThis
)
const sessions = (shared[key] ??= new Map())

/** @param {string} messageType @param {string} sessionId @param {typeof readSessionChecks} read @param {typeof acknowledgeAfterHandoff} acknowledge */
export async function piSessionCheckMessage(
  messageType,
  sessionId,
  read = readSessionChecks,
  acknowledge = acknowledgeAfterHandoff,
) {
  if (messageType !== 'user') return undefined
  const result = await read({ host: 'pi', sessionId, consumer: 'context' })
  if (!result) return undefined
  if (result.receipt)
    acknowledge({ host: 'pi', sessionId, consumer: 'context', receipt: result.receipt })
  return {
    customType: 'antiburn-session-checks',
    content: result.message,
    display: false,
  }
}

/** @param {string} sessionId @param {typeof readSessionChecks} read */
export async function piSessionCheckStatus(sessionId, read = readSessionChecks) {
  const result = await read({ host: 'pi', sessionId, consumer: 'status' })
  return result
    ? formatSessionCheckStatus(result)
    : 'Antiburn session checks: unavailable for this session or CLI version.'
}

/** @param {import('@earendil-works/pi-coding-agent').ExtensionAPI} pi */
export default function antiburn(pi) {
  pi.registerCommand('antiburn', {
    description: 'antiburn help and executable availability',
    handler: async (args, ctx) => {
      if (!ctx.hasUI) return
      const parsed = parseCommand(args)
      const meta = describe('pi', '/antiburn', 'native', ctx.mode)
      const sessionId = ctx.sessionManager.getSessionId()
      if (parsed.command === 'status') {
        await sessionOnce(sessions, sessionId, async () => {})
        const discovery = renderStatus(meta, await discover(), parsed.style)
        const checks = await piSessionCheckStatus(sessionId)
        ctx.ui.notify(`${discovery}\n${checks}`, 'info')
        return
      }
      ctx.ui.notify(renderHelp(meta, parsed.style, parsed.command === 'usage'), 'info')
    },
  })
  pi.on('session_start', async (_event, ctx) => {
    if (process.env.ANTIBURN_DEBUG !== '1' || ctx.mode !== 'tui' || !ctx.hasUI) return
    await sessionOnce(sessions, ctx.sessionManager.getSessionId(), async () => {
      ctx.ui.notify(renderNotice(await discover()), 'info')
    })
  })
  pi.on('before_agent_start', async (event, ctx) => {
    const message = await piSessionCheckMessage(
      event.messageType,
      ctx.sessionManager.getSessionId(),
    )
    return message ? { message } : undefined
  })
}
