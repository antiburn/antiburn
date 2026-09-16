import type { ReactNode } from "react"

import { isMacOS } from "../../lib/platform"

/**
 * Keep navigation and feature panes beneath the native window controls.
 *
 * The optional panel floats at the top right, over the workspace. It holds
 * what every section shows, so the workspace under it holds only the section
 * the reader chose.
 */
export function MainWindowLayout({
  sidebar,
  panel,
  children,
}: {
  sidebar: ReactNode
  panel?: ReactNode
  children: ReactNode
}) {
  return (
    <main
      className={`main-window${isMacOS() ? " main-window-macos" : ""}`}
      aria-label="antiburn main window"
    >
      {isMacOS() && (
        <div className="main-window-titlebar" data-tauri-drag-region aria-hidden="true" />
      )}
      <div className="main-window-navigation">{sidebar}</div>
      <div className="main-window-workspace">{children}</div>
      {panel && <div className="main-window-panel">{panel}</div>}
    </main>
  )
}
