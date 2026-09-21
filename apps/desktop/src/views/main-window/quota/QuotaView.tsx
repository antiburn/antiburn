import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { Check, ChevronDown, Gauge } from "lucide-react"
import {
  type CSSProperties,
  Profiler,
  type ProfilerOnRenderCallback,
  type ReactNode,
  useCallback,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react"

import { useStableAccountNumbers } from "../../../components/providerUsage/useStableAccountNumbers"
import { HeroFigures, type HeroFigureCell } from "../../../components/ui/HeroFigures"
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { cn } from "../../../lib/cn"
import { traceEvent, traceSpan } from "../../../lib/perfTrace"
import { prefersReducedMotion } from "../../../lib/popoverHeight"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { formatSpendFigure } from "../../../lib/presentation/providerUsage"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { QuotaAccountPayload, QuotaLanePayload } from "../../../lib/providerUsageIpc"
import type { SessionSubject } from "../../../lib/sessionSubject"
import { resetsIn } from "../../../lib/usageBars"
import { useElementHeight } from "../../../lib/useElementWidth"
import { formatQuotaPercent, QuotaBurnupChart } from "./QuotaBurnupChart"
import type { QuotaSession } from "./QuotaSession"
import {
  isCustomRange,
  isWeeklyLane,
  quotaBurnupSeries,
  quotaDisplayRange,
  presetHasReadings,
  quotaEarliestSampleEpoch,
  quotaLatestSampleEpoch,
  quotaOtherSessionsTotal,
  quotaSwatchClasses,
  quotaTopSessionRows,
  quotaUnattributedTotal,
  quotaUnexplainedTotal,
  selectQuotaPeriods,
  type QuotaRangePreset,
  type QuotaRangeSelection,
  type QuotaTopSessionRow,
} from "./quotaSeries"
import { readQuotaViewPrefs, writeQuotaViewPrefs } from "./quotaViewPrefs"

/** Whether a percent is large enough to show as at least "0.1%" once
 *  rounded to one decimal, the same precision `formatQuotaPercent` shows. */
function roundsToAtLeastOneDecimal(value: number | null): boolean {
  return value != null && Math.round(value * 10) / 10 >= 0.1
}

/** The range menu's presets, in menu order. The date presets are not
 *  listed: a saved one still loads until the chart drops its date axis. */
const RANGE_MENU_PRESETS: readonly QuotaRangePreset[] = [
  "thisWindow",
  "lastWindow",
  "last3Windows",
  "last5Windows",
  "last10Windows",
]
/** How long the pointer must rest on one chart band before the list scrolls to its row. */
const CHART_HOVER_SCROLL_DELAY_MS = 1200

/** An account's own combined id: providers can reuse an opaque account key. */
function accountId(account: Pick<QuotaAccountPayload, "provider" | "accountKey">): string {
  return `${account.provider}:${account.accountKey}`
}

/** The word a lane's windows go by: a weekly lane resets each week, so its
 *  windows are "weeks"; every other lane's are "windows". */
function windowWord(lane: QuotaLanePayload | null, count: number): string {
  const singular = lane != null && isWeeklyLane(lane.lane) ? "week" : "window"
  return count === 1 ? singular : `${singular}s`
}

/** A range preset's label in the lane's own words: "Last 3 weeks" on a
 *  weekly lane, "Last 3 windows" on a 5-hour lane. */
function rangePresetLabel(preset: QuotaRangePreset, lane: QuotaLanePayload | null): string {
  switch (preset) {
    case "thisWindow":
      return `This ${windowWord(lane, 1)}`
    case "lastWindow":
      return `Last ${windowWord(lane, 1)}`
    case "last3Windows":
      return `Last 3 ${windowWord(lane, 3)}`
    case "last5Windows":
      return `Last 5 ${windowWord(lane, 5)}`
    case "last10Windows":
      return `Last 10 ${windowWord(lane, 10)}`
  }
}

/** The range segment's label: a preset in the lane's words, or the dates a
 *  session-detail link handed in, until the reader picks a preset. */
function rangeLabel(range: QuotaRangeSelection, lane: QuotaLanePayload | null): string {
  if (!isCustomRange(range)) return rangePresetLabel(range, lane)
  const day = (epoch: number) =>
    new Date(epoch * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short" })
  return `${day(range.startEpoch)} – ${day(range.endEpoch)}`
}

/** The window a row's percent is a share of, named by provider and lane,
 *  such as "a Claude weekly window" or "a Claude Fable weekly window". */
function windowNoun(
  account: QuotaAccountPayload | null,
  lane: QuotaLanePayload | null,
): string {
  const provider = account?.displayName ?? ""
  const kind =
    lane == null
      ? ""
      : lane.lane === "weekly"
        ? "weekly"
        : lane.lane === "fiveHour"
          ? "5-hour"
          : `${lane.label} weekly`
  const words = [provider, kind, "window"].filter((word) => word !== "").join(" ")
  return `${/^[aeiou]/i.test(words) ? "an" : "a"} ${words}`
}

/** The provider's display name, plus "account n" only when this provider has more than one. */
function accountLabel(
  account: QuotaAccountPayload,
  accounts: readonly QuotaAccountPayload[],
  numbers: ReadonlyMap<string, number>,
): string {
  const sameProvider = accounts.filter((candidate) => candidate.provider === account.provider)
  if (sameProvider.length <= 1) return account.displayName
  const number = numbers.get(account.accountKey)
  return number ? `${account.displayName} account ${number}` : account.displayName
}

/** A session row's fallback title: the agent's name and a short id. */
function sessionRowTitle(title: string | null, agent: string, sessionId: string): string {
  return title ?? `${agentDisplayName(agent)} · ${sessionId.slice(0, 8)}`
}

/** The chart series a top-sessions row highlights: its own key inside the
 *  chart's top sessions, else the "other" band the chart absorbs it into. */
function topRowSeriesKey(row: QuotaTopSessionRow, topSessionKeys: ReadonlySet<string>): string {
  return topSessionKeys.has(row.key) ? row.key : "other"
}

/** Records each commit of the burnup chart subtree. Module-level so the
 *  Profiler element never needs a fresh callback identity across renders. */
const onRenderChart: ProfilerOnRenderCallback = (id, phase, actualDuration, baseDuration) => {
  traceEvent("react.commit", {
    id,
    phase,
    actualDurationMs: actualDuration,
    baseDurationMs: baseDuration,
  })
  // React's own commit timing stops at the DOM mutation. The browser's own
  // layout and paint for that mutation land in the next frame, so this
  // schedules one to measure the cost the commit timing could not see.
  const commitEndedAt = performance.now()
  requestAnimationFrame(() => {
    traceEvent("frame.after_commit", { id, msSinceCommit: performance.now() - commitEndedAt })
  })
}

interface JumpBarOption {
  value: string
  label: string
  /** True when the choice would show nothing; the item stays listed but greyed. */
  disabled?: boolean
}

/**
 * One level of the header's jump bar: account, lane, or range. With one
 * option the level is plain text, since there is nothing to choose. With
 * more it is a borderless button that opens a menu of the options, with the
 * current one checked; its chevron shows on hover, focus, and while open.
 */
/** A range with nothing to draw: the Limits icon over a title and a
 *  caption that says why, centred where the chart would be. */
function QuotaEmptyState({ title, caption }: { title: string; caption: string }) {
  return (
    <div
      role="status"
      className="flex flex-1 flex-col items-center justify-center px-8 py-12 text-center"
    >
      <Gauge size={28} aria-hidden className="mb-3 text-label-tertiary" />
      <p className="type-body text-label">{title}</p>
      <p className="mt-1 type-callout text-label-tertiary">{caption}</p>
    </div>
  )
}

/** The pace-line toggle: a small push button under the chart that says
 *  what a press does, "Show pace" or "Hide pace". */
function PaceToggleButton({
  checked,
  onCheckedChange,
}: {
  checked: boolean
  onCheckedChange: (value: boolean) => void
}) {
  return (
    <button
      type="button"
      aria-pressed={checked}
      onClick={() => onCheckedChange(!checked)}
      className="inline-flex h-6 items-center rounded-control bg-surface-secondary px-2 type-footnote text-label-secondary hover:bg-surface-hover"
    >
      {checked ? "Hide pace" : "Show pace"}
    </button>
  )
}

function JumpBarSeparator() {
  return (
    <span aria-hidden="true" className="quota-jump-separator">
      ›
    </span>
  )
}

function JumpBarSegment({
  name,
  value,
  valueLabel,
  options,
  onSelect,
}: {
  name: string
  /** The checked option, or null while none applies (a custom range). */
  value: string | null
  valueLabel: string
  options: readonly JumpBarOption[]
  onSelect: (value: string) => void
}) {
  if (options.length <= 1) {
    return <span className="quota-jump-segment">{valueLabel}</span>
  }
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          aria-label={`${name}: ${valueLabel}`}
          className="quota-jump-segment quota-jump-button hover:bg-selected-ink/10 data-[state=open]:bg-selected-ink/15"
        >
          {valueLabel}
          <ChevronDown size={12} aria-hidden="true" className="quota-jump-chevron" />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="ui-menu min-w-40"
          side="top"
          align="center"
          sideOffset={6}
        >
          <DropdownMenu.RadioGroup value={value ?? ""} onValueChange={onSelect}>
            {options.map((option) => (
              <DropdownMenu.RadioItem
                key={option.value}
                value={option.value}
                disabled={option.disabled ?? false}
                className="ui-menu-item data-[disabled]:opacity-40"
              >
                <span className="flex w-3 shrink-0 justify-center">
                  <DropdownMenu.ItemIndicator>
                    <Check size={10} aria-hidden="true" />
                  </DropdownMenu.ItemIndicator>
                </span>
                {option.label}
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}

/** A percent for a hero cell: whole percents, an em dash with no value. */
function percentFigure(value: number | null): ReactNode {
  return <SegmentFigure>{value == null ? "—" : `${Math.round(value)}%`}</SegmentFigure>
}

/**
 * The sessions list: a caption, then the rows in a scroll pane. The section
 * sizes to its rows, up to 45% of the page, and the chart above takes the
 * rest. The cap is also the measured content height, so a change in the
 * rows moves the boundary with a transition instead of a jump; the CSS in
 * quota.css reads `--quota-list-content`. Measured here, in the component
 * that mounts with the list, so the size hooks find their elements.
 */
function QuotaSessionList({ heading, children }: { heading: string; children: ReactNode }) {
  const headingRef = useRef<HTMLHeadingElement>(null)
  const gridRef = useRef<HTMLDivElement>(null)
  const headingHeight = useElementHeight(headingRef)
  const gridHeight = useElementHeight(gridRef)
  const style =
    headingHeight > 0 && gridHeight > 0
      ? ({
          "--quota-list-content": `calc(${headingHeight + gridHeight}px + var(--space-sm))`,
        } as CSSProperties)
      : undefined
  return (
    <section
      className="quota-session-list flex min-h-0 flex-col gap-[var(--space-sm)]"
      style={style}
    >
      <h2 ref={headingRef} className="type-caption text-label-secondary">
        {heading}
      </h2>
      <ScrollPane
        className="quota-session-scroll"
        topEdgeFade
        bottomEdgeFade
        viewportLabel="Most prominent sessions"
      >
        <div
          ref={gridRef}
          className="grid grid-cols-[auto_minmax(0,1fr)_auto_auto_auto] gap-y-1.5"
        >
          {children}
        </div>
      </ScrollPane>
    </section>
  )
}

/** The columns every list row shares: swatch, agent icon, title, percent, dollars. */
const ROW_GRID_CLASS =
  "session-card col-span-full grid grid-cols-subgrid items-center gap-x-1.5 rounded-control px-3 py-2 text-left type-body"

/** One totals row in the sessions list: the "other sessions", "unattributed"
 *  and "unexplained" bands share this quieter card. Each hovers and
 *  registers like a session row, so the chart can find it. */
function GroupRow({
  rowKey,
  swatchClass,
  hovered,
  onHover,
  registerRow,
  label,
  percent,
  usd,
}: {
  rowKey: string
  swatchClass: string | undefined
  hovered: string | null
  onHover: (key: string | null) => void
  registerRow: (key: string, element: HTMLElement | null) => void
  label: string
  percent: number | null
  usd: number | null
}) {
  return (
    <div
      ref={(element) => registerRow(rowKey, element)}
      className={cn(
        ROW_GRID_CLASS,
        "bg-surface-card/50 text-label-secondary hover:bg-surface-secondary/50",
        hovered === rowKey && "bg-surface-secondary/50",
      )}
      onMouseEnter={() => onHover(rowKey)}
      onMouseLeave={() => onHover(null)}
    >
      <span aria-hidden="true" />
      <span className="min-w-0 truncate">{label}</span>
      <span aria-hidden="true" className={cn("size-2 shrink-0 rounded-full", swatchClass)} />
      <span className="text-right">
        <SegmentFigure>{formatQuotaPercent(percent)}</SegmentFigure>
      </span>
      <span className="text-right">
        {usd != null && <SegmentFigure>{formatSpendFigure(usd)}</SegmentFigure>}
      </span>
    </div>
  )
}

/** The `fraction` percentile of ascending `sorted`, with linear
 *  interpolation between the two nearest values, so a handful of windows
 *  still gives a value between them instead of one of them. */
function percentileOf(sorted: readonly number[], fraction: number): number | null {
  if (sorted.length === 0) return null
  const rank = fraction * (sorted.length - 1)
  const lower = Math.floor(rank)
  const upper = Math.min(sorted.length - 1, lower + 1)
  return sorted[lower]! + (sorted[upper]! - sorted[lower]!) * (rank - lower)
}

/**
 * The main window's Limits section: a jump bar for the account, lane, and
 * range, hero figures for the windows in range, the burnup chart, and the
 * sessions that contributed the most in range.
 */
export function QuotaView({
  active,
  session,
  onSelectSession,
}: {
  active: boolean
  session: QuotaSession
  onSelectSession: (subject: SessionSubject) => void
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const [hovered, setHovered] = useState<string | null>(null)
  // Read once per render as the initial value only: a `useState` initializer
  // runs once at mount, so a later save (from the change handler below)
  // never fights this read.
  const [showPace, setShowPace] = useState(readQuotaViewPrefs().showPace ?? true)
  // Each top-sessions row's own DOM node, keyed the same way as the chart's
  // series keys. A ref, not state: registering a row must never trigger a
  // render, and a chart hover reads the current map without depending on it.
  const rowElements = useRef<Map<string, HTMLElement>>(new Map())
  const scrollTimer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const accounts = state.accounts ?? []
  const accountNumbers = useStableAccountNumbers(
    accounts.map((account) => ({ key: account.accountKey, provider: account.provider })),
  )
  const selectedAccount =
    accounts.find(
      (account) =>
        account.provider === state.selection?.provider &&
        account.accountKey === state.selection?.accountKey,
    ) ?? null
  const selectedLane: QuotaLanePayload | null =
    selectedAccount?.lanes.find((lane) => lane.lane === state.selection?.lane) ?? null

  const loading = state.accounts == null && !state.accountsError
  const accountsEmpty = state.accounts != null && state.accounts.length === 0
  const usage = state.usage
  // The full-page loading state above covers the first load. A later
  // reload (account, lane, or range change) keeps the stale reading on
  // screen, dimmed, instead of clearing the page.
  const refreshing = state.loading && usage != null
  const fullPageError =
    (state.accountsError && !accounts.length) || (state.usageError && !usage)

  // The windows the chart actually draws, its own x range, and its series:
  // every fetched period for a date preset or a custom range, else just the
  // named windows out of what a window preset fetched. The chart's own x
  // range then hugs those windows, not the wider span a window preset can
  // fetch to find them. Building the series walks every sample and bucket in
  // range, so this must not redo that work on every hover change; the
  // top-sessions list below shares `series`, so both agree on which sessions
  // rank in the chart's top five.
  const { displayPeriods, displayRange, series } = useMemo(() => {
    if (!usage) return { displayPeriods: [], displayRange: null, series: null }
    const fetched = { startEpoch: usage.rangeStartEpoch, endEpoch: usage.rangeEndEpoch }
    const shownPeriods = selectQuotaPeriods(state.range, usage.periods, fetched, state.now)
    const shownRange = quotaDisplayRange(state.range, shownPeriods, fetched)
    const built = traceSpan("quota.series", { periods: usage.periods.length }, () =>
      quotaBurnupSeries(usage, shownRange.startEpoch, shownRange.endEpoch, state.now),
    )
    traceEvent("quota.series.built", {
      rows: built.rows.length,
      topSessions: built.topSessions.length,
    })
    return { displayPeriods: shownPeriods, displayRange: shownRange, series: built }
  }, [usage, state.range, state.now])
  const usageEmpty = usage != null && displayPeriods.length === 0
  // When this app first saw the lane, across every fetched window, not only
  // the shown ones: an empty range's caption says readings began later.
  const earliestFetchedSampleEpoch = usage ? quotaEarliestSampleEpoch(usage.periods) : null

  // The hero figures and the session list read the same windows the chart
  // draws, so "Last window" does not chart one window and list another. A
  // window's own end value is its stack top at the reset: `estimatedPercent`
  // already sums sessions, unattributed, and unexplained spend. Highest,
  // P90 and Median read every displayed window, and the open window counts
  // at its value now. The provider's own meter never passes 100 percent, so
  // a window clamps at 100 here: an estimated tail with no meter readings
  // can price a window past 100, and that overshoot shows as full instead
  // of a value the meter itself could never reach.
  const endValues = displayPeriods
    .map((period) => period.estimatedPercent)
    .filter((value): value is number => value != null)
    .map((value) => Math.min(100, value))
    .sort((a, b) => a - b)
  const openPeriod = displayPeriods.find((period) => period.resetsAtEpoch > state.now) ?? null
  const currentLimitUsage =
    openPeriod?.estimatedPercent != null ? Math.min(100, openPeriod.estimatedPercent) : null
  const latestSampleEpoch = quotaLatestSampleEpoch(displayPeriods)
  const topRows = quotaTopSessionRows(displayPeriods)
  const otherSessions = quotaOtherSessionsTotal(displayPeriods)
  const unattributed = quotaUnattributedTotal(displayPeriods)
  const unexplained = quotaUnexplainedTotal(displayPeriods)
  const topSessionKeys = useMemo(
    () => new Set((series?.topSessions ?? []).map((session) => session.key)),
    [series],
  )
  // Index into `series.topSessions`, for the wrapper attribute below: the
  // chart's own top-session areas carry a matching `quota-area-s<index>`
  // class, so this is the only lookup needed to dim every other layer.
  const topSessionIndexByKey = useMemo(
    () => new Map((series?.topSessions ?? []).map((session, index) => [session.key, index])),
    [series],
  )
  const swatchClasses = useMemo(() => quotaSwatchClasses(series?.topSessions ?? []), [series])

  // The hero cells. The open window leads with its reset countdown. The
  // three comparison cells need more than one window to compare, so a
  // single window shows one cell: the open one, or the closed one with its
  // reset time.
  const windowCount = displayPeriods.length
  const figureCells: HeroFigureCell[] = []
  if (openPeriod) {
    figureCells.push({
      key: "current",
      label: `This ${windowWord(selectedLane, 1)}`,
      figure: percentFigure(currentLimitUsage),
      caption: resetsIn(new Date(openPeriod.resetsAtEpoch * 1000), state.now * 1000),
    })
  } else if (windowCount === 1) {
    const only = displayPeriods[0]!
    const word = windowWord(selectedLane, 1)
    figureCells.push({
      key: "only",
      label: word.charAt(0).toUpperCase() + word.slice(1),
      figure: percentFigure(endValues[0] ?? null),
      caption: `reset ${relativeTime(new Date(only.resetsAtEpoch * 1000).toISOString())}`,
    })
  }
  if (windowCount > 1) {
    const ofWindows = `of ${windowCount} ${windowWord(selectedLane, windowCount)}`
    const word = windowWord(selectedLane, 1)
    figureCells.push(
      {
        key: "max",
        label: `Highest ${word}`,
        figure: percentFigure(endValues.length > 0 ? endValues[endValues.length - 1]! : null),
        caption: ofWindows,
      },
      {
        key: "p90",
        label: `P90 ${word}`,
        figure: percentFigure(percentileOf(endValues, 0.9)),
        caption: ofWindows,
      },
      {
        key: "median",
        label: `Median ${word}`,
        figure: percentFigure(percentileOf(endValues, 0.5)),
        caption: ofWindows,
      },
    )
  }

  // The token a CSS attribute selector matches in quota.css: `s<index>` for
  // a top session, else the hovered band's own name. Absent when nothing is
  // hovered. Computed fresh each render (a plain lookup, not chart work) so
  // it never forces the memoized chart below to re-render.
  const hoveredTopIndex = hovered == null ? undefined : topSessionIndexByKey.get(hovered)
  const highlightToken =
    hovered == null ? undefined : hoveredTopIndex != null ? `s${hoveredTopIndex}` : hovered

  /** Attach a top-sessions row's node to its series key, or forget it once
   *  the row unmounts. */
  const registerRow = (key: string, element: HTMLElement | null) => {
    if (element) rowElements.current.set(key, element)
    else rowElements.current.delete(key)
  }

  // Scroll a series' row into view. "nearest" leaves an already visible row
  // in place. Reads the row map through the ref at call time, so this stays
  // stable across renders with no dependency on `rowElements` itself.
  const scrollRowIntoView = useCallback((key: string) => {
    rowElements.current.get(key)?.scrollIntoView({
      block: "nearest",
      behavior: prefersReducedMotion() ? "auto" : "smooth",
    })
  }, [])

  // Stable across renders: the chart below is memoized and must not
  // re-render on hover.
  const onChartHighlight = useCallback(
    (key: string | null) => {
      setHovered(key)
      // A pass of the pointer over the chart must not move the list: the row
      // scrolls only when the pointer rests on one band for the whole delay.
      if (scrollTimer.current != null) clearTimeout(scrollTimer.current)
      scrollTimer.current =
        key == null
          ? null
          : setTimeout(() => scrollRowIntoView(key), CHART_HOVER_SCROLL_DELAY_MS)
    },
    [scrollRowIntoView],
  )

  /** Change and persist the pace-line switch. */
  const handleShowPaceChange = (value: boolean) => {
    setShowPace(value)
    writeQuotaViewPrefs({ showPace: value })
  }

  const hasListRows =
    topRows.length > 0 ||
    otherSessions.count > 0 ||
    unattributed.usd > 0 ||
    roundsToAtLeastOneDecimal(unexplained.percent)
  // The range names windows, but none holds a reading or a priced session:
  // the hero figures and the chart would show nothing, so one message
  // stands in for both.
  const noReadings = displayPeriods.length > 0 && latestSampleEpoch == null && !hasListRows
  const shownRangeLabel = rangeLabel(state.range, selectedLane).toLowerCase()

  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window"
      data-quota-active={active ? "" : undefined}
    >
      <h1 className="sr-only">Limits</h1>
      {loading ? (
        <p role="status" aria-busy="true" className="p-8 type-body text-label-secondary">
          Loading Limits.
        </p>
      ) : fullPageError ? (
        <div className="flex flex-1 items-center justify-center text-center">
          <div>
            <p role="alert" className="type-body text-label-secondary">
              {state.accountsError && !accounts.length
                ? "Limit accounts are unavailable."
                : "Limit usage is unavailable."}
            </p>
            <button type="button" onClick={session.refresh} className="ui-push-button mt-3">
              Retry
            </button>
          </div>
        </div>
      ) : accountsEmpty ? (
        <p className="p-8 type-body text-label-secondary">
          No limit readings yet. Turn on live usage in Settings to start recording provider
          limits.
        </p>
      ) : (
        <div
          role="region"
          aria-label="Limits"
          className="quota-page relative flex min-h-0 w-full flex-1 flex-col gap-[var(--space-lg)] px-8 pt-6 pb-16"
        >
          <div className="flex items-baseline justify-end gap-x-2 type-footnote text-label-tertiary">
            {latestSampleEpoch != null && (
              <span>
                Last reading{" "}
                {relativeTime(new Date(latestSampleEpoch * 1000).toISOString(), {
                  compact: true,
                })}{" "}
                ago
              </span>
            )}
            {state.usageError && <span role="alert">Could not refresh.</span>}
            {state.usageError && (
              <button type="button" onClick={session.refresh} className="underline">
                Retry
              </button>
            )}
          </div>

          <div
            aria-busy={refreshing || undefined}
            className={cn(
              "flex min-h-0 flex-1 flex-col gap-[var(--space-lg)] transition-opacity duration-[var(--duration-medium)]",
              refreshing && "opacity-60",
            )}
          >
            {figureCells.length > 0 && !noReadings && (
              <section aria-label="Limit usage">
                <HeroFigures cells={figureCells} />
              </section>
            )}

            {usageEmpty ? (
              <QuotaEmptyState
                title={`No readings for ${shownRangeLabel}.`}
                caption={
                  earliestFetchedSampleEpoch != null
                    ? `Readings began ${relativeTime(
                        new Date(earliestFetchedSampleEpoch * 1000).toISOString(),
                      )}.`
                    : "antiburn records a meter only while the app is running."
                }
              />
            ) : noReadings ? (
              <QuotaEmptyState
                title={
                  openPeriod
                    ? `No usage recorded yet this ${windowWord(selectedLane, 1)}.`
                    : `No usage recorded ${shownRangeLabel}.`
                }
                caption={
                  openPeriod
                    ? resetsIn(new Date(openPeriod.resetsAtEpoch * 1000), state.now * 1000)
                    : "antiburn records a meter only while the app is running."
                }
              />
            ) : (
              usage &&
              series &&
              displayRange && (
                <div
                  className="flex min-h-0 flex-1 flex-col"
                  data-quota-highlight={highlightToken}
                >
                  <Profiler id="quota-chart" onRender={onRenderChart}>
                    <QuotaBurnupChart
                      rangeStartEpoch={displayRange.startEpoch}
                      rangeEndEpoch={displayRange.endEpoch}
                      nowEpoch={state.now}
                      periods={displayPeriods}
                      showPace={showPace}
                      series={series}
                      onHighlight={onChartHighlight}
                    />
                  </Profiler>
                  <div className="flex justify-end">
                    <PaceToggleButton
                      checked={showPace}
                      onCheckedChange={handleShowPaceChange}
                    />
                  </div>
                </div>
              )
            )}

            {hasListRows && (
              <QuotaSessionList
                heading={`Sessions · share of ${windowNoun(selectedAccount, selectedLane)}`}
              >
                {topRows.map((row) => {
                  const seriesKey = topRowSeriesKey(row, topSessionKeys)
                  return (
                    <button
                      type="button"
                      key={row.key}
                      ref={(element) => registerRow(seriesKey, element)}
                      className={cn(
                        ROW_GRID_CLASS,
                        "bg-session-card text-label hover:bg-surface-secondary/50",
                        hovered === seriesKey && "bg-surface-secondary/50",
                      )}
                      onMouseEnter={() => setHovered(seriesKey)}
                      onMouseLeave={() => setHovered(null)}
                      onFocus={() => setHovered(seriesKey)}
                      onBlur={() => setHovered(null)}
                      onClick={() =>
                        onSelectSession({
                          agent: row.agent,
                          sessionId: row.sessionId,
                          wslDistro: row.wslDistro,
                          title: row.title ?? undefined,
                        })
                      }
                    >
                      {renderAgentIcon(row.agent, 16)}
                      <span className="min-w-0">
                        <span className="block truncate">
                          {sessionRowTitle(row.title, row.agent, row.sessionId)}
                        </span>
                        {row.periodCount > 1 && (
                          <span className="block type-caption text-label-tertiary">
                            across {row.periodCount} {windowWord(selectedLane, row.periodCount)}
                          </span>
                        )}
                      </span>
                      <span
                        aria-hidden="true"
                        className={cn("size-2 shrink-0 rounded-full", swatchClasses[seriesKey])}
                      />
                      <span className="text-right">
                        <SegmentFigure>{formatQuotaPercent(row.percent)}</SegmentFigure>
                      </span>
                      <span className="text-right text-label-secondary">
                        <SegmentFigure>{formatSpendFigure(row.usd)}</SegmentFigure>
                      </span>
                    </button>
                  )
                })}

                {otherSessions.count > 0 && (
                  <GroupRow
                    rowKey="other"
                    swatchClass={swatchClasses.other}
                    hovered={hovered}
                    onHover={setHovered}
                    registerRow={registerRow}
                    label={`${otherSessions.count} other session${otherSessions.count > 1 ? "s" : ""}`}
                    percent={otherSessions.percent}
                    usd={otherSessions.usd}
                  />
                )}

                {unattributed.usd > 0 && (
                  <GroupRow
                    rowKey="unattributed"
                    swatchClass={swatchClasses.unattributed}
                    hovered={hovered}
                    onHover={setHovered}
                    registerRow={registerRow}
                    label="Unattributed"
                    percent={unattributed.percent}
                    usd={unattributed.usd}
                  />
                )}

                {roundsToAtLeastOneDecimal(unexplained.percent) && (
                  <GroupRow
                    rowKey="unexplained"
                    swatchClass="quota-swatch-hatch"
                    hovered={hovered}
                    onHover={setHovered}
                    registerRow={registerRow}
                    label="Unexplained"
                    percent={unexplained.percent}
                    usd={null}
                  />
                )}
              </QuotaSessionList>
            )}
          </div>
          {/* The scope picker floats over the bottom of the page, in reach of
              the chart and the list it changes, like the section picker in the
              session detail. The wrapper lets pointer events through to the
              content on either side of the pill. */}
          <div className="pointer-events-none absolute inset-x-0 bottom-0 flex justify-center pb-6">
            <div
              role="group"
              aria-label="Limits scope"
              className="quota-jump pointer-events-auto flex shrink-0 items-center rounded-full bg-selected-fill p-0.5 type-callout text-selected-ink shadow-raised"
            >
              <JumpBarSegment
                name="Account"
                value={selectedAccount ? accountId(selectedAccount) : null}
                valueLabel={
                  selectedAccount
                    ? accountLabel(selectedAccount, accounts, accountNumbers)
                    : "Choose account"
                }
                options={accounts.map((account) => ({
                  value: accountId(account),
                  label: accountLabel(account, accounts, accountNumbers),
                }))}
                onSelect={(value) => {
                  const account = accounts.find((candidate) => accountId(candidate) === value)
                  if (account) session.selectAccount(account.provider, account.accountKey)
                }}
              />
              {selectedAccount && selectedAccount.lanes.length > 0 && (
                <>
                  <JumpBarSeparator />
                  <JumpBarSegment
                    name="Lane"
                    value={selectedLane?.lane ?? null}
                    valueLabel={selectedLane?.label ?? "Choose lane"}
                    options={selectedAccount.lanes.map((lane) => ({
                      value: lane.lane,
                      label: lane.label,
                    }))}
                    onSelect={session.selectLane}
                  />
                </>
              )}
              <JumpBarSeparator />
              <JumpBarSegment
                name="Range"
                value={isCustomRange(state.range) ? null : state.range}
                valueLabel={rangeLabel(state.range, selectedLane)}
                options={RANGE_MENU_PRESETS.map((preset) => {
                  const hasReadings = presetHasReadings(preset, selectedLane, state.now)
                  const label = rangePresetLabel(preset, selectedLane)
                  return {
                    value: preset,
                    label: hasReadings ? label : `${label} · no readings`,
                    disabled: !hasReadings,
                  }
                })}
                onSelect={(value) => session.selectRange(value as QuotaRangePreset)}
              />
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
