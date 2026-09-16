import type { ReactNode } from "react"

import { isMacOS } from "../../lib/platform"
import { useViewportWidth } from "../../lib/viewport"
import { ResponsiveNavigation } from "../../components/ui/ResponsiveNavigation"

/** Keep navigation and feature panes beneath the native window controls. */
export function MainWindowLayout({
  sidebar,
  children,
}: {
  sidebar: ReactNode | ((close: () => void) => ReactNode)
  children: ReactNode
}) {
  const compact = useViewportWidth() < 720
  return (
    <main
      className={`main-window${isMacOS() ? " main-window-macos" : ""}`}
      aria-label="antiburn main window"
      data-compact-navigation={compact || undefined}
    >
      {isMacOS() && (
        <div className="main-window-titlebar" data-tauri-drag-region aria-hidden="true" />
      )}
      <ResponsiveNavigation compact={compact} label="Main navigation">
        {(close) => (
          <div className="main-window-navigation">
            {typeof sidebar === "function" ? sidebar(close) : sidebar}
          </div>
        )}
      </ResponsiveNavigation>
      <div className="main-window-workspace">{children}</div>
    </main>
  )
}
