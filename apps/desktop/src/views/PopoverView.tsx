// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { lazy, Suspense, useCallback, useState, useSyncExternalStore } from "react"

import { AlertTriangle, Settings } from "lucide-react"

import {
  LocalActivityList,
  type LocalActivityEntry,
} from "../components/activity/LocalActivityList"
import { UsageLimitsBar } from "../components/providerUsage"
import { Banner } from "../components/ui/Banner"
import { Skeleton } from "../components/ui/Skeleton"
import { renderAgentIcon } from "../lib/agentIcon"
import { indexOfSession } from "../lib/activityEntries"
import { attentionBanners } from "../lib/attention"
import {
  DEFAULT_SETTINGS,
  noteInteraction,
  openSettingsWindow,
  type LiveUsageSummaryPayload,
  type ProviderUsageSummaryPayload,
} from "../lib/ipc"
import type { PopoverSurface } from "../lib/popoverHeight"
import { PopoverSession, sessionKey } from "./popover/PopoverSession"
import { UsageView } from "./popover/UsageView"
import type { SessionSubject } from "./popover/SessionPane"

// Session analytics pulls in the charting library and a substantial set of
// presentation components. Keep it out of the activity surface's initial
// chunk; opening a session is the only action that needs this code.
const SessionPane = lazy(() =>
  import("./popover/SessionPane").then(({ SessionPane: pane }) => ({ default: pane })),
)

/**
 * The tray popover.
 *
 * Three surfaces share one 380px window: the activity list, one session's
 * analytics, and local provider usage. There is no router — a popover is a
 * single place, and a stack of "where I came from" is all the navigation it
 * needs.
 *
 * There used to be a fourth. The first-run flow now has its own window
 * (`views/OnboardingView.tsx`, `src-tauri/src/onboarding.rs`, D-25), and with
 * it went the scan roots, folder permissions, and repository toggling this
 * component carried for one surface out of four. What is left of that here is
 * only what the attention banners genuinely read.
 *
 * Three things are owned by `PopoverSession` rather than by any one surface,
 * because they are properties of *the window* and not of a component
 * lifecycle:
 *
 * - **Height.** Each surface declares one (`lib/popoverHeight`) and the shell
 *   animates between them, bounded at 700px.
 * - **Escape.** The keyboard's way out of a tray popover. It dismisses the
 *   window unless a surface has already handled the key for something nearer —
 *   an open provider panel, say — which those surfaces signal by calling
 *   `preventDefault`.
 * - **Focus.** Swapping surfaces leaves focus on `<body>` and tells a
 *   screen-reader user nothing. Every surface marks its heading, and that
 *   heading is focused when the surface changes — here, by keying the surface
 *   wrapper so it remounts and a callback ref can claim focus each time.
 *
 * A fourth thing is owned by the session for a different reason: the
 * **attention banners** above the activity list. What they may say is decided
 * by `lib/attention`, from signals the shell reports; dismissal is held there,
 * because "I have seen this" is a fact about this run of the popover and not
 * something worth persisting.
 */

/** Placeholder rows while the first list load is in flight. */
function ActivitySkeleton() {
  return (
    <div aria-hidden data-testid="activity-skeleton" className="space-y-1 px-2 pt-10">
      {[0, 1, 2, 3].map((row) => (
        <div key={row} className="flex items-start gap-3 px-2 py-2">
          <Skeleton className="mt-0.5 h-[18px] w-[18px] shrink-0" />
          <div className="min-w-0 flex-1 space-y-1.5">
            <Skeleton className="h-3.5 w-44" />
            <Skeleton className="h-3 w-28" />
          </div>
        </div>
      ))}
    </div>
  )
}

/** Placeholder while the session analytics chunk is fetched on first open. */
function SessionPaneLoading() {
  return (
    <div className="flex h-full flex-col" aria-busy="true" data-testid="session-pane-loading">
      <header className="flex h-11 shrink-0 items-center px-4">
        <h1 data-view-heading tabIndex={-1} className="type-headline text-label outline-none">
          Session Analytics
        </h1>
      </header>
      <div className="min-h-0 flex-1 px-4 pt-4">
        <Skeleton className="h-24 w-full" />
      </div>
    </div>
  )
}

/**
 * What the usage view had to show when it opened.
 *
 * The product question is whether an installation ever gets the provider's own
 * limit figures or only antiburn's estimates from local transcripts. Three
 * values answer that; a per-provider breakdown would answer it no better and
 * would say more about the reader than the question needs.
 */
function usageEvidence(
  usage: ProviderUsageSummaryPayload | null,
  live: LiveUsageSummaryPayload,
): "live" | "estimated_only" | "none" {
  if (live.providers.length > 0) return "live"
  return (usage?.providers.length ?? 0) > 0 ? "estimated_only" : "none"
}

/**
 * The activity surface's bottom bar: the app's name, which also carries the
 * surface's focus heading (see the class doc below), and the way to the
 * standalone Settings window.
 *
 * This used to also hold a chip per provider used today. Those moved to the
 * usage-limits section at the top of the surface, collapsed — the same
 * chips, just relocated, so the footer here is name and gear and nothing
 * else.
 */
function PopoverFooter({ onOpenSettings }: { onOpenSettings: () => void }) {
  return (
    <div className="flex h-11 shrink-0 items-center gap-2 border-t border-separator px-4">
      {/* Focused by the popover when this surface takes over, so a keyboard
          or screen-reader user lands in the view rather than on <body>. */}
      <h1 data-view-heading tabIndex={-1} className="type-headline text-label outline-none">
        antiburn
      </h1>
      <button
        type="button"
        onClick={onOpenSettings}
        aria-label="Open settings"
        className="-mr-0.5 ml-auto inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-control text-label-secondary hover:bg-surface-hover"
      >
        <Settings size={14} strokeWidth={1.75} aria-hidden="true" />
      </button>
    </div>
  )
}

export function PopoverView() {
  const [session] = useState(() => new PopoverSession())
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )

  const current = state.stack.at(-1) ?? null
  const windowDays = state.settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays

  /* ---------------------------------------------------------------------
   * Window behaviour: which surface is showing, and focus on the way in
   * ------------------------------------------------------------------ */

  const surface: PopoverSurface =
    state.showUsage && state.usage ? "usage" : current ? "session" : "activity"

  // Conditional render swaps the whole surface; without this, focus is left
  // on <body> and a keyboard or screen-reader user has to walk back in from
  // the top of the document every time. `key={surface}` below forces the
  // wrapper to remount on every surface change, which is what makes this
  // ref callback fire again — a ref on a node that never remounts would only
  // ever run once.
  const focusHeading = useCallback((node: HTMLDivElement | null) => {
    node?.querySelector<HTMLElement>("[data-view-heading]")?.focus()
  }, [])

  /* ---------------------------------------------------------------------
   * Session analytics: derived from the session's tagged load result
   * ------------------------------------------------------------------ */

  const currentKey = current ? sessionKey(current) : null
  const settledAnalytics = state.analytics?.key === currentKey ? state.analytics : null
  const sessionPayload = settledAnalytics?.payload ?? null
  const sessionLoading = current != null && settledAnalytics == null
  const sessionError = settledAnalytics?.error ?? false

  /* ---------------------------------------------------------------------
   * Attention banners
   * ------------------------------------------------------------------ */

  const banners = attentionBanners({
    repositories: state.repositories,
    storage: state.storage,
  }).filter((banner) => !state.dismissed.includes(banner.id))

  const subjectFor = (entry: LocalActivityEntry): SessionSubject => {
    return {
      agent: entry.agent,
      sessionId: entry.sessionId ?? "",
      wslDistro: entry.wslDistro ?? null,
      ...(entry.title ? { title: entry.title } : {}),
      isActive: entry.isActive,
    }
  }

  /* ---------------------------------------------------------------------
   * Surfaces
   * ------------------------------------------------------------------ */

  function body() {
    // Usage sits over the list rather than in the session stack: it is a second
    // way of reading the same activity, not a place a session leads to.
    if (state.showUsage && state.usage) {
      return (
        <UsageView
          summary={state.usage}
          live={state.liveUsage}
          onBack={() => session.setShowUsage(false)}
        />
      )
    }

    if (current) {
      // Traversal only applies to a session that is actually in the list; a
      // sub-agent or a fork opened from elsewhere has no neighbours.
      const position = current.subagent
        ? -1
        : indexOfSession(
            state.entries ?? [],
            current.agent,
            current.sessionId,
            current.wslDistro,
          )
      const neighbour = (offset: number) => {
        const entry = position >= 0 ? state.entries?.[position + offset] : undefined
        if (!entry?.sessionId) return undefined
        return () => session.replaceTop(subjectFor(entry))
      }

      return (
        <Suspense fallback={<SessionPaneLoading />}>
          <SessionPane
            subject={current}
            payload={sessionPayload}
            loading={sessionLoading}
            error={sessionError}
            onBack={session.goBack}
            onPrev={neighbour(-1)}
            onNext={neighbour(1)}
            onOpenSession={session.openSession}
            onDeleted={session.sessionDeleted}
          />
        </Suspense>
      )
    }

    const limitsExpanded =
      state.settings?.overviewLimitsExpanded ?? DEFAULT_SETTINGS.overviewLimitsExpanded

    return (
      <div className="flex h-full flex-col">
        {banners.length > 0 && (
          <div className="shrink-0 space-y-1 px-2 pt-2 pb-1.5">
            {banners.map((banner) => (
              <Banner
                key={banner.id}
                icon={AlertTriangle}
                message={banner.message}
                actionLabel={banner.actionLabel}
                onAction={() => {
                  if (banner.action.kind === "rescan") {
                    void session.rescan()
                    return
                  }
                  void openSettingsWindow(banner.action.pane)
                }}
                onDismiss={() => session.dismissBanner(banner.id)}
                dismissLabel={banner.dismissLabel}
              />
            ))}
          </div>
        )}

        <UsageLimitsBar
          live={state.liveUsage}
          expanded={limitsExpanded}
          onToggleExpanded={() => session.setOverviewLimitsExpanded(!limitsExpanded)}
          refreshing={state.usageRefreshing}
          onViewAll={() => {
            // A provider pill is the one place the reader asks for the full
            // Usage view from the activity surface. Counts and a three-value
            // evidence label, never a per-provider list.
            noteInteraction({
              kind: "usageViewed",
              providers: state.usage?.providers.length ?? 0,
              evidence: usageEvidence(state.usage, state.liveUsage),
            })
            session.setShowUsage(true)
          }}
        />

        <div className="min-h-0 flex-1">
          {state.entries == null ? (
            <ActivitySkeleton />
          ) : (
            <LocalActivityList
              entries={state.entries}
              days={windowDays}
              onOpenSession={(entry) => {
                if (!entry.sessionId) return
                // The card click, not the traversal inside a session — the
                // question is how often the list leads anywhere, and the
                // newer/older arrows would drown that out. Instrumented here
                // at the call site rather than in `PopoverSession.openSession`,
                // which the session pane also calls to open a sub-agent.
                // Which agent, and native or WSL; never the distribution's
                // name, which the reader chose.
                noteInteraction({
                  kind: "sessionOpened",
                  agent: entry.agent,
                  environment: entry.wslDistro ? "wsl" : "native",
                })
                session.openSession(subjectFor(entry))
              }}
              renderAgentIcon={renderAgentIcon}
            />
          )}
        </div>

        <PopoverFooter onOpenSettings={() => void openSettingsWindow()} />
      </div>
    )
  }

  return (
    <div key={surface} ref={focusHeading} className="h-full">
      {body()}
    </div>
  )
}
