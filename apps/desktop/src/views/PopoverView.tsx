import { useCallback, useRef, useState, useSyncExternalStore } from "react"

import { AlertTriangle, Settings } from "lucide-react"
import type { VirtualItem } from "@tanstack/react-virtual"

import { SessionList } from "../components/session/SessionList"
import { UsageLimitsBar } from "../components/providerUsage"
import {
  EMPTY_USAGE_WINDOWS,
  UsageSpendSummary,
} from "../components/providerUsage/UsageSpendSummary"
import { Banner } from "../components/ui/Banner"
import { Skeleton } from "../components/ui/Skeleton"
import { renderAgentIcon } from "../lib/agentIcon"
import { attentionBanners } from "../lib/attention"
import { AnchoredTriggerController } from "../lib/anchoredTrigger"
import {
  DEFAULT_SETTINGS,
  noteInteraction,
  openMainWindowSection,
  openMainWindowSession,
  openGithubRepo,
  openSettingsWindow,
} from "../lib/ipc"
import {
  getPopoverPeekAnchorState,
  hidePopoverPeek,
  onPopoverPeekLifecycle,
  POPOVER_PEEK_LABEL,
  showPopoverPeek,
  type PopoverPeekData,
  type PopoverPeekTarget,
} from "../lib/popoverPeekIpc"
import { checksPresentation } from "../lib/presentation/checks"
import { PopoverSession } from "./popover/PopoverSession"
import { ChecksSummary } from "./popover/ChecksView"
import { foldActivityHeader } from "./popover/usageChartFold"

function samePopoverPeekTarget(left: PopoverPeekTarget, right: PopoverPeekTarget): boolean {
  if (left.kind !== right.kind) return false
  if (left.kind === "checks" && right.kind === "checks") return true
  if (left.kind === "checks" || right.kind === "checks") return false
  return left.provider === right.provider && left.utcOffsetMinutes === right.utcOffsetMinutes
}

function createPopoverPeekTriggers(): AnchoredTriggerController<
  PopoverPeekTarget,
  PopoverPeekData
> {
  return new AnchoredTriggerController(
    POPOVER_PEEK_LABEL,
    samePopoverPeekTarget,
    {
      request: (target, anchor, presentation) =>
        showPopoverPeek(target, anchor, presentation ?? null),
      conceal: hidePopoverPeek,
      listen: onPopoverPeekLifecycle,
      state: getPopoverPeekAnchorState,
    },
    { hoverDelayMs: 150 },
  )
}

function selectedProviderPresentation(
  presentation: PopoverPeekData | undefined,
  provider: string,
): PopoverPeekData | undefined {
  if (!presentation || presentation.kind !== "provider") return undefined
  return {
    ...presentation,
    summary: {
      ...presentation.summary,
      providers: presentation.summary.providers.filter((entry) => entry.provider === provider),
    },
    live: {
      ...presentation.live,
      providers: presentation.live.providers.filter((entry) => entry.provider === provider),
      errors: presentation.live.errors.filter((entry) => entry.provider === provider),
      meters: presentation.live.meters.filter((entry) => entry.provider === provider),
    },
  }
}

/**
 * The tray popover.
 *
 * The window shows activity. Session cards open the retained main window.
 * Usage and Checks use the anchored companion window.
 * `PopoverSession` owns Escape handling and temporary attention-banner dismissal.
 */

/** Placeholder rows while the first list load is in flight. */
function ActivitySkeleton() {
  return (
    <div aria-hidden data-testid="activity-skeleton" className="space-y-1 px-3 pt-10">
      {[0, 1, 2, 3].map((row) => (
        <div key={row} className="flex flex-col gap-1.5 px-2 py-2">
          <Skeleton className="h-[var(--control-height-regular)] w-full" />
          <Skeleton className="h-3.5 w-44" />
          <Skeleton className="h-3 w-28" />
        </div>
      ))}
    </div>
  )
}

/**
 * The activity surface's bottom bar shows the app name and version.
 * The name also carries the surface's focus heading, and opens the
 * project's GitHub repository when clicked.
 * The settings control opens the standalone Settings window.
 */
function PopoverFooter({
  appVersion,
  debugBuild,
  onOpenSettings,
}: {
  appVersion: string | null
  debugBuild: boolean
  onOpenSettings: () => void
}) {
  const versionLabel = appVersion ? ` v${appVersion}${debugBuild ? " debug" : ""}` : ""

  return (
    <div className="flex h-11 shrink-0 items-center gap-2 border-t border-separator px-4">
      {/* Focused by the popover when this surface takes over, so a keyboard
          or screen-reader user lands in the view rather than on <body>. */}
      <button
        type="button"
        data-view-heading
        onClick={() => void openGithubRepo()}
        className="type-caption whitespace-nowrap text-label-secondary outline-none hover:underline"
      >
        antiburn{versionLabel}
      </button>
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
  const [peekTriggers] = useState(createPopoverPeekTriggers)
  const state = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const peekTrigger = useSyncExternalStore(
    peekTriggers.subscribe,
    peekTriggers.getSnapshot,
    peekTriggers.getSnapshot,
  )
  const peekPresentation: PopoverPeekData | undefined = state.usage
    ? { kind: "provider", summary: state.usage, live: state.liveUsage }
    : undefined
  const checks = state.checksReport
    ? checksPresentation(state.checksReport, state.checksUnavailable)
    : null

  const windowDays = state.settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays

  // Focus the activity heading when the popover renderer mounts.
  const focusHeading = useCallback((node: HTMLDivElement | null) => {
    node?.querySelector<HTMLElement>("[data-view-heading]")?.focus()
  }, [])

  // Keep the virtualizer offset stable across ordinary activity-list renders.
  const listScrollTop = useRef(0)
  const [listMeasurements, setListMeasurements] = useState<VirtualItem[]>([])
  const initialListScrollOffset = useCallback(() => listScrollTop.current, [])
  // The Activity header's fold wrapper, driven from the list's scroll events.
  const activityHeaderWrap = useRef<HTMLDivElement | null>(null)
  const restoreListScroll = useCallback(
    (node: HTMLDivElement | null) => {
      if (!node) return
      const record = () => {
        const moved = node.scrollTop !== listScrollTop.current
        listScrollTop.current = node.scrollTop
        foldActivityHeader(activityHeaderWrap.current, node)
        if (moved) void peekTriggers.leave()
      }
      // The restored offset must fold the header too, or the surface comes
      // back with the header at full height over a scrolled list.
      record()
      node.addEventListener("scroll", record, { passive: true })
      return () => node.removeEventListener("scroll", record)
    },
    [peekTriggers],
  )

  /* ---------------------------------------------------------------------
   * Attention banners
   * ------------------------------------------------------------------ */

  const banners = attentionBanners({
    repositories: state.repositories,
    storage: state.storage,
  }).filter((banner) => !state.dismissed.includes(banner.id))

  /* ---------------------------------------------------------------------
   * Surfaces
   * ------------------------------------------------------------------ */

  function body() {
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

        {/* The wrapper clips the Activity header while `foldActivityHeader` closes it in
            step with the list scroll. */}
        <div ref={activityHeaderWrap} className="shrink-0 overflow-hidden">
          <div>
            {state.usage && (
              <UsageSpendSummary
                totals={state.usage.totals ?? EMPTY_USAGE_WINDOWS}
                compact
                showApiPricingCaveat={state.liveUsage.providers.some(
                  ({ plan }) => plan !== null,
                )}
              />
            )}
            <div className="divide-y divide-separator border-b border-separator">
              <UsageLimitsBar
                live={state.liveUsage}
                expanded={limitsExpanded}
                onToggleExpanded={() => {
                  void peekTriggers.leave()
                  session.setOverviewLimitsExpanded(!limitsExpanded)
                }}
                refreshing={state.usageRefreshing}
                onHoverProvider={(provider, anchor) => {
                  if (provider && anchor) {
                    void peekTriggers.hover(
                      {
                        kind: "provider",
                        provider,
                        utcOffsetMinutes: -new Date().getTimezoneOffset(),
                      },
                      anchor,
                      selectedProviderPresentation(peekPresentation, provider),
                    )
                  } else {
                    void peekTriggers.leave()
                  }
                }}
                activeProvider={
                  peekTrigger.target?.kind === "provider" && peekTrigger.activation !== "idle"
                    ? {
                        provider: peekTrigger.target.provider,
                        activation: peekTrigger.activation,
                      }
                    : null
                }
              />
              <div className="px-2 py-1">
                <ChecksSummary
                  active={
                    peekTrigger.target?.kind === "checks" && peekTrigger.activation !== "idle"
                  }
                  presentation={checks}
                  reportUnavailable={state.checksUnavailable}
                  onPreview={(anchor) => {
                    if (!checks) return
                    void peekTriggers.hover({ kind: "checks" }, anchor, {
                      kind: "checks",
                      presentation: checks,
                      pendingEvidence: state.checksReport?.pendingEvidence ?? 0,
                    })
                  }}
                  onLeave={() => void peekTriggers.leave()}
                  onOpen={() => {
                    void peekTriggers.leave()
                    void openMainWindowSection("burnChecks")
                  }}
                />
              </div>
            </div>
          </div>
        </div>

        <div className="min-h-0 flex-1">
          {state.entries == null ? (
            <ActivitySkeleton />
          ) : (
            <SessionList
              entries={state.entries}
              days={windowDays}
              onOpenSession={(entry) => {
                if (!entry.sessionId) return
                void peekTriggers.leave()
                // Record when the activity list leads to session detail.
                // Keep this analytics event privacy-safe at the call site.
                // Which agent, and native or WSL; never the distribution's
                // name, which the reader chose.
                noteInteraction({
                  kind: "sessionOpened",
                  agent: entry.agent,
                  environment: entry.wslDistro ? "wsl" : "native",
                })
                void openMainWindowSession({
                  agent: entry.agent,
                  sessionId: entry.sessionId,
                  wslDistro: entry.wslDistro ?? null,
                })
              }}
              renderAgentIcon={renderAgentIcon}
              viewportRef={restoreListScroll}
              initialScrollOffset={initialListScrollOffset}
              initialMeasurementsCache={listMeasurements}
              onMeasurementsChange={setListMeasurements}
              badgeMetric={
                state.settings?.sessionBadgeMetric ?? DEFAULT_SETTINGS.sessionBadgeMetric
              }
              onBadgeMetricChange={session.setSessionBadgeMetric}
              now={new Date(state.now)}
              liveUsage={state.liveUsage}
              sessionLimitAllocations={state.sessionLimitAllocations}
            />
          )}
        </div>

        <PopoverFooter
          appVersion={state.appVersion}
          debugBuild={state.debugBuild}
          onOpenSettings={() => void openSettingsWindow()}
        />
      </div>
    )
  }

  return (
    <div ref={focusHeading} className="h-full">
      {body()}
    </div>
  )
}
