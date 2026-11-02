export const VERSION = '0.2.0'
export const CONTRACT_VERSION = 1
export const HOSTS = ['claude-code', 'codex', 'cursor', 'opencode', 'pi']

/** @typedef {'plain' | 'markdown' | 'ascii' | 'flame'} Style */
/** @typedef {{state: 'found' | 'not-found' | 'unavailable'}} Availability */
/** @param {string} input @returns {{command: 'help'|'status'|'usage', style: Style}} */
export function parseCommand(input = '') {
  if (typeof input !== 'string' || input.length > 512)
    return { command: 'usage', style: 'plain' }
  const [command = 'help', flag, ...rest] = input.trim().split(/\s+/).filter(Boolean)
  const style = flag === '--ascii' ? 'ascii' : flag === '--flame' ? 'flame' : 'plain'
  if (
    rest.length ||
    (flag && flag !== '--ascii' && flag !== '--flame') ||
    !['help', 'status'].includes(command)
  )
    return { command: 'usage', style }
  return { command: /** @type {'help'|'status'} */ (command), style }
}

/** @param {string} host @param {string} invocation @param {'native'|'model-mediated'} mode @param {string} surface @param {string} [hostVersion] */
export function describe(host, invocation, mode, surface = 'unknown', hostVersion = 'unknown') {
  return {
    id: 'antiburn',
    version: VERSION,
    contractVersion: CONTRACT_VERSION,
    host: {
      id: HOSTS.includes(host) ? host : 'unknown',
      version: clean(hostVersion),
      surface: clean(surface),
    },
    capabilities: {
      invocationMode: mode,
      invocation: clean(invocation),
      output: clean(surface),
      composition: host === 'claude-code',
    },
    certification: 'unverified',
  }
}

/** @param {string} text */
function clean(text) {
  return String(text)
    .replace(/[\x00-\x1f\x7f-\x9f]/g, '')
    .slice(0, 128)
}

/** @param {{nativeColor?: boolean, markdown?: boolean, unicode?: boolean}} capability @returns {Style} */
export function presentation(capability) {
  if (capability.unicode === false) return 'ascii'
  return capability.markdown ? 'markdown' : 'plain'
}

/** @param {Style} style */
function brand(style) {
  if (style === 'ascii') return '`antiburn`'
  return '🔥 `antiburn`'
}

/** @param {Availability} result @param {Style} [style] */
export function renderNotice(result, style = 'plain') {
  const state =
    result.state === 'found'
      ? 'found'
      : result.state === 'not-found'
        ? 'not found'
        : 'check unavailable'
  return `${brand(style)}${style === 'ascii' ? ' ' : ' · '}CLI ${state}`
}

/** @param {ReturnType<typeof describe>} meta @param {Availability} result @param {Style} [style] */
export function renderStatus(meta, result, style = 'plain') {
  const state =
    result.state === 'found'
      ? 'Found'
      : result.state === 'not-found'
        ? 'Not found'
        : 'Check unavailable'
  return [
    brand(style),
    `CLI: ${state}`,
    'App connection: Not checked',
    `Shell: ${meta.version} (contract ${meta.contractVersion})`,
    `Host: ${meta.host.id} ${meta.host.version}`,
    `Surface: ${meta.host.surface}`,
    `Invocation: ${meta.capabilities.invocation} (${meta.capabilities.invocationMode})`,
    'Runtime certification: Unverified',
  ].join('\n')
}

/** @param {ReturnType<typeof describe>} meta @param {Style} [style] @param {boolean} [usage] */
export function renderHelp(meta, style = 'plain', usage = false) {
  return [
    brand(style),
    ...(usage ? ['Unknown arguments.'] : []),
    `Usage: ${meta.capabilities.invocation} [help|status] [--ascii|--flame]`,
    'Checks executable availability without launching antiburn.',
    'App connection: Not checked',
    `Shell: ${VERSION} (contract ${CONTRACT_VERSION})`,
  ].join('\n')
}

/** @param {Map<string, boolean>} state @param {string} id @param {() => Promise<unknown>} work */
export async function sessionOnce(state, id, work) {
  if (!id || state.has(id)) return false
  if (state.size >= 128) state.delete(/** @type {string} */ (state.keys().next().value))
  state.set(id, true)
  try {
    await work()
  } catch {
    /* Keep the attempt marker when the host refuses output. */
  }
  return true
}
