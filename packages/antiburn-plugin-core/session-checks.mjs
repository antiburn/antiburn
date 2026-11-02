import { spawn } from 'node:child_process'
import { accessSync, constants, statSync } from 'node:fs'
import { delimiter, isAbsolute, resolve, win32 } from 'node:path'
import { managedExecutable } from './managed-connection.mjs'
import {
  formatSessionCheckUpdates,
  formatSessionChecksContext,
  validateSessionChecksDelivery,
} from './session-checks-validation.mjs'

const MAX_OUTPUT_BYTES = 256 * 1024
const MAX_SESSION_ID_LENGTH = 512
const DEFAULT_TIMEOUT_MS = 1200
const pendingBySession = new Map()

/** @typedef {'claude-code'|'codex'|'cursor'|'pi'|'opencode'} SessionCheckHost */

/** @param {string} value */
function safeSessionId(value) {
  return (
    value.length > 0 && value.length <= MAX_SESSION_ID_LENGTH && !/[\x00-\x1f\x7f]/.test(value)
  )
}

/**
 * Resolve the CLI from an explicit absolute override or PATH without invoking a shell.
 * @param {NodeJS.ProcessEnv} env
 * @param {string} platform
 */
function executable(env, platform) {
  const managed = managedExecutable({ platform })
  if (managed.managed) return managed.executable
  const pathApi = platform === 'win32' ? win32 : { isAbsolute, resolve }
  const override = env.ANTIBURN_EXECUTABLE
  if (override !== undefined) {
    if (
      !override ||
      override.length > 8192 ||
      /[\x00-\x1f\x7f]/.test(override) ||
      !pathApi.isAbsolute(override)
    )
      return undefined
    try {
      if (!statSync(override).isFile()) return undefined
      accessSync(override, platform === 'win32' ? constants.F_OK : constants.X_OK)
      return override
    } catch {
      return undefined
    }
  }
  const search = env.PATH
  if (!search || search.length > 32768) return undefined
  const names =
    platform === 'win32'
      ? ['antiburn-cli.exe', 'antiburn-cli.com', 'antiburn.exe', 'antiburn.com']
      : ['antiburn-cli', 'antiburn']
  const dirs = search.split(platform === 'win32' ? ';' : delimiter)
  if (dirs.length > 128 || dirs.some((dir) => /[\x00-\x1f\x7f]/.test(dir))) return undefined
  for (const dir of dirs) {
    for (const name of names) {
      const candidate = pathApi.resolve(dir || '.', name)
      try {
        if (statSync(candidate).isFile()) {
          accessSync(candidate, platform === 'win32' ? constants.F_OK : constants.X_OK)
          return candidate
        }
      } catch {
        continue
      }
    }
  }
  return undefined
}

/** @typedef {{protocol_version:number,session_id:string,identity:{environment_key:string,agent:string,session_id:string},session_scope:'main'|'unsupported',freshness_evaluated:boolean,state:string,snapshot:Array<{id:string,check_id:string,name:string,message:string|null,status:'failure'|'success'|null,assessed_at_epoch:number|null,checking:boolean,eligibility:string}>,updates:Array<{id:string,kind:'finding'|'cleared'|'invalidated',name:string,message:string|null}>,receipt:string|null}} DeliverySnapshot */

/** @param {string} sessionId @param {() => Promise<T>} run @returns {Promise<T>} @template T */
function serialize(sessionId, run) {
  const previous = pendingBySession.get(sessionId) ?? Promise.resolve()
  const current = previous.catch(() => {}).then(run)
  pendingBySession.set(sessionId, current)
  return current.finally(() => {
    if (pendingBySession.get(sessionId) === current) pendingBySession.delete(sessionId)
  })
}

/**
 * Read staged findings for a native session boundary. The caller acknowledges only
 * after the host accepts the returned context or the UI applies the snapshot.
 * @param {{host:SessionCheckHost,sessionId:string,consumer?:'context'|'ui'|'status',agentId?:string,env?:NodeJS.ProcessEnv,platform?:string,timeoutMs?:number,maxOutputBytes?:number,run?: (args:string[], options:{timeoutMs:number,maxOutputBytes:number})=>Promise<string>}} options
 */
export async function readSessionChecks(options) {
  const sessionId = options.sessionId
  if (options.agentId || !safeSessionId(sessionId)) return undefined
  return serialize(sessionId, async () => {
    const env = options.env ?? process.env
    const platform = options.platform ?? process.platform
    const cli = options.run ? 'antiburn' : executable(env, platform)
    if (!cli) return undefined
    const run = options.run ?? ((args, limits) => runCli(cli, args, limits, env))
    const consumer = options.consumer ?? 'context'
    if (!['context', 'ui', 'status'].includes(consumer)) return undefined
    const timeoutMs = Math.max(100, Math.min(3000, options.timeoutMs ?? DEFAULT_TIMEOUT_MS))
    const maxOutputBytes = Math.max(
      1024,
      Math.min(MAX_OUTPUT_BYTES, options.maxOutputBytes ?? MAX_OUTPUT_BYTES),
    )
    let raw
    try {
      raw = await run(
        ['checks', sessionId, '--delivery-v1', '--consumer', `${options.host}:${consumer}`],
        {
          timeoutMs,
          maxOutputBytes,
        },
      )
    } catch {
      return undefined
    }
    let result
    try {
      result = JSON.parse(raw)
    } catch {
      return undefined
    }
    result = validateSessionChecksDelivery(result, { sessionId, host: options.host })
    if (!result) return undefined
    const updates = result.updates
    if (consumer === 'context' && (!updates.length || !result.receipt)) return undefined
    const message =
      consumer === 'context'
        ? formatSessionChecksContext(result)
        : formatSessionCheckUpdates(updates)
    if (consumer === 'context' && !message.trim()) return undefined
    const delivery = {
      message,
      updates,
      snapshot: result.snapshot,
      state: result.state,
      freshnessEvaluated: result.freshness_evaluated,
      sessionScope: result.session_scope,
      receipt: result.receipt,
    }
    return delivery
  })
}

/** @param {{state:string,snapshot:Array<{status:'failure'|'success'|null,checking:boolean}>,freshnessEvaluated:boolean,sessionScope:'main'|'unsupported'}} result */
export function formatSessionCheckStatus(result) {
  const failures = result.snapshot.filter((check) => check.status === 'failure').length
  const successes = result.snapshot.filter((check) => check.status === 'success').length
  const waiting = result.snapshot.filter(
    (check) => check.checking || check.status === null,
  ).length
  const summary = `Antiburn session checks: connected (protocol v1); scope ${result.sessionScope}; state ${result.state}; ${result.snapshot.length} checks (${failures} failed, ${successes} passed, ${waiting} waiting).`
  return result.freshnessEvaluated ? summary : `${summary} Freshness has not been evaluated.`
}

/** @param {{host:SessionCheckHost,sessionId:string,consumer?:'context'|'ui',receipt:string,env?:NodeJS.ProcessEnv,platform?:string,timeoutMs?:number,maxOutputBytes?:number,run?: (args:string[], options:{timeoutMs:number,maxOutputBytes:number})=>Promise<string>}} options */
async function acknowledgeReceipt(options) {
  if (!safeSessionId(options.sessionId) || !options.receipt || options.receipt.length > 4096)
    return false
  const env = options.env ?? process.env
  const platform = options.platform ?? process.platform
  const cli = options.run ? 'antiburn' : executable(env, platform)
  if (!cli) return false
  const run = options.run ?? ((args, limits) => runCli(cli, args, limits, env))
  const timeoutMs = Math.max(100, Math.min(3000, options.timeoutMs ?? DEFAULT_TIMEOUT_MS))
  const maxOutputBytes = Math.max(
    1024,
    Math.min(MAX_OUTPUT_BYTES, options.maxOutputBytes ?? MAX_OUTPUT_BYTES),
  )
  try {
    const consumer = options.consumer ?? 'context'
    const ackRaw = await run(
      [
        'checks',
        options.sessionId,
        '--delivery-v1',
        '--consumer',
        `${options.host}:${consumer}`,
        '--ack',
        options.receipt,
      ],
      {
        timeoutMs,
        maxOutputBytes,
      },
    )
    const ack = JSON.parse(ackRaw)
    return ack.protocol_version === 1 && ack.acknowledged === true
  } catch {
    return false
  }
}

/** @param {{host:SessionCheckHost,sessionId:string,consumer?:'context'|'ui',receipt:string,env?:NodeJS.ProcessEnv,platform?:string,timeoutMs?:number,maxOutputBytes?:number,run?: (args:string[], options:{timeoutMs:number,maxOutputBytes:number})=>Promise<string>}} options */
export async function acknowledgeSessionChecks(options) {
  return serialize(options.sessionId, () => acknowledgeReceipt(options))
}

/** Schedule an acknowledgement after the adapter has constructed the host handoff result. */
export function acknowledgeAfterHandoff(options) {
  setTimeout(() => void acknowledgeSessionChecks(options), 0)
}

/** @param {string} cli @param {string[]} args @param {{timeoutMs:number,maxOutputBytes:number}} limits @param {NodeJS.ProcessEnv} env */
function runCli(cli, args, limits, env) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(cli, args, {
      shell: false,
      stdio: ['ignore', 'pipe', 'ignore'],
      windowsHide: true,
      env: { ...env, ANTIBURN_EXECUTABLE: cli },
    })
    let output = ''
    let size = 0
    const timer = setTimeout(() => {
      child.kill()
      reject(new Error('CLI request timed out'))
    }, limits.timeoutMs)
    child.stdout.on('data', (chunk) => {
      size += chunk.length
      if (size > limits.maxOutputBytes) {
        child.kill()
        reject(new Error('CLI output limit exceeded'))
      } else output += chunk.toString('utf8')
    })
    child.once('error', (error) => {
      clearTimeout(timer)
      reject(error)
    })
    child.once('close', (code) => {
      clearTimeout(timer)
      if (code === 0) resolvePromise(output)
      else reject(new Error('CLI request failed'))
    })
  })
}
