import { Flame, MessagesSquare, Settings } from "lucide-react"
import { useState, useSyncExternalStore, type ReactNode } from "react"

import type { SessionListEntry } from "../components/session/SessionList"
import {
  SidebarNav,
  type SidebarNavChildItem,
  type SidebarNavItem,
} from "../components/ui/SidebarNav"
import { openSettingsWindow } from "../lib/ipc"
import {
  parseSessionFilterId,
  sessionFilterCounts,
  sessionFilterId,
  type SessionFilter,
} from "../lib/sessionFilters"
import { useGlobalKeydown } from "../lib/useGlobalKeydown"
import {
  sessionHygieneIdentities,
  useSessionHygiene,
  type SessionHygieneSnapshot,
} from "../lib/useSessionHygiene"
import { MainActivityView } from "./main-window/MainActivityView"
import { MainActivitySession } from "./main-window/MainActivitySession"
import { BurnChecksView } from "./main-window/BurnChecksView"
import { BurnChecksSession } from "./main-window/BurnChecksSession"
import { MainWindowLayout } from "./main-window/MainWindowLayout"
import { MainWindowNavigationSession } from "./main-window/MainWindowNavigationSession"

export interface MainWindowSection extends SidebarNavItem {
  render: (context: { active: boolean }) => ReactNode
}

/** A store subscription that never fires, for a reader that only needs the
 *  current snapshot and must not join the store's active-viewer count. */
function neverSubscribe(): () => void {
  return () => {}
}

/** One child row under "Sessions" for a filter, with its live count. */
function sessionFilterChild(
  filter: SessionFilter,
  label: string,
  count: number,
  loaded: boolean,
  separatorBefore?: boolean,
): SidebarNavChildItem {
  return {
    id: sessionFilterId(filter),
    label,
    // Every filter child drives the same "Sessions" panel; only the parent
    // row owns a real panel id of its own.
    controls: "activity-panel",
    ...(loaded ? { count } : {}),
    ...(separatorBefore ? { separatorBefore: true } : {}),
  }
}

/**
 * Every child under "Sessions": Notable and Material, one row per harness
 * present in the loaded list, then Failing, Passing, and All. A count is
 * omitted while `entries` has not loaded yet, instead of showing zero.
 */
function sessionFilterChildren(
  entries: SessionListEntry[] | null,
  hygiene: SessionHygieneSnapshot,
): SidebarNavChildItem[] {
  const loaded = entries !== null
  const counts = sessionFilterCounts(entries ?? [], hygiene)
  return [
    sessionFilterChild({ kind: "notable" }, "Notable Sessions", counts.notable, loaded),
    sessionFilterChild({ kind: "material" }, "Material Sessions", counts.material, loaded),
    ...counts.agents.map((agent, index) =>
      sessionFilterChild(
        { kind: "agent", agent: agent.agent },
        `${agent.displayName} Sessions`,
        agent.count,
        loaded,
        index === 0,
      ),
    ),
    sessionFilterChild({ kind: "failing" }, "Failing Sessions", counts.failing, loaded, true),
    sessionFilterChild({ kind: "passing" }, "Passing Sessions", counts.passing, loaded),
    sessionFilterChild({ kind: "all" }, "All Sessions", counts.all, loaded, true),
  ]
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
  const [activitySession] = useState(() => new MainActivitySession())
  const [burnChecksSession] = useState(() => new BurnChecksSession())
  const [navigationSession] = useState(() => new MainWindowNavigationSession())
  const navigation = useSyncExternalStore(
    navigationSession.subscribe,
    navigationSession.getSnapshot,
    navigationSession.getSnapshot,
  )
  // Read the Sessions list for the sidebar's counts without joining its
  // active-viewer count, so this alone never starts loading it: the list
  // still only loads once a viewer visits Sessions.
  const activity = useSyncExternalStore(
    sections ? neverSubscribe : activitySession.subscribeInactive,
    activitySession.getSnapshot,
    activitySession.getSnapshot,
  )
  // Pinned to the full unfiltered list, so the selected filter never changes
  // this request's key. Always live, so the sidebar's counts stay current
  // even while another section is on screen.
  const hygieneBySession = useSessionHygiene(sessionHygieneIdentities(activity.entries ?? []))
  const availableSections: readonly MainWindowSection[] = sections ?? [
    {
      id: "burnChecks",
      label: "Burn checks",
      icon: Flame,
      render: ({ active }) => <BurnChecksView active={active} session={burnChecksSession} />,
    },
    {
      id: "activity",
      label: "Sessions",
      icon: MessagesSquare,
      children: sessionFilterChildren(activity.entries, hygieneBySession),
      render: ({ active }) => (
        <MainActivityView
          active={active}
          session={activitySession}
          hygieneBySession={hygieneBySession}
        />
      ),
    },
  ]
  const [customSelectedId, setCustomSelectedId] = useState(() => availableSections[0]?.id ?? "")
  const [customVisited, setCustomVisited] = useState(
    () => new Set(availableSections.slice(0, 1).map((section) => section.id)),
  )
  const selectedId = sections ? customSelectedId : navigation.selected
  const visited: ReadonlySet<string> = sections ? customVisited : new Set(navigation.visited)
  function selectSection(id: string): void {
    if (sections) {
      if (!availableSections.some((section) => section.id === id)) return
      setCustomSelectedId(id)
      setCustomVisited((previous) => new Set(previous).add(id))
      return
    }
    if (id === "burnChecks") {
      navigationSession.select(id)
      return
    }
    if (id === "activity") {
      navigationSession.select(id)
      activitySession.setFilter({ kind: "all" })
      return
    }
    // Every other id is a Sessions filter child, encoded by sessionFilterId.
    navigationSession.select("activity")
    activitySession.setFilter(parseSessionFilterId(id))
  }
  const selected =
    availableSections.find((section) => section.id === selectedId) ?? availableSections[0]
  // Highlight the active filter's own row while inside Sessions, so the
  // matching child reads as selected instead of the parent row.
  const navValue =
    !sections && selected?.id === "activity"
      ? sessionFilterId(activity.filter)
      : (selected?.id ?? "")
  return (
    <MainWindowLayout
      sidebar={
        <SidebarNav
          items={availableSections}
          value={navValue}
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
