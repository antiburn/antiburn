import { Activity } from "lucide-react"

import { ScrollPane } from "../components/ui/ScrollPane"
import { SidebarNav, type SidebarNavItem } from "../components/ui/SidebarNav"
import { isMacOS } from "../lib/platform"

const NAVIGATION_ITEMS = [
  { id: "activity", label: "Activity", icon: Activity },
] as const satisfies readonly SidebarNavItem[]

/** The retained application window shell. Feature views replace its honest placeholders later. */
export function MainWindowView() {
  const macOS = isMacOS()

  return (
    <main
      className={`main-window${macOS ? " main-window-macos" : ""}`}
      aria-label="antiburn main window"
    >
      {macOS && (
        <div className="main-window-titlebar" data-tauri-drag-region aria-hidden="true" />
      )}

      <div className="main-window-navigation">
        <SidebarNav
          items={NAVIGATION_ITEMS}
          value="activity"
          onChange={() => {}}
          ariaLabel="Main sections"
          className="main-window-sidebar min-h-0 flex-1"
        />
      </div>

      <div className={`main-window-content${macOS ? " main-window-content-macos" : ""}`}>
        <ScrollPane className="min-h-0" viewportClassName="main-window-scroll-viewport">
          <section
            id="activity-panel"
            role="tabpanel"
            aria-labelledby="activity-tab"
            tabIndex={0}
            className="main-window-pane"
          >
            <div className="main-window-placeholder">
              <Activity
                size={24}
                strokeWidth={1.5}
                className="text-label-secondary"
                aria-hidden="true"
              />
              <h1 className="type-title-2 text-label">Activity</h1>
              <p className="type-body text-label-secondary">
                Activity will appear here. Current activity remains available from the menu bar.
              </p>
            </div>
          </section>
        </ScrollPane>
      </div>
    </main>
  )
}
