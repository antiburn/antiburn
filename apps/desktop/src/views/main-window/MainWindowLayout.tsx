import type { ReactNode } from "react"

import { isMacOS } from "../../lib/platform"

/** Keep navigation and feature panes beneath the native window controls. */
export function MainWindowLayout({
  sidebar,
  children,
}: {
  sidebar: ReactNode
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
    </main>
  )
}
