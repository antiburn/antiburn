import { Markdown } from '@earendil-works/pi-tui'
import { getMarkdownTheme } from '@earendil-works/pi-coding-agent'
import { registerPiSessionCheckRenderer } from './session-check-renderer.mjs'

const rendererComponents = { Markdown, getMarkdownTheme }

/** @param {import('@earendil-works/pi-coding-agent').ExtensionAPI} pi */
export default function sessionCheckRenderer(pi) {
  registerPiSessionCheckRenderer(pi, rendererComponents)
}
