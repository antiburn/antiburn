// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronRight } from "lucide-react"

import { cn } from "../../lib/cn"
import type { LiveProviderUsagePayload, ProviderUsagePayload } from "../../lib/ipc"
import {
  providerWindow,
  stalenessNote,
  tokenRows,
  updatedNote,
  usageStateDescription,
} from "../../lib/presentation/providerUsage"
import { LiveUsageDetail } from "./LiveUsageDetail"
import { UsageMetricRows } from "./UsageMetricRows"
import { UsageWindowRows } from "./UsageWindowRows"
import { ProviderGlyph } from "./ProviderUsagePrimitives"

interface ProviderUsageDetailProps {
  /** The provider's own name and canonical id — always known, spend or not. */
  displayName: string
  providerId: string
  /**
   * The reader's own local spend and sessions, when this provider has ever
   * produced any. A chip driven purely by a live limit reading — the
   * provider stated a percentage, but nothing was ever run through it on
   * this device — has none, and the panel below drops that half rather than
   * fabricate figures for a provider it has never seen spend from.
   */
  provider?: ProviderUsagePayload | null
  /** The provider's own limits, when a source could prove any. */
  live?: LiveProviderUsagePayload | null
  /** Injected so the rendered output is a function of its inputs in tests. */
  now?: number
  /** Id of the element naming this panel, for the caller's `aria-labelledby`. */
  headingId?: string
  /** Rendered as an "All provider usage" affordance when supplied. */
  onViewAll?: (() => void) | undefined
}

/**
 * One provider's three windows, its token split, and what kind of evidence
 * produced them.
 *
 * Below the header, in this order: the provider's own limits when a source
 * could prove any, then the figures the reader's own sessions produced.
 *
 * The second half never has a meter or a remaining balance, because a
 * transcript records spend and not allowance — the state line at the bottom
 * says exactly that, rather than leaving the reader to assume a missing bar is
 * a loading state. The first half does have meters, because the provider
 * supplied the denominator. Keeping them visibly separate is the whole reason
 * the plan block carries its own heading in a panel this small.
 *
 * The second half is also entirely optional: a provider can reach this panel
 * on a live limit reading alone, with no local session ever attributed to it,
 * and the panel then shows only the header and the limits — never a zeroed
 * spend section standing in for evidence that does not exist.
 */
export function ProviderUsageDetail({
  displayName,
  providerId,
  provider = null,
  live = null,
  now = 0,
  headingId,
  onViewAll,
}: ProviderUsageDetailProps) {
  const stale = provider ? stalenessNote(provider) : null
  const updated = provider ? updatedNote(provider) : null
  // The broadest window the app can see, so the token split describes as much
  // history as exists rather than only the current day.
  const month = provider ? providerWindow(provider, "month") : null

  return (
    <div className="space-y-3">
      <div className="flex items-start gap-2">
        {/* The same glyph the chip carries, so the panel reads as that chip
            opened rather than as an unrelated card. */}
        <ProviderGlyph
          displayName={displayName}
          provider={providerId}
          size={18}
          className="mt-px"
        />
        <div className="min-w-0 flex-1">
          <h2 id={headingId} className="type-headline truncate text-label">
            {displayName}
          </h2>
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
      </div>

      {live && <LiveUsageDetail live={live} now={now} />}

      {provider && month && (
        <>
          <UsageMetricRows provider={provider} />

          <UsageWindowRows provider={provider} className="border-t border-separator pt-2.5" />

          <div className="space-y-1 border-t border-separator pt-2.5">
            <p className="type-caption font-medium text-label-secondary">Tokens · this month</p>
            {tokenRows(month).map((row) => (
              <div key={row.label} className="flex items-baseline justify-between gap-3">
                <span className="type-caption text-label-tertiary">{row.label}</span>
                <span className="type-caption tabular-nums text-label-secondary">
                  {row.value}
                </span>
              </div>
            ))}
          </div>

          <p className="type-caption text-label-tertiary">
            {usageStateDescription(provider.state)}
          </p>
        </>
      )}

      {onViewAll && (
        <button
          type="button"
          onClick={onViewAll}
          className="group -mx-1 flex w-[calc(100%+0.5rem)] items-center justify-between rounded-control px-1 py-1 type-footnote text-label hover:bg-surface-hover"
        >
          <span>All provider usage</span>
          <ChevronRight
            size={13}
            aria-hidden="true"
            className="text-label-tertiary group-hover:text-label-secondary"
          />
        </button>
      )}
    </div>
  )
}
