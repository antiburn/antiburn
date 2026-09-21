import type { AllowanceUsageAccountPayload } from "../../../lib/providerUsageIpc"
import {
  causeLine,
  limitHitsCaption,
  limitHitsFigure,
  limitHitsNote,
  limitHitsTooltip,
  UTILIZATION_LABEL,
  utilizationFigure,
  utilizationTooltip,
} from "./overviewAllowance"

import { Tooltip } from "../../../components/presentation/Tooltip"

import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"

import "./overview.css"

/**
 * The Overview's allowance headline: one cell for each provider account,
 * with utilization on the left and the limit hits that account met on the
 * right.
 *
 * Utilization estimates supply consumed across quota periods. Limit hits
 * count refused requests, so the two figures sit side by side.
 *
 * An account with no quota evidence shows no utilization figure.
 *
 * Each caption names what the figure over it measures, so a label above the
 * figure would say the same thing twice. The label stays for a screen
 * reader, which needs a term for each figure in the list.
 *
 * The utilization caption repeats the label word for word, so a screen
 * reader hears it once: the label carries it and the caption is drawn for
 * the eye alone.
 *
 * Each figure carries a tooltip that says how antiburn makes it and what
 * it leaves out. A hero figure has room for a name and no room for a
 * method, and a reader who doubts a number wants the method.
 */
export function OverviewAllowanceTotals({
  accounts,
  spanDays,
  utilizationSpanDays,
  loading = false,
  error = false,
}: {
  accounts: readonly AllowanceUsageAccountPayload[]
  spanDays: number
  utilizationSpanDays: number
  loading?: boolean
  error?: boolean
}) {
  if (loading) {
    return (
      <section aria-label="Allowance" aria-busy="true">
        <div className="overview-allowance">
          <AllowanceSkeleton />
        </div>
      </section>
    )
  }
  // A failed read is not an account with no meter history. The two states
  // read the same to the eye, so each one states its own cause.
  if (error && accounts.length === 0) {
    return (
      <section aria-label="Allowance">
        <p role="alert" className="type-body text-label-secondary">
          antiburn cannot read the allowance figures now. They appear here after the next read.
        </p>
      </section>
    )
  }
  if (accounts.length === 0) {
    return (
      <section aria-label="Allowance">
        <p className="type-body text-label-secondary">
          antiburn has no allowance history yet. Figures appear when a provider reports usage or
          local sessions provide an estimate.
        </p>
      </section>
    )
  }
  return (
    <section aria-label="Allowance">
      <div className="overview-allowance">
        {accounts.map((account) => (
          <AllowanceAccount
            key={`${account.provider}:${account.accountKey}`}
            account={account}
            spanDays={spanDays}
            utilizationSpanDays={utilizationSpanDays}
          />
        ))}
      </div>
    </section>
  )
}

function AllowanceAccount({
  account,
  spanDays,
  utilizationSpanDays,
}: {
  account: AllowanceUsageAccountPayload
  spanDays: number
  utilizationSpanDays: number
}) {
  const utilization = account.utilization
  const note = limitHitsNote(account.overage)
  const cause = causeLine(account.burst)
  return (
    <div className="overview-allowance-account min-w-0 border-separator">
      <p className="type-callout text-label-secondary">{account.displayName}</p>
      <dl className="overview-allowance-pair mt-[var(--space-sm)]">
        {utilization && (
          <Tooltip label={utilizationTooltip(utilization, utilizationSpanDays)}>
            <div className="min-w-0" tabIndex={0}>
              <dt className="sr-only">{UTILIZATION_LABEL}</dt>
              <dd className="type-hero-figure whitespace-nowrap font-mono text-measure">
                <SegmentFigure>{utilizationFigure(utilization)}</SegmentFigure>
              </dd>
              <dd
                aria-hidden="true"
                className="type-caption mt-[var(--space-xs)] text-label-tertiary"
              >
                {UTILIZATION_LABEL.toLowerCase()}
              </dd>
            </div>
          </Tooltip>
        )}
        <Tooltip label={limitHitsTooltip(account.overage, spanDays)}>
          <div className="min-w-0" tabIndex={0}>
            <dt className="sr-only">Limit hits</dt>
            <dd className="type-hero-figure whitespace-nowrap font-mono text-measure">
              <SegmentFigure>{limitHitsFigure(account.overage)}</SegmentFigure>
            </dd>
            <dd className="type-caption mt-[var(--space-xs)] text-label-tertiary">
              {limitHitsCaption(account.overage, spanDays)}
              {note && (
                <>
                  <span aria-hidden="true"> · </span>
                  {note}
                </>
              )}
            </dd>
            {cause && (
              <dd className="type-caption mt-[var(--space-xs)] text-label-tertiary">{cause}</dd>
            )}
          </div>
        </Tooltip>
      </dl>
    </div>
  )
}

function AllowanceSkeleton() {
  return (
    <div className="overview-allowance-account min-w-0 border-separator">
      <Skeleton className="h-3 w-24" />
      <div className="overview-allowance-pair mt-[var(--space-sm)]">
        <div className="min-w-0">
          <Skeleton className="mt-[var(--space-xs)] h-8 w-28" />
          <Skeleton className="mt-[var(--space-xs)] h-3 w-36 max-w-full" />
        </div>
        <div className="min-w-0">
          <Skeleton className="mt-[var(--space-xs)] h-8 w-16" />
          <Skeleton className="mt-[var(--space-xs)] h-3 w-32 max-w-full" />
        </div>
      </div>
    </div>
  )
}
