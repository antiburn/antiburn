export const VERSION = '0.2.1'
export const CONTRACT_VERSION = 1
export const HOSTS = ['claude-code', 'codex', 'cursor', 'opencode', 'pi']

/** @typedef {'plain' | 'ascii' | 'flame'} Style */
/** @typedef {'help'|'status'|'diagnostics'|'usage'|'details'|'refresh'|'invalid'} Command */

/** @param {string} input @returns {{command: Command, style: Style}} */
export function parseCommand(input = '') {
  if (typeof input !== 'string' || input.length > 512)
    return { command: 'invalid', style: 'plain' }

  const parts = input.trim().split(/\s+/).filter(Boolean)
  const [command = 'help', arg] = parts
  const style = arg === '--ascii' ? 'ascii' : arg === '--flame' ? 'flame' : 'plain'
  if (parts.length > 2) return { command: 'invalid', style }
  if (command === 'help' || command === 'status') {
    if (arg && arg !== '--ascii' && arg !== '--flame') return { command: 'invalid', style }
    return { command, style }
  }
  if (arg) {
    if (command === 'usage' && ['details', 'refresh'].includes(arg))
      return { command: /** @type {'details'|'refresh'} */ (arg), style }
    return { command: 'invalid', style }
  }
  if (['diagnostics', 'usage', 'details', 'refresh'].includes(command))
    return { command: /** @type {Command} */ (command), style }
  return { command: 'invalid', style }
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
      usage: host === 'claude-code',
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

/** @param {{markdown?: boolean, unicode?: boolean}} capability @returns {Style} */
export function presentation(capability) {
  return capability.unicode === false ? 'ascii' : 'plain'
}

/** @param {Style} style */
function brand(style) {
  if (style === 'ascii') return '`antiburn`'
  return '🔥 `antiburn`'
}

/** @param {ReturnType<typeof describe>} meta @param {Style} [style] @param {boolean} [usage] */
export function renderHelp(meta, style = 'plain', usage = false) {
  return [
    brand(style),
    ...(usage ? ['Unknown arguments.'] : []),
    `Usage: ${meta.capabilities.invocation} [help|status|diagnostics|usage [details|refresh]]`,
    'Help and status accept --ascii or --flame.',
    'Aliases: details and refresh show or refresh usage.',
    'Shows antiburn feature and usage information.',
    `antiburn ${VERSION} (contract ${CONTRACT_VERSION})`,
  ].join('\n')
}

/** @param {ReturnType<typeof describe>} meta @param {Style} [style] */
export function renderStatus(meta, style = 'plain') {
  return [
    brand(style),
    `antiburn: ${meta.version} (contract ${meta.contractVersion})`,
    `Usage feature: ${meta.capabilities.usage ? 'Included' : 'Not included'}`,
    `Host: ${meta.host.id} ${meta.host.version}`,
    `Surface: ${meta.host.surface}`,
    `Invocation: ${meta.capabilities.invocation} (${meta.capabilities.invocationMode})`,
    'Runtime certification: Unverified',
  ].join('\n')
}

/** @param {string} input @returns {'details'|'refresh'|'help'|'invalid'} */
export function parseUsageCommand(input = '') {
  if (typeof input !== 'string' || input.length > 512) return 'invalid'
  const args = input.trim().split(/\s+/).filter(Boolean).join(' ')
  if (['', 'usage', 'details', 'usage details'].includes(args)) return 'details'
  if (['refresh', 'usage refresh'].includes(args)) return 'refresh'
  return args === 'help' ? 'help' : 'invalid'
}

/** @param {string} invocation @param {boolean} [invalid] */
export function renderUsageHelp(invocation, invalid = false) {
  return [
    ...(invalid ? ['Unknown arguments.'] : []),
    `Run \`${invocation}\` to see your usage.`,
    `Run \`${invocation} refresh\` to update it.`,
  ].join('\n')
}
