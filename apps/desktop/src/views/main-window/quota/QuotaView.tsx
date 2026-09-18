import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { ChevronDown } from "lucide-react"
import {
  type ComponentPropsWithoutRef,
  forwardRef,
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
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { SegmentedControl } from "../../../components/ui/SegmentedControl"
import { ToggleSwitch } from "../../../components/ui/ToggleSwitch"
import { cn } from "../../../lib/cn"
import { traceEvent, traceSpan } from "../../../lib/perfTrace"
import { prefersReducedMotion } from "../../../lib/popoverHeight"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { formatSpendFigure } from "../../../lib/presentation/providerUsage"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { QuotaAccountPayload, QuotaLanePayload } from "../../../lib/providerUsageIpc"
import type { SessionSubject } from "../../../lib/sessionSubject"
import {
  formatQuotaPercent,
  QuotaBurnupChart,
  type QuotaChartAxisMode,
} from "./QuotaBurnupChart"
import type { QuotaSession } from "./QuotaSession"
import {
  isCustomRange,
  quotaBurnupSeries,
  quotaDisplayRange,
  quotaLatestPeriod,
  quotaLatestSampleEpoch,
  quotaOtherSessionsTotal,
  quotaSwatchClasses,
  quotaTopSessionRows,
  quotaUnattributedTotal,
  selectQuotaPeriods,
  type QuotaRangePreset,
  type QuotaRangeSelection,
  type QuotaTopSessionRow,
} from "./quotaSeries"

const WINDOW_RANGE_OPTIONS: ReadonlyArray<{ value: QuotaRangePreset; label: string }> = [
  { value: "thisWindow", label: "This window" },
  { value: "lastWindow", label: "Last window" },
  { value: "last3Windows", label: "Last 3 windows" },
  { value: "last5Windows", label: "Last 5 windows" },
  { value: "last10Windows", label: "Last 10 windows" },
]
const DATE_RANGE_OPTIONS: ReadonlyArray<{ value: QuotaRangePreset; label: string }> = [
  { value: "thisWeek", label: "This week" },
  { value: "lastWeek", label: "Last week" },
  { value: "last30Days", label: "30 days" },
]
const ALL_RANGE_OPTIONS = [...WINDOW_RANGE_OPTIONS, ...DATE_RANGE_OPTIONS]
const CUSTOM_RANGE_LABEL = "Custom"
const AXIS_MODE_OPTIONS: ReadonlyArray<{ value: QuotaChartAxisMode; label: string }> = [
  { value: "window", label: "Windows" },
  { value: "date", label: "Dates" },
]
/** How long the pointer must rest on one chart band before the list scrolls to its row. */
const CHART_HOVER_SCROLL_DELAY_MS = 1200

/** An account's own combined id: providers can reuse an opaque account key. */
function accountId(account: Pick<QuotaAccountPayload, "provider" | "accountKey">): string {
  return `${account.provider}:${account.accountKey}`
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
 *  chart's top five, else the "other" band the chart absorbs it into. */
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

/** A dropdown's own trigger button: the account picker and the range picker
 *  share this look. `asChild` on `DropdownMenu.Trigger` clones this element
 *  and merges in its own ref and handlers, so the button forwards both. */
const MenuTriggerButton = forwardRef<
  HTMLButtonElement,
  ComponentPropsWithoutRef<"button"> & { ariaLabel?: string; children: ReactNode }
>(function MenuTriggerButton({ ariaLabel, children, ...props }, ref) {
  return (
    <button
      ref={ref}
      type="button"
      aria-label={ariaLabel}
      className="flex h-6 items-center gap-1 rounded-control border border-separator bg-surface-secondary px-2 type-footnote text-label"
      {...props}
    >
      {children}
      <ChevronDown size={12} aria-hidden="true" />
    </button>
  )
})

function AccountPicker({
  accounts,
  selected,
  numbers,
  onSelect,
}: {
  accounts: readonly QuotaAccountPayload[]
  selected: QuotaAccountPayload | null
  numbers: ReadonlyMap<string, number>
  onSelect: (account: QuotaAccountPayload) => void
}) {
  if (accounts.length <= 3) {
    return (
      <SegmentedControl
        ariaLabel="Account"
        options={accounts.map((account) => ({
          value: accountId(account),
          label: accountLabel(account, accounts, numbers),
        }))}
        value={selected ? accountId(selected) : ""}
        onChange={(value) => {
          const account = accounts.find((candidate) => accountId(candidate) === value)
          if (account) onSelect(account)
        }}
      />
    )
  }
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <MenuTriggerButton>
          {selected ? accountLabel(selected, accounts, numbers) : "Choose account"}
        </MenuTriggerButton>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="ui-menu min-w-40"
          side="bottom"
          align="start"
          sideOffset={4}
        >
          {accounts.map((account) => (
            <DropdownMenu.Item
              key={accountId(account)}
              className="ui-menu-item"
              onSelect={() => onSelect(account)}
            >
              {accountLabel(account, accounts, numbers)}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}

/** The range control's own label: a preset's fixed label, or "Custom" while
 *  a session-detail deep link's own range is active. */
function rangeControlLabel(range: QuotaRangeSelection): string {
  if (isCustomRange(range)) return CUSTOM_RANGE_LABEL
  return ALL_RANGE_OPTIONS.find((option) => option.value === range)?.label ?? CUSTOM_RANGE_LABEL
}

/**
 * The Quota screen's range picker: a dropdown split into a "Windows" group
 * (the lane's own reset-to-reset windows) and a "Dates" group (fixed date
 * spans). While a custom range from a session-detail link is active, the
 * trigger reads "Custom" and no menu item is checked; picking any item
 * always hands the reader back a preset of their own.
 */
function RangeControl({
  range,
  onSelect,
}: {
  range: QuotaRangeSelection
  onSelect: (preset: QuotaRangePreset) => void
}) {
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <MenuTriggerButton ariaLabel="Range">{rangeControlLabel(range)}</MenuTriggerButton>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          className="ui-menu min-w-40"
          side="bottom"
          align="start"
          sideOffset={4}
        >
          <DropdownMenu.Label className="px-2 py-1 type-caption text-label-tertiary">
            Windows
          </DropdownMenu.Label>
          {WINDOW_RANGE_OPTIONS.map((option) => (
            <DropdownMenu.Item
              key={option.value}
              className="ui-menu-item"
              onSelect={() => onSelect(option.value)}
            >
              {option.label}
            </DropdownMenu.Item>
          ))}
          <DropdownMenu.Separator className="ui-menu-separator" />
          <DropdownMenu.Label className="px-2 py-1 type-caption text-label-tertiary">
            Dates
          </DropdownMenu.Label>
          {DATE_RANGE_OPTIONS.map((option) => (
            <DropdownMenu.Item
              key={option.value}
              className="ui-menu-item"
              onSelect={() => onSelect(option.value)}
            >
              {option.label}
            </DropdownMenu.Item>
          ))}
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  )
}

/** One totals row in the sessions list: the "other sessions" band and the
 *  "unattributed" band share this wrapper (the hover row, its ref, and the
 *  swatch), each filling the remaining columns with its own content. */
function GroupRow({
  rowKey,
  swatchClass,
  hovered,
  onHover,
  registerRow,
  children,
}: {
  rowKey: string
  swatchClass: string | undefined
  hovered: string | null
  onHover: (key: string | null) => void
  registerRow: (key: string, element: HTMLElement | null) => void
  children: ReactNode
}) {
  return (
    <div
      ref={(element) => registerRow(rowKey, element)}
      className={cn(
        "col-span-full grid grid-cols-subgrid items-center gap-2 rounded-control -mx-2 px-2 py-1.5 text-center type-body text-label hover:bg-surface-hover",
        hovered === rowKey && "bg-surface-hover",
      )}
      onMouseEnter={() => onHover(rowKey)}
      onMouseLeave={() => onHover(null)}
    >
      <span aria-hidden="true" className={cn("size-2 shrink-0 rounded-full", swatchClass)} />
      {children}
    </div>
  )
}

/**
 * The main window's Quota section: a header for the account, lane, and range,
 * a summary strip for the most recent period, the burnup chart, and the
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
  const [axisMode, setAxisMode] = useState<QuotaChartAxisMode>("window")
  const [showPace, setShowPace] = useState(true)
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

  // The summary strip and the session list read the same windows the chart
  // draws, so "Last window" does not chart one window and list another.
  const latestPeriod = quotaLatestPeriod(displayPeriods)
  const latestSampleEpoch = quotaLatestSampleEpoch(displayPeriods)
  const meterPeak = latestPeriod?.peakPercent ?? null
  const estimateTotal = latestPeriod?.estimatedPercent ?? null
  const gap = meterPeak != null && estimateTotal != null ? meterPeak - estimateTotal : null
  const topRows = quotaTopSessionRows(displayPeriods)
  const otherSessions = quotaOtherSessionsTotal(displayPeriods)
  const unattributed = quotaUnattributedTotal(displayPeriods)
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

  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window"
      data-quota-active={active ? "" : undefined}
    >
      <h1 className="sr-only">Quota</h1>
      {loading ? (
        <p role="status" aria-busy="true" className="p-8 type-body text-label-secondary">
          Loading Quota.
        </p>
      ) : fullPageError ? (
        <div className="flex flex-1 items-center justify-center text-center">
          <div>
            <p role="alert" className="type-body text-label-secondary">
              {state.accountsError && !accounts.length
                ? "Quota accounts are unavailable."
                : "Quota usage is unavailable."}
            </p>
            <button type="button" onClick={session.refresh} className="ui-push-button mt-3">
              Retry
            </button>
          </div>
        </div>
      ) : accountsEmpty ? (
        <p className="p-8 type-body text-label-secondary">
          No quota readings yet. Turn on live usage in Settings to start recording provider
          limits.
        </p>
      ) : (
        <div
          role="region"
          aria-label="Quota"
          className="flex min-h-0 w-full flex-1 flex-col gap-[var(--space-lg)] px-8 py-6"
        >
          <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
            <AccountPicker
              accounts={accounts}
              selected={selectedAccount}
              numbers={accountNumbers}
              onSelect={(account) =>
                session.selectAccount(account.provider, account.accountKey)
              }
            />
            {selectedAccount && selectedAccount.lanes.length > 0 && (
              <SegmentedControl
                ariaLabel="Lane"
                options={selectedAccount.lanes.map((lane) => ({
                  value: lane.lane,
                  label: lane.label,
                }))}
                value={selectedLane?.lane ?? ""}
                onChange={session.selectLane}
              />
            )}
            <RangeControl range={state.range} onSelect={session.selectRange} />
            <div className="ml-auto flex items-center gap-x-4">
              <SegmentedControl
                ariaLabel="Time axis"
                options={AXIS_MODE_OPTIONS}
                value={axisMode}
                onChange={setAxisMode}
              />
              <label className="flex items-center gap-2 type-caption text-label-secondary">
                Pace line
                <ToggleSwitch
                  checked={showPace}
                  onCheckedChange={setShowPace}
                  aria-label="Pace line"
                />
              </label>
            </div>
          </div>

          <div
            aria-busy={refreshing || undefined}
            className={cn(
              "flex min-h-0 flex-1 flex-col gap-[var(--space-lg)] transition-opacity duration-[var(--duration-medium)]",
              refreshing && "opacity-60",
            )}
          >
            {usage && latestPeriod && (
              <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 type-callout text-label-secondary">
                <span>
                  Meter peak ·{" "}
                  <span className="tabular-nums">{formatQuotaPercent(meterPeak)}</span>
                </span>
                <span>
                  Estimated total ·{" "}
                  <span className="tabular-nums">{formatQuotaPercent(estimateTotal)}</span>
                </span>
                {gap != null && (
                  <span>
                    {gap >= 0
                      ? `${Math.round(gap)}% from other devices or unattributed`
                      : `Estimate exceeds meter by ${Math.round(-gap)}%`}
                  </span>
                )}
                {latestSampleEpoch != null && (
                  <span>
                    Last reading{" "}
                    {relativeTime(new Date(latestSampleEpoch * 1000).toISOString(), {
                      compact: true,
                    })}{" "}
                    ago
                  </span>
                )}
                {state.usageError && <span role="alert">Could not refresh. </span>}
                {state.usageError && (
                  <button type="button" onClick={session.refresh} className="underline">
                    Retry
                  </button>
                )}
              </div>
            )}

            {usageEmpty ? (
              <p className="type-body text-label-secondary">No windows in this range.</p>
            ) : (
              usage &&
              series &&
              displayRange && (
                <div
                  className="flex min-h-64 shrink-0 basis-[38%] flex-col"
                  data-quota-highlight={highlightToken}
                >
                  <Profiler id="quota-chart" onRender={onRenderChart}>
                    <QuotaBurnupChart
                      usage={usage}
                      rangeStartEpoch={displayRange.startEpoch}
                      rangeEndEpoch={displayRange.endEpoch}
                      nowEpoch={state.now}
                      periods={displayPeriods}
                      axisMode={axisMode}
                      showPace={showPace}
                      series={series}
                      onHighlight={onChartHighlight}
                    />
                  </Profiler>
                </div>
              )
            )}

            {(topRows.length > 0 || otherSessions.count > 0 || unattributed.usd > 0) && (
              <section className="flex min-h-0 flex-1 flex-col gap-[var(--space-sm)]">
                <ScrollPane
                  className="min-h-0 flex-1"
                  topEdgeFade
                  bottomEdgeFade
                  viewportLabel="Most prominent sessions"
                >
                  <div className="grid grid-cols-[auto_auto_auto_auto_auto_1fr] flex-col">
                    {topRows.map((row) => {
                      const seriesKey = topRowSeriesKey(row, topSessionKeys)
                      return (
                        <button
                          type="button"
                          key={row.key}
                          ref={(element) => registerRow(seriesKey, element)}
                          className={cn(
                            "col-span-full grid grid-cols-subgrid items-center gap-2 rounded-control -mx-2 px-2 py-2.5 text-center type-body text-label hover:bg-surface-hover",
                            hovered === seriesKey && "bg-surface-hover",
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
                          <span
                            aria-hidden="true"
                            className={cn(
                              "size-2 shrink-0 rounded-full",
                              swatchClasses[seriesKey],
                            )}
                          />
                          <span className="tabular-nums font-semibold">
                            {formatQuotaPercent(row.percent)} of window
                          </span>
                          <span className="tabular-nums text-label-secondary">
                            {formatSpendFigure(row.usd)}
                          </span>
                          <span className="type-callout text-label-tertiary">
                            {row.periodCount} period{row.periodCount > 1 ? "s" : ""}
                          </span>
                          <span className="type-callout text-label-secondary">
                            [{agentDisplayName(row.agent)}]
                          </span>
                          <span className="truncate text-left">
                            {sessionRowTitle(row.title, row.agent, row.sessionId)}
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
                      >
                        <span className="tabular-nums font-semibold">
                          {formatQuotaPercent(otherSessions.percent)} of window
                        </span>
                        <span className="tabular-nums text-label-secondary">
                          {formatSpendFigure(otherSessions.usd)}
                        </span>
                        <span />
                        <span />
                        <span className="truncate text-left type-callout text-label-tertiary">
                          {otherSessions.count} other session
                          {otherSessions.count > 1 ? "s" : ""}
                        </span>
                      </GroupRow>
                    )}

                    {unattributed.usd > 0 && (
                      <GroupRow
                        rowKey="unattributed"
                        swatchClass={swatchClasses.unattributed}
                        hovered={hovered}
                        onHover={setHovered}
                        registerRow={registerRow}
                      >
                        <span />
                        <span className="min-w-0 flex-1 truncate">Unattributed</span>
                        <span className="tabular-nums">
                          {formatQuotaPercent(unattributed.percent)}
                        </span>
                        <span className="tabular-nums">
                          {formatSpendFigure(unattributed.usd)}
                        </span>
                        <span />
                      </GroupRow>
                    )}
                  </div>
                </ScrollPane>
              </section>
            )}
          </div>
        </div>
      )}
    </div>
  )
}
