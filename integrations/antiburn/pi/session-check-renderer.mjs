const registrationsKey = Symbol.for('antiburn.pi.session-check-renderers.v1')
const shared = /** @type {typeof globalThis & {[key: symbol]: WeakSet<object>}} */ (globalThis)
const registrations = (shared[registrationsKey] ??= new WeakSet())

/** @typedef {{Markdown: typeof import('@earendil-works/pi-tui').Markdown, getMarkdownTheme: typeof import('@earendil-works/pi-coding-agent').getMarkdownTheme}} PiRendererComponents */

/** @param {unknown} message @param {import('@earendil-works/pi-coding-agent').MessageRenderOptions & {outputPad?:number}} options @param {PiRendererComponents} components */
export function createPiSessionCheckRenderer(message, options, components) {
  const entry = /** @type {{content?:unknown,details?:{frame?:unknown}}} */ (message)
  const markdown =
    typeof entry.details?.frame === 'string' && entry.details.frame.length > 0
      ? entry.details.frame
      : typeof entry.content === 'string'
        ? entry.content
        : ''
  return new components.Markdown(
    markdown,
    options.outputPad ?? 1,
    0,
    components.getMarkdownTheme(),
  )
}

/** @param {import('@earendil-works/pi-coding-agent').ExtensionAPI} pi @param {PiRendererComponents} components */
export function registerPiSessionCheckRenderer(pi, components) {
  if (registrations.has(pi)) return true
  if (typeof pi.registerMessageRenderer !== 'function') return false

  pi.registerMessageRenderer('antiburn-session-checks', (message, options) =>
    createPiSessionCheckRenderer(message, options, components),
  )
  registrations.add(pi)
  return true
}
