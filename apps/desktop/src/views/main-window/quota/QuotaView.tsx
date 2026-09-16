import * as DropdownMenu from "@radix-ui/react-dropdown-menu"
import { ChevronDown } from "lucide-react"
import { useCallback, useState, useSyncExternalStore } from "react"

import { useStableAccountNumbers } from "../../../components/providerUsage/useStableAccountNumbers"
import { ScrollPane } from "../../../components/ui/ScrollPane"
import { SegmentedControl } from "../../../components/ui/SegmentedControl"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { agentDisplayName } from "../../../lib/presentation/agents"
import { formatSpendFigure } from "../../../lib/presentation/providerUsage"
import { relativeTime } from "../../../lib/presentation/relativeTime"
import type { QuotaAccountPayload, QuotaLanePayload } from "../../../lib/providerUsageIpc"
import type { SessionSubject } from "../../../lib/sessionSubject"
import { formatQuotaPercent, QuotaBurnupChart } from "./QuotaBurnupChart"
import type { QuotaSession } from "./QuotaSession"
import {
  isCustomRange,
  quotaLatestPeriod,
  quotaLatestSampleEpoch,
  quotaTopSessionRows,
  quotaUnattributedTotal,
  type QuotaRangePreset,
} from "./quotaSeries"

import "./quota.css"

/** The range control's value while a custom range (from a session detail
 *  link) is showing, alongside the fixed presets. */
type RangeControlValue = QuotaRangePreset | "custom"

const RANGE_OPTIONS: ReadonlyArray<{ value: RangeControlValue; label: string }> = [
  { value: "thisWeek", label: "This week" },
  { value: "lastWeek", label: "Last week" },
  { value: "last30Days", label: "30 days" },
]
const CUSTOM_RANGE_OPTION = { value: "custom" as const, label: "Custom" }

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
        <button
          type="button"
          className="flex h-6 items-center gap-1 rounded-control border border-separator bg-surface-secondary px-2 type-footnote text-label"
        >
          {selected ? accountLabel(selected, accounts, numbers) : "Choose account"}
          <ChevronDown size={12} aria-hidden="true" />
        </button>
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
  const [pinned, setPinned] = useState<string | null>(null)
  const highlight = hovered ?? pinned
  const togglePin = useCallback(
    (series: string) => setPinned((current) => (current === series ? null : series)),
    [],
  )

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
  const periods = usage?.periods ?? []
  const usageEmpty = usage != null && periods.length === 0
  const fullPageError =
    (state.accountsError && !accounts.length) || (state.usageError && !usage)

  const latestPeriod = quotaLatestPeriod(periods)
  const latestSampleEpoch = quotaLatestSampleEpoch(periods)
  const meterPeak = latestPeriod?.peakPercent ?? null
  const estimateTotal = latestPeriod?.estimatedPercent ?? null
  const gap = meterPeak != null && estimateTotal != null ? meterPeak - estimateTotal : null
  const topRows = quotaTopSessionRows(periods)
  const unattributed = quotaUnattributedTotal(periods)

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
        <ScrollPane className="min-h-0" topEdgeFade>
          <div
            role="region"
            aria-label="Quota"
            className="flex w-full flex-col gap-[var(--space-lg)] px-8 py-6"
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
              <SegmentedControl
                ariaLabel="Range"
                // "Custom" only appears while a custom range from a session
                // detail link is active, and drops away once the reader
                // picks a preset of their own.
                options={
                  isCustomRange(state.range)
                    ? [...RANGE_OPTIONS, CUSTOM_RANGE_OPTION]
                    : RANGE_OPTIONS
                }
                value={isCustomRange(state.range) ? "custom" : state.range}
                onChange={(value) => {
                  if (value === "custom") return
                  session.selectRange(value)
                }}
              />
            </div>

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
              usage && (
                <div className="min-h-64 flex-1">
                  <QuotaBurnupChart
                    usage={usage}
                    rangeStartEpoch={usage.rangeStartEpoch}
                    rangeEndEpoch={usage.rangeEndEpoch}
                    nowEpoch={state.now}
                    highlight={highlight}
                    onHighlight={setHovered}
                    onPin={togglePin}
                  />
                </div>
              )
            )}

            {(topRows.length > 0 || unattributed.usd > 0) && (
              <section
                aria-label="Top sessions"
                className="flex flex-col gap-[var(--space-sm)]"
              >
                <h2 className="type-caption text-label-secondary">Top sessions</h2>
                <ul className="flex flex-col gap-1">
                  {topRows.map((row) => (
                    <li key={row.key}>
                      <button
                        type="button"
                        onClick={() =>
                          onSelectSession({
                            agent: row.agent,
                            sessionId: row.sessionId,
                            wslDistro: row.wslDistro,
                            title: row.title ?? undefined,
                          })
                        }
                        className="flex w-full items-center gap-2 rounded-control px-2 py-1.5 text-left type-body text-label hover:bg-surface-hover"
                      >
                        <span aria-hidden="true">{renderAgentIcon(row.agent, 14)}</span>
                        <span className="min-w-0 flex-1 truncate">
                          {sessionRowTitle(row.title, row.agent, row.sessionId)}
                        </span>
                        <span className="type-callout text-label-secondary">
                          {agentDisplayName(row.agent)}
                        </span>
                        <span className="tabular-nums">{formatQuotaPercent(row.percent)}</span>
                        <span className="tabular-nums text-label-secondary">
                          {formatSpendFigure(row.usd)}
                        </span>
                        <span className="type-callout text-label-tertiary">
                          {row.periodCount} {row.periodCount === 1 ? "period" : "periods"}
                        </span>
                      </button>
                    </li>
                  ))}
                  {unattributed.usd > 0 && (
                    <li>
                      <div className="flex w-full items-center gap-2 rounded-control px-2 py-1.5 type-body text-label-secondary">
                        <span className="min-w-0 flex-1 truncate">Unattributed</span>
                        <span className="tabular-nums">
                          {formatQuotaPercent(unattributed.percent)}
                        </span>
                        <span className="tabular-nums">
                          {formatSpendFigure(unattributed.usd)}
                        </span>
                      </div>
                    </li>
                  )}
                </ul>
              </section>
            )}
          </div>
        </ScrollPane>
      )}
    </div>
  )
}
