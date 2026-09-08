import { Activity, Settings } from "lucide-react"
import { useState, type ReactNode } from "react"

import { openSettingsWindow } from "../lib/ipc"
import { useGlobalKeydown } from "../lib/useGlobalKeydown"
import { SidebarNav, type SidebarNavItem } from "../components/ui/SidebarNav"
import { CollectionDetailPane } from "./main-window/CollectionDetailPane"
import { MainWindowLayout } from "./main-window/MainWindowLayout"

export interface MainWindowSection extends SidebarNavItem {
  render: (context: { active: boolean }) => ReactNode
}

/** A section supplies its panes without changing the main window's native lifecycle. */
export function MainWindowView({ sections }: { sections?: readonly MainWindowSection[] }) {
  const [settingsError, setSettingsError] = useState(false)
  async function openSettings(): Promise<void> {
    setSettingsError(false)
    try {
      await openSettingsWindow()
    } catch {
      setSettingsError(true)
    }
  }
  useGlobalKeydown(true, (event) => {
    if (
      (event.metaKey || event.ctrlKey) &&
      event.key === "," &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault()
      if (!event.repeat) void openSettings()
    }
  })
  const availableSections: readonly MainWindowSection[] = sections ?? [
    {
      id: "activity",
      label: "Activity",
      icon: Activity,
      render: () => (
        <CollectionDetailPane
          title="Activity"
          items={[]}
          emptyMessage="No activity yet"
          detailEmptyMessage="Select an item to view its details."
          renderDetail={() => null}
        />
      ),
    },
  ]
  const [selectedId, setSelectedId] = useState(() => availableSections[0]?.id ?? "")
  const [visited, setVisited] = useState(
    () => new Set(availableSections.slice(0, 1).map((section) => section.id)),
  )
  function selectSection(id: string): void {
    if (!availableSections.some((section) => section.id === id)) return
    setSelectedId(id)
    setVisited((previous) => new Set(previous).add(id))
  }
  const selected =
    availableSections.find((section) => section.id === selectedId) ?? availableSections[0]
  return (
    <MainWindowLayout
      sidebar={
        <SidebarNav
          items={availableSections}
          value={selected?.id ?? ""}
          onChange={selectSection}
          ariaLabel="Main sections"
          className="main-window-sidebar min-h-0 flex-1"
          footer={
            <>
              {settingsError && (
                <p role="alert" className="px-2 pb-2 type-caption text-label-secondary">
                  Could not open Settings. Try again.
                </p>
              )}
              <button
                type="button"
                onClick={() => void openSettings()}
                className="flex h-7 w-full items-center gap-2 rounded-control px-2 type-body text-label hover:bg-surface-hover"
              >
                <Settings size={14} strokeWidth={2} aria-hidden="true" />
                <span>Settings</span>
              </button>
            </>
          }
        />
      }
    >
      {availableSections.map((section) => (
        <div
          key={section.id}
          id={`${section.id}-panel`}
          role="tabpanel"
          aria-labelledby={`${section.id}-tab`}
          hidden={section.id !== selected?.id}
          className="main-window-section"
        >
          {(visited.has(section.id) || section.id === selected?.id) &&
            section.render({ active: section.id === selected?.id })}
        </div>
      ))}
    </MainWindowLayout>
  )
}
