import {
  formatSessionCheckStatus,
  readSessionChecks,
} from '../../packages/antiburn-plugin-core/session-checks.mjs'
import { isDirectExecution } from '../../packages/antiburn-plugin-core/native-hook.mjs'

const hosts = new Set(['codex', 'cursor'])

/** @param {string[]} args @param {typeof readSessionChecks} read */
export async function renderStatusCommand(args, read = readSessionChecks) {
  if (args.length !== 4 || args[0] !== '--host' || args[2] !== '--session') return undefined
  const host = args[1]
  const sessionId = args[3]
  if (
    !hosts.has(host) ||
    !sessionId ||
    sessionId.length > 512 ||
    /[\x00-\x1f\x7f]/.test(sessionId)
  )
    return undefined
  const result = await read({
    host: /** @type {'codex'|'cursor'} */ (host),
    sessionId,
    consumer: 'status',
  })
  return result
    ? formatSessionCheckStatus(result)
    : 'Antiburn session checks: unavailable for this session or CLI version.'
}

if (isDirectExecution(import.meta.filename)) {
  const status = await renderStatusCommand(process.argv.slice(2))
  if (!status) {
    process.stderr.write(
      'Usage: node session-check-status.mjs --host codex|cursor --session SESSION_ID\n',
    )
    process.exitCode = 2
  } else {
    process.stdout.write(`${status}\n`)
  }
}
