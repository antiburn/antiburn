import { piSessionCheckMessage } from './antiburn.mjs'

/** @param {import('@earendil-works/pi-coding-agent').ExtensionAPI} pi */
export default function sessionChecks(pi) {
  pi.on('before_agent_start', async (event, ctx) => {
    const message = await piSessionCheckMessage(
      event.messageType,
      ctx.sessionManager.getSessionId(),
    )
    return message ? { message } : undefined
  })
}
