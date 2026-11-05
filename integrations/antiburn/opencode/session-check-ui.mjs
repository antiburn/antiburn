import {
  acknowledgeSessionChecks,
  readSessionChecks,
} from '../../../packages/antiburn-plugin-core/session-checks.mjs'
import { formatSessionCheckUpdates } from '../../../packages/antiburn-plugin-core/session-checks-validation.mjs'

/** @typedef {NonNullable<Awaited<ReturnType<typeof readSessionChecks>>>} Delivery */
/** @typedef {{sessionId:string,text:string,markdown:string,receipt:string|null,revision:number}} AdvisoryFrame */

/** @param {unknown} value @returns {value is string} */
function safeSessionId(value) {
  return (
    typeof value === 'string' &&
    value.length > 0 &&
    value.length <= 512 &&
    !/[\x00-\x1f\x7f]/.test(value)
  )
}

/** @param {unknown} value */
export function loopbackOrigin(value) {
  if (typeof value !== 'string') return undefined
  try {
    const url = new URL(value)
    return ['http:', 'https:'].includes(url.protocol) &&
      ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname) &&
      !url.username &&
      !url.password
      ? url.origin
      : undefined
  } catch {
    return undefined
  }
}

/**
 * Bind only public SDK response metadata for an exact main session on a loopback server.
 * @param {import('@opencode-ai/plugin/tui').TuiPluginApi} api
 * @param {string} sessionId
 */
export async function resolveOpenCodeUiSession(api, sessionId) {
  if (api.app.version !== '1.19.3' || !safeSessionId(sessionId)) return undefined
  try {
    const result = await api.client.session.get(
      { sessionID: sessionId },
      { signal: globalThis.AbortSignal.timeout(1200), redirect: 'error' },
    )
    const origin = loopbackOrigin(result.request.url)
    if (
      !origin ||
      result.error ||
      !result.response.ok ||
      result.response.redirected ||
      (result.response.url && loopbackOrigin(result.response.url) !== origin) ||
      result.data?.id !== sessionId ||
      result.data.parentID
    )
      return undefined
    return origin
  } catch {
    return undefined
  }
}

/** @param {Delivery} delivery */
function snapshotKey(delivery) {
  return JSON.stringify([delivery.state, delivery.sessionScope, delivery.snapshot])
}

/** @param {Delivery} delivery */
function findings(delivery) {
  return delivery.state === 'ready' && delivery.sessionScope === 'main'
    ? delivery.snapshot
        .filter(
          (row) =>
            row.status === 'failure' &&
            row.eligibility === 'ready' &&
            typeof row.message === 'string' &&
            row.message.length > 0,
        )
        .map((row) => ({ id: row.id, name: row.name, message: row.message, kind: 'finding' }))
    : []
}

/** @param {import('@opencode-ai/plugin/tui').TuiThemeCurrent} theme */
export function openCodeMarkdownStyles(theme) {
  return {
    default: { fg: theme.markdownText },
    'markup.heading': { fg: theme.markdownHeading, bold: true },
    'markup.strong': { fg: theme.markdownStrong, bold: true },
    'markup.italic': { fg: theme.markdownEmph, italic: true },
    'markup.raw': { fg: theme.markdownCode },
    'markup.link': { fg: theme.markdownLink, underline: true },
    'markup.link.label': { fg: theme.markdownLinkText, underline: true },
    'markup.link.url': { fg: theme.markdownLink, underline: true },
    'markup.list': { fg: theme.markdownListItem },
    'markup.quote': { fg: theme.markdownBlockQuote },
    'markup.strikethrough': { fg: theme.markdownText },
  }
}

/**
 * Keep reads and receipts separate from the server context consumer.
 * @param {{currentSessionId:()=>string|undefined,resolveSession:(id:string)=>Promise<string|undefined>,onFrame:(frame:AdvisoryFrame|undefined)=>void,read?:typeof readSessionChecks,acknowledge?:typeof acknowledgeSessionChecks,schedule?:(callback:()=>void)=>ReturnType<typeof setTimeout>,cancel?:typeof clearTimeout}} options
 */
export function createOpenCodeUiController(options) {
  const read = options.read ?? readSessionChecks
  const acknowledge = options.acknowledge ?? acknowledgeSessionChecks
  const schedule = options.schedule ?? ((callback) => setTimeout(callback, 0))
  const cancel = options.cancel ?? clearTimeout
  let disposed = false
  let generation = 0
  /** @type {string|undefined} */
  let sessionId
  /** @type {string|undefined} */
  let origin
  let bindingChanged = false
  let reading = false
  let queued = false
  let revision = 0
  /** @type {string|undefined} */
  let key
  /** @type {Delivery['updates']} */
  let transitions = []
  /** @type {AdvisoryFrame|undefined} */
  let frame
  /** @type {AdvisoryFrame|undefined} */
  let mounted
  /** @type {ReturnType<typeof setTimeout>|undefined} */
  let timer
  /** @type {string|null|undefined} */
  let acknowledged
  /** @type {string|null|undefined} */
  let acknowledging

  /** @param {string} id @param {number} token */
  function active(id, token) {
    return (
      !disposed && generation === token && sessionId === id && options.currentSessionId() === id
    )
  }

  function unmount() {
    mounted = undefined
    if (timer !== undefined) cancel(timer)
    timer = undefined
  }

  function clear() {
    unmount()
    frame = undefined
    options.onFrame(undefined)
  }

  function handoff() {
    if (!frame?.receipt || mounted !== frame || timer !== undefined) return
    const value = frame
    const token = generation
    if (acknowledged === value.receipt || acknowledging === value.receipt) return
    timer = schedule(() => {
      timer = undefined
      if (!active(value.sessionId, token) || frame !== value || mounted !== value) return
      acknowledging = value.receipt
      void acknowledge({
        host: 'opencode',
        sessionId: value.sessionId,
        consumer: 'ui',
        receipt: /** @type {string} */ (value.receipt),
      })
        .then((accepted) => {
          if (accepted && active(value.sessionId, token)) acknowledged = value.receipt
        })
        .catch(() => {})
        .finally(() => {
          if (active(value.sessionId, token)) acknowledging = undefined
        })
    })
  }

  async function refresh() {
    const id = sessionId
    const token = generation
    if (!safeSessionId(id) || !active(id, token)) return
    if (reading) {
      queued = true
      return
    }
    reading = true
    try {
      const binding = await options.resolveSession(id)
      if (!active(id, token)) return
      const verified = binding !== undefined && loopbackOrigin(binding) === binding
      if (verified && origin && binding !== origin) bindingChanged = true
      if (!verified || bindingChanged) {
        key = undefined
        transitions = []
        clear()
        return
      }
      origin = binding
      const delivery = await read({ host: 'opencode', sessionId: id, consumer: 'ui' })
      if (!active(id, token)) return
      if (!delivery || delivery.sessionScope !== 'main') {
        key = undefined
        transitions = []
        clear()
        return
      }
      const nextKey = snapshotKey(delivery)
      if (nextKey !== key) transitions = []
      key = nextKey
      const updates = delivery.updates.filter((update) => update.kind !== 'finding')
      if (updates.length) transitions = updates
      const rows = [...findings(delivery), ...transitions]
      const text = formatSessionCheckUpdates(rows, 'text')
      if (!text) {
        clear()
        return
      }
      if (frame?.text === text && frame.receipt === delivery.receipt) {
        handoff()
        return
      }
      unmount()
      frame = {
        sessionId: id,
        text,
        markdown: formatSessionCheckUpdates(rows),
        receipt: delivery.receipt,
        revision: ++revision,
      }
      options.onFrame(frame)
    } catch {
      if (active(id, token)) clear()
    } finally {
      reading = false
      if (queued && !disposed) {
        queued = false
        void refresh()
      }
    }
  }

  return {
    /** @param {unknown} id */
    activate(id) {
      if (disposed || id === sessionId) return
      generation += 1
      sessionId = safeSessionId(id) ? id : undefined
      origin = undefined
      bindingChanged = false
      key = undefined
      transitions = []
      acknowledged = undefined
      acknowledging = undefined
      clear()
      void refresh()
    },
    refresh,
    /** @param {AdvisoryFrame} value */
    mounted(value) {
      if (frame !== value || !active(value.sessionId, generation)) return () => {}
      mounted = value
      handoff()
      return () => {
        if (mounted === value) unmount()
      }
    },
    dispose() {
      disposed = true
      generation += 1
      queued = false
      clear()
    },
  }
}
