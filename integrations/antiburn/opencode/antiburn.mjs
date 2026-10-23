import { renderNotice, sessionOnce } from '../../../packages/antiburn-plugin-core/core.mjs'
import { discover } from '../../../packages/antiburn-plugin-core/discovery.mjs'
import { opencodeSessionCheckMessages, opencodeSessionCheckStatus } from './session-checks.mjs'

const key = Symbol.for('antiburn.shell.opencode.sessions.v1')
const shared = /** @type {typeof globalThis & {[key: symbol]: Map<string, boolean>}} */ (
  globalThis
)
const sessions = (shared[key] ??= new Map())
const MAX_COMPACTING_SESSIONS = 256

/** @type {import('@opencode-ai/plugin').Plugin} */
export const Antiburn = async ({ client, serverUrl }) => {
  const local = ['localhost', '127.0.0.1', '[::1]'].includes(serverUrl.hostname)
  const compactingSessions = new Set()
  const handoffs = new Map()
  return {
    'experimental.chat.messages.transform': async (_input, output) => {
      if (!local) return
      await opencodeSessionCheckMessages(
        output.messages,
        output,
        compactingSessions,
        undefined,
        undefined,
        handoffs,
      )
    },
    'experimental.session.compacting': async ({ sessionID }) => {
      if (!local || !sessionID) return
      handoffs.delete(sessionID)
      compactingSessions.delete(sessionID)
      compactingSessions.add(sessionID)
      while (compactingSessions.size > MAX_COMPACTING_SESSIONS)
        compactingSessions.delete(compactingSessions.values().next().value)
    },
    event: async ({ event }) => {
      if (event.type === 'session.deleted') {
        handoffs.delete(event.properties.info.id)
        compactingSessions.delete(event.properties.info.id)
        return
      }
      if (
        process.env.ANTIBURN_DEBUG !== '1' ||
        !local ||
        event.type !== 'session.created' ||
        event.properties.info.parentID
      )
        return
      await sessionOnce(sessions, event.properties.info.id, async () => {
        const checkStatus = await opencodeSessionCheckStatus(event.properties.info.id)
        await client.tui.showToast({
          body: {
            title: 'antiburn',
            message: `${renderNotice(await discover())}\n${checkStatus}`,
            variant: 'info',
          },
        })
      })
    },
  }
}
