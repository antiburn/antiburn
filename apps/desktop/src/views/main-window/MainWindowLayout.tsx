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
 *
 * `compactPanel` moves the panel to the bottom right and gives it the height
 * of its content. The caller sets it when the panel holds few meters, because
 * a short card at the top right crowds the controls the sections put there.
 */
export function MainWindowLayout({
  sidebar,
  panel,
  compactPanel = false,
  children,
}: {
  sidebar: ReactNode
  panel?: ReactNode
  compactPanel?: boolean
  children: ReactNode
}) {
  return (
    <main
      className={`main-window${isMacOS() ? " main-window-macos" : ""}`}
      aria-label="antiburn main window"
      data-compact-panel={compactPanel || undefined}
    >
      {isMacOS() && (
        <div className="main-window-titlebar" data-tauri-drag-region aria-hidden="true" />
      )}
      <div className="main-window-navigation">{sidebar}</div>
      <div className="main-window-workspace">{children}</div>
      {panel && (
        <div className="main-window-panel" data-compact={compactPanel || undefined}>
          <ScrollPane className="min-h-0" topEdgeFade>
            {panel}
          </ScrollPane>
        </div>
      )}
    </main>
  )
}
