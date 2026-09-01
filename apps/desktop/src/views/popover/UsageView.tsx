import { useId, useState, useSyncExternalStore } from "react"

import { ChevronDown, ChevronLeft, PictureInPicture2 } from "lucide-react"

import { ProviderGlyph } from "../../components/providerUsage"
import { LiveUsageDetail } from "../../components/providerUsage/LiveUsageDetail"
import { UsageMetricRows } from "../../components/providerUsage/UsageMetricRows"
import { UsageWindowRows } from "../../components/providerUsage/UsageWindowRows"
import { useStableAccountNumbers } from "../../components/providerUsage/useStableAccountNumbers"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { cn } from "../../lib/cn"
import type {
  LiveProviderUsagePayload,
  LiveUsageSourceErrorPayload,
  LiveUsageSummaryPayload,
  ProviderUsagePayload,
  ProviderUsageSummaryPayload,
} from "../../lib/ipc"
import { EMPTY_LIVE_USAGE } from "../../lib/ipc"
import { HudVisibilitySession } from "../../lib/overlayWindow"
import { isMacOS } from "../../lib/platform"
import { agentDisplayName } from "../../lib/presentation/agents"
import {
  liveAuthNote,
  liveErrorNote,
  livePlanLabel,
  orderedLiveAccounts,
} from "../../lib/presentation/liveUsage"
import { noMeterSelected } from "../../lib/usageBars"
import {
  providerWindow,
  rankByWindow,
  stalenessNote,
  updatedNote,
  usageStateDescription,
  windowHasEvidence,
} from "../../lib/presentation/providerUsage"

export interface UsageViewProps {
  summary: ProviderUsageSummaryPayload
  /** The provider's own limit figures, when a source could prove any. */
  live?: LiveUsageSummaryPayload
  /**
   * The instant countdowns and elapsed markers are measured from. Defaults to
   * when the shell collected the snapshot, which is both pure — a render must
   * not read the clock — and more truthful than reading it here would be: the
   * countdown then agrees with the reading it sits under, instead of drifting
   * a little further from it on every re-render.
   */
  now?: number
  onBack: () => void
}

/** Providers split the way a reader scans them: current work first. */
function sectioned(providers: readonly ProviderUsagePayload[]): {
  recent: ProviderUsagePayload[]
  rest: ProviderUsagePayload[]
} {
  const ranked = rankByWindow(providers, "month")
  const recent = ranked.filter(
    (provider) =>
      windowHasEvidence(providerWindow(provider, "today")) || provider.staleness === "fresh",
  )
  const rest = ranked.filter((provider) => !recent.includes(provider))
  return { recent, rest }
}

interface UsageCardEntry {
  local: ProviderUsagePayload[]
  live: LiveProviderUsagePayload[]
  errors: LiveUsageSourceErrorPayload[]
  key: string
}

function isUnattributed(card: UsageCardEntry): boolean {
  return (
    card.local[0]?.provider === "unknown" ||
    card.live[0]?.provider === "unknown" ||
    card.errors[0]?.provider === "unknown"
  )
}

/** Join local evidence to live accounts and keep live-only providers visible. */
function usageCards(
  providers: readonly ProviderUsagePayload[],
  live: LiveUsageSummaryPayload,
): UsageCardEntry[] {
  const cards = new Map<string, UsageCardEntry>()
  for (const local of providers) {
    const card = cards.get(local.provider) ?? {
      local: [],
      live: [],
      errors: [],
      key: `provider:${local.provider}`,
    }
    card.local.push(local)
    cards.set(local.provider, card)
  }

  for (const reading of live.providers) {
    const card = cards.get(reading.provider) ?? {
      local: [],
      live: [],
      errors: [],
      key: `provider:${reading.provider}`,
    }
    card.live.push(reading)
    cards.set(reading.provider, card)
  }

  for (const error of live.errors) {
    if (!error.provider) continue
    const card = cards.get(error.provider) ?? {
      local: [],
      live: [],
      errors: [],
      key: `provider:${error.provider}`,
    }
    card.errors.push(error)
    cards.set(error.provider, card)
  }
  return [...cards.values()]
}

/**
 * Every provider antiburn can attribute local work to.
 *
 * Two sections — recently used, then everything else detected — with one card
 * per provider. Each card carries up to two things, in this order and never
 * the other way round:
 *
 * 1. **The provider's own limits**, when a source could prove them: real
 *    meters against a real allowance, dated with when the provider said it.
 * 2. **What this machine spent**, always: an on-device estimate at API rates
 *    over three calendar windows, whose bars are shares of the reader's own
 *    month and not a meter against anything.
 *
 * The order is deliberate and so is the fact that the second half never
 * moves. A reader who connects a source should find the limits added above
 * what they already knew, and a reader whose source goes quiet should lose
 * the top half and keep the bottom — never a view that reshuffles because a
 * file on disk went stale. The two halves also travel over separate IPC
 * commands, so the estimate payload's "no percentage, no allowance, no reset"
 * guarantee survives this feature intact.
 */
export function UsageView({ summary, live = EMPTY_LIVE_USAGE, now, onBack }: UsageViewProps) {
  // `|| 0` rather than a fallback clock: with no snapshot there is no live
  // section to render, so nothing consumes this.
  const at = now ?? (Date.parse(live.generatedAt) || 0)
  const { recent: recentLocal, rest: restLocal } = sectioned(summary.providers)
  const cards = usageCards([...recentLocal, ...restLocal], live)
  const recentProviders = new Set(recentLocal)
  const recent = cards.filter(
    (card) =>
      !isUnattributed(card) &&
      (card.local.length === 0 || card.local.some((local) => recentProviders.has(local))),
  )
  const rest = [
    ...cards.filter(
      (card) =>
        !isUnattributed(card) &&
        card.local.length > 0 &&
        !card.local.some((local) => recentProviders.has(local)),
    ),
    ...cards.filter(isUnattributed),
  ]
  const empty = recent.length === 0 && rest.length === 0
  // A reader who turned every meter off gets that sentence instead of an
  // auth note. No source ran, so no failure of theirs is current.
  const noMeter = noMeterSelected(live)
  const providerlessAuthNote = noMeter
    ? null
    : liveAuthNote({ ...live, errors: live.errors.filter((error) => !error.provider) })

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-11 shrink-0 items-center gap-1 px-2">
        <button
          type="button"
          onClick={onBack}
          aria-label="Back to activity"
          className="inline-flex h-6 shrink-0 items-center rounded-control px-1 text-label-secondary hover:bg-surface-hover"
        >
          <ChevronLeft size={15} strokeWidth={2} aria-hidden="true" />
        </button>
        {/* Focused by the popover when this surface takes over, so a keyboard
            or screen-reader user lands in the view rather than on <body>. */}
        <h1 data-view-heading tabIndex={-1} className="type-headline text-label outline-none">
          Usage
        </h1>
        {isMacOS() && <HudPopOutButton />}
      </header>

      <ScrollPane viewportClassName="px-3 pb-2">
        {providerlessAuthNote && (
          <p
            role="status"
            className="mb-2 rounded-control bg-system-orange/10 px-3 py-2 type-caption text-system-orange"
          >
            {providerlessAuthNote}
          </p>
        )}
        {noMeter && (
          <p role="status" className="mb-2 px-3 py-2 type-caption text-label-tertiary">
            No meter selected. The cards below show this machine's own estimate.
          </p>
        )}
        {empty ? (
          <p className="px-2 py-6 text-center type-footnote text-label-tertiary">
            No local evidence yet
          </p>
        ) : (
          <>
            <UsageSection title="Recently used" cards={recent} now={at} />
            <UsageSection title="All detected" cards={rest} now={at} />
          </>
        )}
      </ScrollPane>

      <footer className="shrink-0 space-y-1 border-t border-separator px-4 py-2.5">
        <p className="type-caption text-label-tertiary">
          Spend figures are local estimates, priced on this device from the sessions antiburn
          found here. Not a bill, and not your provider&rsquo;s own figure — work done on
          another machine is not counted.
        </p>
        <p className="type-caption text-label-tertiary">
          Plan limits are your provider&rsquo;s own figures, fetched directly with your own
          credentials while Settings &rarr; Usage&rsquo;s switch is on. A limit is only as
          current as the moment shown beside it.
        </p>
        <p className="type-caption text-label-tertiary">
          Each session counts in the window of its most recent activity.
        </p>
      </footer>
    </div>
  )
}

function HudPopOutButton() {
  const [session] = useState(() => new HudVisibilitySession())
  const shown = useSyncExternalStore(
    session.subscribe,
    session.getSnapshot,
    session.getSnapshot,
  )
  const label = shown ? "Hide the floating usage HUD" : "Show the floating usage HUD"

  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={shown}
      onClick={session.toggle}
      className={`ml-auto inline-flex h-6 shrink-0 items-center rounded-control px-1 hover:bg-surface-hover ${
        shown ? "text-burn" : "text-label-secondary"
      }`}
    >
      <PictureInPicture2 size={14} strokeWidth={2} aria-hidden="true" />
    </button>
  )
}

function UsageSection({
  title,
  cards,
  now,
}: {
  title: string
  cards: readonly UsageCardEntry[]
  now: number
}) {
  if (cards.length === 0) return null
  return (
    <section aria-label={title} className="pt-2 first:pt-0">
      <h2 className="px-1 pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
        {title}
      </h2>
      <ul className="space-y-2">
        {cards.map(({ key, ...card }) => (
          <ProviderCard key={key} {...card} now={now} />
        ))}
      </ul>
    </section>
  )
}

function ProviderCard({
  local,
  live,
  errors,
  now,
}: {
  local: readonly ProviderUsagePayload[]
  live: readonly LiveProviderUsagePayload[]
  errors: readonly LiveUsageSourceErrorPayload[]
  now: number
}) {
  const primaryLocal = local[0] ?? null
  const stale = primaryLocal ? stalenessNote(primaryLocal) : null
  const updated = primaryLocal ? updatedNote(primaryLocal) : null
  const usedToday = local.some((entry) => windowHasEvidence(providerWindow(entry, "today")))
  const accounts = orderedLiveAccounts(live)
  const unmatchedLocal = local.filter(
    (entry) =>
      entry.accountKey == null ||
      !accounts.some(({ reading }) => reading.accountKey === entry.accountKey),
  )
  const localAccountIdentities = local.flatMap((entry) =>
    entry.accountKey
      ? [{ key: `account:${entry.provider}:${entry.accountKey}`, provider: entry.provider }]
      : [],
  )
  const accountNumbers = useStableAccountNumbers([
    ...accounts.flatMap(({ key, reading }) =>
      reading.accountKey ? [{ key, provider: reading.provider }] : [],
    ),
    ...localAccountIdentities,
  ])
  const identifiedAccountKeys = new Set([
    ...accounts.flatMap(({ reading }) => (reading.accountKey ? [reading.accountKey] : [])),
    ...local.flatMap((entry) => (entry.accountKey ? [entry.accountKey] : [])),
  ])
  const hasUnassignedAccount =
    accounts.some(({ reading }) => reading.accountKey == null) ||
    local.some((entry) => entry.accountKey == null)
  const accountGroupCount = identifiedAccountKeys.size + (hasUnassignedAccount ? 1 : 0)
  const accountPlans = accounts.map(({ reading }) => livePlanLabel(reading))
  const plan =
    accountPlans.length > 0 && accountPlans.every((entry) => entry === accountPlans[0])
      ? accountPlans[0]
      : null
  const showAccountPlans = accountPlans.some((entry) => entry !== plan)
  const provider = primaryLocal?.provider ?? live[0]?.provider ?? errors[0]?.provider ?? ""
  const displayName =
    primaryLocal?.displayName ?? live[0]?.displayName ?? errors[0]?.displayName ?? provider
  const [open, setOpen] = useState(provider !== "unknown")
  const bodyId = useId()

  return (
    <li
      data-provider-card={provider}
      className="overflow-hidden rounded-control bg-surface-card"
    >
      <div className="flex items-start gap-2 px-3 py-2.5">
        <ProviderGlyph
          displayName={displayName}
          provider={provider}
          size={18}
          className="mt-px"
        />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <h3 className="truncate type-footnote font-medium text-label">
              {displayName}
              {plan && <span className="text-label-secondary"> · {plan}</span>}
            </h3>
            {usedToday && (
              <span className="shrink-0 rounded-full bg-system-green/15 px-1.5 py-px type-caption text-system-green">
                Used today
              </span>
            )}
          </div>
          {(stale ?? updated) && (
            <p
              className={cn(
                "type-caption",
                stale ? "text-system-orange" : "text-label-tertiary",
              )}
            >
              {stale ?? updated}
            </p>
          )}
        </div>
        <button
          type="button"
          aria-label={`${open ? "Collapse" : "Expand"} ${displayName} usage`}
          aria-expanded={open}
          aria-controls={bodyId}
          onClick={() => setOpen((value) => !value)}
          className="-mr-1 inline-flex size-6 shrink-0 cursor-pointer! items-center justify-center rounded-control text-label-secondary transition-colors duration-[var(--duration-fast)] hover:bg-surface-hover hover:text-label"
        >
          <ChevronDown
            size={14}
            strokeWidth={2}
            aria-hidden="true"
            className={cn(
              "transition-transform duration-[var(--duration-fast)] ease-out-quart",
              open && "rotate-180",
            )}
          />
        </button>
      </div>

      <div id={bodyId} hidden={!open} className="space-y-2.5 px-3 pb-2.5">
        {accounts.map(({ reading, key }) => {
          const matchingLocal = local.find(
            (entry) => entry.accountKey != null && entry.accountKey === reading.accountKey,
          )
          const accountLabel =
            accountGroupCount > 1
              ? reading.accountKey
                ? `Account ${accountNumbers.get(key)}`
                : "Unassigned account"
              : undefined
          return (
            <div key={key} className="space-y-2.5">
              <LiveUsageDetail
                live={reading}
                now={now}
                showPlan={showAccountPlans}
                {...(accountLabel ? { accountLabel } : {})}
              />
              {matchingLocal && <LocalUsageDetail provider={matchingLocal} />}
            </div>
          )
        })}

        {errors.map((error, index) => (
          <p
            key={`${error.source}:${index}`}
            role="status"
            className="rounded-control bg-system-orange/10 px-2 py-1.5 type-caption text-system-orange"
          >
            {liveErrorNote(error.category, error.provider)}
          </p>
        ))}

        {unmatchedLocal.map((entry) => {
          const key = entry.accountKey
            ? `account:${entry.provider}:${entry.accountKey}`
            : "unassigned"
          const accountNumber = accountNumbers.get(key)
          const accountLabel = entry.accountKey ? `Account ${accountNumber} usage` : null
          return (
            <div
              key={key}
              role={accountLabel ? "group" : undefined}
              aria-label={accountLabel ? `${displayName} ${accountLabel}` : undefined}
              className="space-y-2"
            >
              {accountGroupCount > 1 && (
                <p className="type-caption font-medium text-label-secondary">
                  {entry.accountKey == null ? "Unassigned account" : accountLabel}
                </p>
              )}
              <LocalUsageDetail provider={entry} />
            </div>
          )
        })}
      </div>
    </li>
  )
}

function LocalUsageDetail({ provider }: { provider: ProviderUsagePayload }) {
  const sources = provider.agents.map((entry) => agentDisplayName(entry.agent)).join(", ")
  return (
    <div className="space-y-2">
      {sources && <p className="type-caption text-label-tertiary">From {sources}</p>}
      <UsageMetricRows provider={provider} />
      <UsageWindowRows provider={provider} className="border-t border-separator pt-2" />
      <p className="type-caption text-label-tertiary">
        {usageStateDescription(provider.state)}
      </p>
    </div>
  )
}
