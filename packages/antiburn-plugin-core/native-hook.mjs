import { pathToFileURL } from 'node:url'
import { resolve } from 'node:path'
import { acknowledgeSessionChecks } from './session-checks.mjs'

const MAX_INPUT_BYTES = 64 * 1024
const receiptMetadata = Symbol.for('antiburn.session-check-receipt.v1')

/** @param {Record<string, unknown>} output @param {{host:string,sessionId:string,consumer:'context'|'ui',receipt:string}} metadata */
export function withSessionCheckReceipt(output, metadata) {
  Object.defineProperty(output, receiptMetadata, { value: metadata })
  return output
}

/** @param {Record<string, unknown>} output @param {(text:string,callback:(error?:Error|null)=>void)=>unknown} write @param {typeof acknowledgeSessionChecks} acknowledge */
export async function handoffJsonHook(
  output,
  write = (text, callback) => process.stdout.write(text, callback),
  acknowledge = acknowledgeSessionChecks,
) {
  const metadata = /** @type {Record<symbol, unknown>} */ (output)[receiptMetadata]
  await new Promise((resolveWrite, rejectWrite) => {
    write(JSON.stringify(output), (error) =>
      error ? rejectWrite(error) : resolveWrite(undefined),
    )
  })
  if (metadata && typeof metadata === 'object') {
    await acknowledge(
      /** @type {{host:'claude-code'|'codex'|'cursor'|'pi'|'opencode',sessionId:string,consumer:'context'|'ui',receipt:string}} */ (
        metadata
      ),
    )
  }
}

/** @param {(input:Record<string,unknown>)=>Promise<unknown>} handler */
export async function runJsonHook(handler) {
  let input = ''
  for await (const chunk of process.stdin) {
    input += chunk.toString('utf8')
    if (Buffer.byteLength(input) > MAX_INPUT_BYTES) return
  }
  let parsed
  try {
    parsed = JSON.parse(input)
  } catch {
    return
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return
  try {
    const output = await handler(/** @type {Record<string, unknown>} */ (parsed))
    if (!output) return
    await handoffJsonHook(/** @type {Record<string, unknown>} */ (output))
  } catch {}
}

/** @param {string} filename */
export function isDirectExecution(filename) {
  return (
    typeof filename === 'string' &&
    !!process.argv[1] &&
    pathToFileURL(resolve(process.argv[1])).href === pathToFileURL(resolve(filename)).href
  )
}
