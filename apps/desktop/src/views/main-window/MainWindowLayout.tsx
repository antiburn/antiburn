import type { ReactNode } from "react"

import { isMacOS } from "../../lib/platform"

/**
 * Keep navigation and feature panes beneath the native window controls.
 *
 * The optional rail stands on the right, opposite the navigation. It holds
 * what every section shows, so the workspace between them holds only the
 * section the reader chose.
 */
export function MainWindowLayout({
  sidebar,
  rail,
  children,
}: {
  sidebar: ReactNode
  rail?: ReactNode
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
      {rail && <div className="main-window-rail">{rail}</div>}
    </main>
  )
}
