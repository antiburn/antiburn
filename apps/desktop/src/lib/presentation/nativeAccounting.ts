import type {
  NativeCalculatedCost,
  NativeSourceAccounting,
  NativeTokenClasses,
  NativeUsageReconciliation,
} from "../types/session"

const tokenFields = [
  ["inputTokens", "Input"],
  ["visibleOutputTokens", "Visible output"],
  ["reasoningTokens", "Reasoning"],
  ["cacheReadTokens", "Cache read"],
  ["cacheWriteTokens", "Cache write"],
] as const

function tokenText(tokens: NativeTokenClasses | null): string {
  if (tokens === null) return "Unknown"
  if (
    tokenFields.some(([field]) => !Number.isSafeInteger(tokens[field]) || tokens[field] < 0)
  ) {
    return "Unavailable at this precision"
  }
  return tokenFields
    .map(([field, label]) => `${label} ${tokens[field].toLocaleString()}`)
    .join(" · ")
}

function reconciliationText<T>(
  result: NativeUsageReconciliation<T> | null | undefined,
  format: (value: T) => string,
): string {
  switch (result?.status) {
    case "matched":
      return "None in recorded totals"
    case "inconsistent":
      return "Inconsistent recorded totals"
    case "unallocated":
      return format(result.value)
    default:
      return "Unknown"
  }
}

function costText(usd: number | undefined): string {
  return usd === undefined || !Number.isFinite(usd) || usd < 0
    ? "Unknown"
    : `$${usd.toFixed(6)}`
}

export function nativeAccountingRows(
  accounting: NativeSourceAccounting,
  tokens?: NativeUsageReconciliation<NativeTokenClasses> | null,
  cost?: NativeUsageReconciliation<NativeCalculatedCost> | null,
): [string, string][] {
  const aggregate = accounting.aggregateCalculatedCost?.usd
  const retained = accounting.retainedCalculatedCost?.usd
  return [
    ["Retained token classes", tokenText(accounting.retainedTokens)],
    ["Aggregate token classes", tokenText(accounting.aggregateTokens)],
    ["Unallocated token classes", reconciliationText(tokens, tokenText)],
    ["Retained calculated cost", costText(retained)],
    ["Aggregate calculated cost", costText(aggregate)],
    ["Unallocated calculated cost", reconciliationText(cost, (value) => costText(value.usd))],
  ]
}
