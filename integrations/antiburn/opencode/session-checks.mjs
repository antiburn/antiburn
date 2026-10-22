import {
  acknowledgeAfterHandoff,
  formatSessionCheckStatus,
  readSessionChecks,
} from '../../../packages/antiburn-plugin-core/session-checks.mjs'

/** @param {string|undefined} sessionId @param {{system:string[]}} output @param {typeof readSessionChecks} read @param {(options:{host:'opencode',sessionId:string,consumer:'context',receipt:string})=>void} acknowledge */
export async function opencodeSessionCheckContext(
  sessionId,
  output,
  read = readSessionChecks,
  acknowledge = acknowledgeAfterHandoff,
) {
  if (!sessionId) return
  const result = await read({ host: 'opencode', sessionId, consumer: 'context' })
  if (result) {
    output.system.push(result.message)
    if (result.receipt)
      acknowledge({ host: 'opencode', sessionId, consumer: 'context', receipt: result.receipt })
  }
}

/** @param {string} sessionId @param {typeof readSessionChecks} read */
export async function opencodeSessionCheckStatus(sessionId, read = readSessionChecks) {
  const result = await read({ host: 'opencode', sessionId, consumer: 'status' })
  return result
    ? formatSessionCheckStatus(result)
    : 'Antiburn session checks: unavailable for this session or CLI version.'
}
