import { useState } from "react"

import { hasShell, setInterfaceScale } from "../lib/ipc"
import { interfaceScaleShortcut } from "../lib/interfaceScale"
import { isMacOS } from "../lib/platform"
import { useGlobalKeydown } from "../lib/useGlobalKeydown"

/** Native menus own macOS shortcuts; other platforms use this window boundary. */
export function InterfaceScaleShortcuts() {
  const [failed, setFailed] = useState(false)
  useGlobalKeydown(hasShell() && !isMacOS(), (event) => {
    const change = interfaceScaleShortcut(event, false)
    if (!change || event.defaultPrevented) return
    event.preventDefault()
    void setInterfaceScale(change, "shortcut").then(
      () => setFailed(false),
      () => setFailed(true),
    )
  })

  return failed ? (
    <div
      role="alert"
      className="interface-scale-error type-callout text-label bg-surface-window"
    >
      <span>Could not change interface size. Try again.</span>
      <button type="button" className="ui-push-button" onClick={() => setFailed(false)}>
        Dismiss
      </button>
    </div>
  ) : null
}
