import { ExternalLink } from "lucide-react"

import { openClaudeDesktopLimitsDocs } from "../../lib/ipc"

/**
 * "Learn more" after a limits note that antiburn cannot fix, such as Claude
 * Desktop with no Claude Code sign-in. Opens the docs page in the browser.
 */
export function LimitsDocsLink() {
  return (
    <button
      type="button"
      onClick={() => void openClaudeDesktopLimitsDocs()}
      className="inline-flex items-center gap-1 type-footnote text-accent hover:underline"
    >
      Learn more <ExternalLink size={11} aria-hidden="true" />
    </button>
  )
}
