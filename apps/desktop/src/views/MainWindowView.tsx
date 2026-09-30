import { Flame, Gauge, House, MessagesSquare, Settings } from "lucide-react"
import { useState, useSyncExternalStore, type ReactNode } from "react"
import { flushSync } from "react-dom"

import { SidebarNav, type SidebarNavItem } from "../components/ui/SidebarNav"
import { noteInteraction, openSettingsWindow } from "../lib/ipc"
import { MAIN_VIEWS, isMainViewId, type MainViewId } from "../lib/navigation/mainViews"
import { useGlobalKeydown } from "../lib/useGlobalKeydown"
import { sessionHygieneIdentities, useSessionHygiene } from "../lib/useSessionHygiene"
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
  // Keep the shared Sessions list live for main-window navigation and hygiene.
  // Detail analysis remains pane-scoped.
  const activity = useSyncExternalStore(
    sections ? neverSubscribe : activitySession.subscribeList,
    activitySession.getSnapshot,
    activitySession.getSnapshot,
  )
  // Use the full list so facet selection does not change the hygiene request key.
  const hygieneBySession = useSessionHygiene(sessionHygieneIdentities(activity.entries ?? []))
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
              filters: { agents: [], result: "all", spend: "all" },
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
              filters: { agents: [], result: "all", spend: "all" },
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
  }
  const selected =
    availableSections.find((section) => section.id === selectedId) ?? availableSections[0]
  async function chooseSearchResult(result: AppSearchResult): Promise<void> {
    const target = result.target
    if (target.kind === "setting") {
      const destination = resolveSettingsSearchTarget(target)
      await openSettingsWindow(destination.pane, destination.control)
    } else if (target.kind === "check")
      navigationSession.navigate({ section: "burnChecks", check: target.check })
    else {
      flushSync(() => {
        if (target.section === "activity" && target.filters) {
          navigationSession.navigate(
            {
              section: target.section,
              filters: target.filters,
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
            value={selected?.id ?? ""}
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
