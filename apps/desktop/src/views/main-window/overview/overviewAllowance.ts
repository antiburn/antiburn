import type {
  AllowanceOveragePayload,
  AllowanceUsageAccountPayload,
  AllowanceUsageSummaryPayload,
  AllowanceUtilizationPayload,
} from "../../../lib/providerUsageIpc"

/**
 * How the Overview states one account's two allowance numbers.
 *
 * Utilization is supply consumed: the share of the plan the provider's own
 * meter reports. Overage is demand refused: how often the provider blocked
 * a request. Neither follows from the other, so no function here derives
 * one from the other.
 *
 * Percent is the only unit. A dollar figure, a token count, and a "days'
 * worth" all change meaning when a vendor reprices a tier without renaming
 * it, and a reader cannot tell that happened.
 */

/** The figure a window reaches when the provider has nothing left to give. */
const MAXED_PERCENT = 100

/** One hour in seconds, for reading a wait in hours. */
const SECONDS_PER_HOUR = 3600

/** Round a percentage the way every allowance figure on this page rounds. */
function percentFigure(percent: number): string {
  return `${Math.round(percent)}%`
}

/**
 * The utilization hero figure: the average share of the subscription the
 * provider's meter reports, across every period antiburn holds.
 *
 * One figure answers "am I paying for room I never use". A range of two
 * figures asked the reader to hold two ideas to read one cell.
 */
export function utilizationFigure(utilization: AllowanceUtilizationPayload): string {
  return percentFigure(utilization.averagePercent)
}

/**
 * The noun for one instance of a window, as the store names the window.
 *
 * The weekly window measures plan fit and the rolling window measures
 * burstiness. A reader who does not know which window a figure covers reads
 * the wrong question, so every utilization label names it.
 */
function periodNoun(windowKind: string, count: number): string {
  if (windowKind === "weekly") return count === 1 ? "week" : "weeks"
  if (windowKind === "rolling") return count === 1 ? "window" : "windows"
  return count === 1 ? "period" : "periods"
}

/**
 * What the utilization figure measures.
 *
 * The figure covers every period antiburn holds, so no count of periods
 * belongs in these words. A count told the reader the size of the sample
 * and nothing about the number over it.
 */
export const UTILIZATION_LABEL = "Average subscription utilization"

/**
 * The long form of the utilization figure, for a tooltip.
 *
 * The caption names the figure and the tooltip says how antiburn makes it.
 * A reader who doubts a number wants the method and the span it covers,
 * and neither fits under a hero figure.
 */
export function utilizationTooltip(utilization: AllowanceUtilizationPayload): string {
  const count = utilization.periodCount
  const periods = `${count} ${periodNoun(utilization.windowKind, count)}`
  const one = periodNoun(utilization.windowKind, 1)
  return (
    `The average share of your plan used in one ${one}, read from the ` +
    `provider's own meter. It covers all ${periods} antiburn has readings ` +
    `for. The meter stops at 100%, so it never counts the demand the ` +
    `provider refused.`
  )
}

/**
 * The long form of the limit-hits figure, for a tooltip.
 *
 * The figure means two different things, so the tooltip does too. A wait
 * and a count answer different questions, and a reader must know which one
 * the figure over them states.
 */
export function limitHitsTooltip(overage: AllowanceOveragePayload, spanDays: number): string {
  if (overage.blockCount === 0) {
    return `How many times the provider refused a request in the last ${spanDays} days. It refused none.`
  }
  const hits = overage.blockCount === 1 ? "limit hit" : "limit hits"
  const span = `${overage.blockCount} ${hits} in the last ${spanDays} days`
  if (!statesWait(overage)) {
    return `${span}. The figure counts them instead of timing them, because none of them stated when the limit resets. A run of retries counts as one.`
  }
  return `How long you waited for the limit to reset, across ${span}. A run of retries counts as one. A limit hit that states no reset adds no time to this figure.`
}

/**
 * True when the limit hits state enough resets to give a wait.
 *
 * A limit hit that states no usable reset adds no time. The figure and the
 * caption both change with this answer, so they read it from one place.
 */
function statesWait(overage: AllowanceOveragePayload): boolean {
  return overage.waitedSeconds > 0
}

/**
 * The overage hero figure: the time the reader waited on the provider.
 *
 * The figure falls back to the count of limit hits when no limit hit states
 * a reset. A zero there would say the reader waited no time, which is a
 * different and false claim.
 */
export function limitHitsFigure(overage: AllowanceOveragePayload): string {
  if (!statesWait(overage)) return `${overage.blockCount}`
  const hours = overage.waitedSeconds / SECONDS_PER_HOUR
  if (hours < 1) return `${Math.max(1, Math.round(overage.waitedSeconds / 60))}m`
  return `${hours < 10 ? hours.toFixed(1) : Math.round(hours)}h`
}

/**
 * What the figure above counts, over the span it covers.
 *
 * The caption carries the whole sentence, because no label sits over the
 * figure. It names the wait when the figure states one, and gives the
 * figure its noun when the figure is already the count.
 */
export function limitHitsCaption(overage: AllowanceOveragePayload, spanDays: number): string {
  const hits = overage.blockCount === 1 ? "limit hit" : "limit hits"
  const span = `${hits} in ${spanDays} days`
  return statesWait(overage) ? `waiting on ${overage.blockCount} ${span}` : span
}

/**
 * The limit hits that state no reset, or null when every one states a reset.
 *
 * The wait above covers only the limit hits that state a reset. This note
 * tells the reader how many the figure leaves out.
 */
export function limitHitsNote(overage: AllowanceOveragePayload): string | null {
  if (overage.blocksWithoutWait === 0) return null
  if (overage.blocksWithoutWait === overage.blockCount) return "no stated reset"
  return `${overage.blocksWithoutWait} with no stated reset`
}

/**
 * Why the limit hits happened, from the short rolling window.
 *
 * A refusal happens at 100% and at nothing less. Across the five-hour
 * windows antiburn has recorded, windows peaking at 96% and 99% refused
 * nothing, so this line names no threshold below the ceiling.
 */
export function causeLine(burst: AllowanceUtilizationPayload | null): string | null {
  if (!burst || burst.periodCount === 0) return null
  if (burst.maxedPeriodCount === 0) return null
  const windows = periodNoun(burst.windowKind, burst.periodCount)
  return `${burst.maxedPeriodCount} of ${burst.periodCount} ${windows} reached ${MAXED_PERCENT}%`
}

/** True when an account has a number worth a cell of its own. */
export function hasAllowanceFigures(account: AllowanceUsageAccountPayload): boolean {
  return account.utilization != null || account.overage.blockCount > 0
}

/**
 * The accounts that have a number worth a cell.
 *
 * The backend sorts the accounts by provider and by account key. That order
 * keeps each cell in the same place between two reads.
 */
export function allowanceAccounts(
  summary: AllowanceUsageSummaryPayload | null,
): AllowanceUsageAccountPayload[] {
  if (!summary) return []
  return summary.accounts.filter(hasAllowanceFigures)
}
