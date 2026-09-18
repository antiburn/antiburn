import type { ReactNode } from "react"

import { isMacOS } from "../../lib/platform"

import { ScrollPane } from "../../components/ui/ScrollPane"

/**
 * Keep navigation and feature panes beneath the native window controls.
 *
 * The optional panel floats at the top right, over the workspace. It holds
 * what every section shows, so the workspace under it holds only the section
 * the reader chose.
 *
 * The panel keeps the full window height between its margins, then scrolls.
 * A reader with many provider accounts reaches the last of them without the
 * card leaving the window.
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
      {panel && (
        <div className="main-window-panel">
          {/* The panel holds no focusable control, so the viewport itself
              carries the tab stop. Without it a keyboard reaches nothing
              that has scrolled out of sight. */}
          <ScrollPane
            className="min-h-0"
            viewportTabIndex={0}
            viewportLabel="Side panel"
            topEdgeFade
          >
            {panel}
          </ScrollPane>
        </div>
      )}
    </main>
  )
}
