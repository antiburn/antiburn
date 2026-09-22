import { Flame, Gauge, House, MessagesSquare, Settings } from "lucide-react"
import { useState, useSyncExternalStore, type ReactNode } from "react"
import { flushSync } from "react-dom"

import type { SessionListEntry } from "../components/session/SessionList"
import {
  SidebarNav,
  type SidebarNavChildItem,
  type SidebarNavItem,
} from "../components/ui/SidebarNav"
import type { BurnCheckDetectorId } from "../lib/insightsIpc"
import { noteInteraction, openSettingsWindow } from "../lib/ipc"
import { FIXED_SESSION_FILTERS } from "../lib/navigation/sessionFilterDefinitions"
import { agentSessionFilterLabel } from "../lib/presentation/agents"
import { MAIN_VIEWS, isMainViewId, type MainViewId } from "../lib/navigation/mainViews"
import {
  parseSessionFilterId,
  sessionFilterCounts,
  sessionFilterId,
  type SessionFilter,
} from "../lib/sessionFilters"
import { useGlobalKeydown } from "../lib/useGlobalKeydown"
import { snoozedDetectorIds, useSnoozedBurnChecks } from "../lib/snoozedBurnChecks"
import {
  sessionHygieneIdentities,
  useSessionHygiene,
  type SessionHygieneSnapshot,
} from "../lib/useSessionHygiene"
import { MainActivityView } from "./main-window/MainActivityView"
import { MainActivitySession, subjectForEntry } from "./main-window/MainActivitySession"
import { BurnChecksView } from "./main-window/BurnChecksView"
import { BurnChecksSession } from "./main-window/BurnChecksSession"
import { AppSearch } from "./main-window/AppSearch"
import { resolveSettingsSearchTarget, type AppSearchResult } from "../lib/appSearch"
import { MainWindowLayout } from "./main-window/MainWindowLayout"
import { MainWindowNavigationSession } from "./main-window/MainWindowNavigationSession"
import { MainOverviewSession } from "./main-window/MainOverviewSession"
import { OverviewView } from "./main-window/OverviewView"
import { QuotaSession } from "./main-window/quota/QuotaSession"
import { QuotaView } from "./main-window/quota/QuotaView"

export interface MainWindowSection extends SidebarNavItem {
  render: (context: { active: boolean }) => ReactNode
}

type ViewBinding = Omit<MainWindowSection, "id" | "label">

/** A store subscription that never fires, for a reader that only needs the
 *  current snapshot and must not join the store's active-viewer count. */
function neverSubscribe(): () => void {
  return () => undefined
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
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): SidebarNavChildItem[] {
  const loaded = entries !== null
  const counts = sessionFilterCounts(entries ?? [], hygiene, snoozed)
  const fixedGroup = (group: (typeof FIXED_SESSION_FILTERS)[number]["group"]) =>
    FIXED_SESSION_FILTERS.filter((filter) => filter.group === group).map((filter, index) =>
      sessionFilterChild(
        { kind: filter.id },
        filter.label,
        counts[filter.id],
        loaded,
        group !== "featured" && index === 0,
      ),
    )
  return [
    ...fixedGroup("featured"),
    ...counts.agents.map((agent, index) =>
      sessionFilterChild(
        { kind: "agent", agent: agent.agent },
        agentSessionFilterLabel(agent.agent),
        agent.count,
        loaded,
        index === 0,
      ),
    ),
    ...fixedGroup("status"),
    ...fixedGroup("all"),
  ]
}

/** A section supplies its panes without changing the main window's native lifecycle. */
export function MainWindowView({ sections }: { sections?: readonly MainWindowSection[] }) {
  const [searchOpen, setSearchOpen] = useState(false)
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
      !event.defaultPrevented &&
      !event.isComposing &&
      !document.querySelector("dialog[open], [aria-modal='true']") &&
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
  const [navigationSession] = useState(
    () => new MainWindowNavigationSession(sections ? undefined : activitySession),
  )
  const [overviewSession] = useState(() => new MainOverviewSession(activitySession))
  const [quotaSession] = useState(() => new QuotaSession())
  const navigation = useSyncExternalStore(
    navigationSession.subscribe,
    navigationSession.getSnapshot,
    navigationSession.getSnapshot,
  )
  // Read the shared Sessions list for the sidebar's counts. This list is a
  // main-window dependency, while detail analysis remains pane-scoped.
  const activity = useSyncExternalStore(
    sections ? neverSubscribe : activitySession.subscribeList,
    activitySession.getSnapshot,
    activitySession.getSnapshot,
  )
  // Pinned to the full unfiltered list, so the selected filter never changes
  // this request's key. Always live, so the sidebar's counts stay current
  // even while another section is on screen.
  const hygieneBySession = useSessionHygiene(sessionHygieneIdentities(activity.entries ?? []))
  const snoozes = useSnoozedBurnChecks()
  const snoozedDetectors = snoozedDetectorIds(snoozes.records)
  const viewBindings: Record<MainViewId, ViewBinding> = {
    overview: {
      icon: House,
      render: ({ active }) => (
        <OverviewView
          active={active}
          session={overviewSession}
          onOpenSessions={() => selectSection("activity")}
          onSelectSession={(entry) => {
            if (!entry.sessionId) return
            navigationSession.navigate({
              section: "activity",
              filter: { kind: "all" },
              subject: subjectForEntry(entry),
            })
          }}
        />
      ),
    },
    quota: {
      icon: Gauge,
      render: ({ active }) => (
        <QuotaView
          active={active}
          session={quotaSession}
          onSelectSession={(subject) => {
            navigationSession.navigate({
              section: "activity",
              filter: { kind: "all" },
              subject,
            })
          }}
        />
      ),
    },
    burnChecks: {
      icon: Flame,
      render: ({ active }) => (
        <BurnChecksView
          active={active}
          session={burnChecksSession}
          focusedCheck={navigation.destination.check}
          focusRevision={navigation.destinationRevision}
        />
      ),
    },
    activity: {
      icon: MessagesSquare,
      children: sessionFilterChildren(
        snoozes.status === "ready" ? activity.entries : null,
        hygieneBySession,
        snoozedDetectors,
      ),
      render: ({ active }) => (
        <MainActivityView
          active={active}
          session={activitySession}
          hygieneBySession={hygieneBySession}
          onOpenQuota={(target) => {
            quotaSession.open(
              { provider: target.provider, accountKey: target.accountKey, lane: target.lane },
              { startEpoch: target.rangeStart, endEpoch: target.rangeEnd },
            )
            selectSection("quota")
          }}
        />
      ),
    },
  }
  const availableSections: readonly MainWindowSection[] =
    sections ?? MAIN_VIEWS.map(({ id, label }) => ({ id, label, ...viewBindings[id] }))
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
    if (isMainViewId(id)) {
      navigationSession.select(id)
      return
    }
    navigationSession.navigate(
      {
        section: "activity",
        filter: parseSessionFilterId(id),
        subject: activitySession.getSnapshot().subject,
      },
      false,
    )
  }
  const selected =
    availableSections.find((section) => section.id === selectedId) ?? availableSections[0]
  // Highlight the active filter's own row while inside Sessions, so the
  // matching child reads as selected instead of the parent row.
  const navValue =
    !sections && selected?.id === "activity"
      ? sessionFilterId(activity.filter)
      : (selected?.id ?? "")
  async function chooseSearchResult(result: AppSearchResult): Promise<void> {
    const target = result.target
    if (target.kind === "setting") {
      const destination = resolveSettingsSearchTarget(target)
      await openSettingsWindow(destination.pane, destination.control)
    } else if (target.kind === "check")
      navigationSession.navigate({ section: "burnChecks", check: target.check })
    else {
      flushSync(() => {
        if (target.filter && target.filter.kind !== "all") {
          navigationSession.navigate(
            {
              section: target.section,
              filter: target.filter,
              subject: activitySession.getSnapshot().subject,
            },
            false,
          )
        } else {
          navigationSession.select(target.section)
        }
      })
      document.getElementById(`${target.section}-panel`)?.focus({ preventScroll: true })
    }
    noteInteraction({ kind: "appSearchResultOpened", category: target.kind })
  }
  return (
    <>
      {searchOpen && (
        <AppSearch onChoose={chooseSearchResult} onClose={() => setSearchOpen(false)} />
      )}
      <MainWindowLayout
        canBack={navigation.canBack}
        canForward={navigation.canForward}
        onBack={navigationSession.back}
        onForward={navigationSession.forward}
        onSearch={() => {
          if (!searchOpen) {
            setSearchOpen(true)
            noteInteraction({ kind: "appSearchOpened" })
          }
        }}
        searchOpen={searchOpen}
        sidebar={(closeNavigation) => (
          <SidebarNav
            items={availableSections}
            value={navValue}
            onChange={selectSection}
            onActivate={closeNavigation}
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
                  onClick={() => {
                    closeNavigation()
                    void openSettings()
                  }}
                  className="flex h-7 w-full items-center gap-2 rounded-control px-2 type-body text-label hover:bg-surface-hover"
                >
                  <Settings size={14} strokeWidth={2} aria-hidden="true" />
                  <span>Settings</span>
                </button>
              </>
            }
          />
        )}
      >
        {availableSections.map((section) => (
          <div
            key={section.id}
            id={`${section.id}-panel`}
            role="tabpanel"
            tabIndex={-1}
            aria-label={section.label}
            hidden={section.id !== selected?.id}
            className="main-window-section"
          >
            {(visited.has(section.id) || section.id === selected?.id) &&
              section.render({ active: section.id === selected?.id })}
          </div>
        ))}
      </MainWindowLayout>
    </>
  )
}
