import { piSessionCheckMessage } from './antiburn.mjs'

/** @param {import('@earendil-works/pi-coding-agent').ExtensionAPI} pi */
export default async function sessionChecks(pi) {
  pi.on('before_agent_start', async (event, ctx) => {
    const message = await piSessionCheckMessage(
      'messageType' in event && typeof event.messageType === 'string'
        ? event.messageType
        : 'user',
      ctx.sessionManager.getSessionId(),
      undefined,
      undefined,
      ctx.hasUI && ctx.mode === 'tui',
    )
    return message ? { message } : undefined
  })
}
