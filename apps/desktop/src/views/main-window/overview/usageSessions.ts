import {
  sessionLimitAllocationKey,
  type SessionListEntry,
} from "../../../components/session/SessionList"
import { agentDisplayName } from "../../../lib/presentation/agents"
import type { SessionLimitAllocationPayload } from "../../../lib/providerUsageIpc"

/** The sessions and limit shares that the allowance chart can name. */
export interface UsageInput {
  entries: readonly SessionListEntry[]
  allocations: readonly SessionLimitAllocationPayload[]
}

/** One session on the allowance chart, at its last activity. */
export interface UsageSession {
  key: string
  atEpoch: number
  title: string
  /** Estimated share of the weekly limit, in percent. */
  weeklyPercent: number | null
  /** Estimated share of a 5-hour limit, in percent. */
  fiveHourPercent: number | null
}

/** The sessions with an estimated limit share on one account. A session
 *  without a share on the account is left out, because the chart cannot
 *  tell which account it used. */
export function usageSessions(
  input: UsageInput | undefined,
  provider: string,
  accountKey: string | null,
): UsageSession[] {
  if (!input) return []
  const shares = new Map<string, number>()
  for (const allocation of input.allocations) {
    if (allocation.provider !== provider) continue
    if (
      accountKey != null &&
      allocation.accountKey != null &&
      allocation.accountKey !== accountKey
    )
      continue
    shares.set(
      sessionLimitAllocationKey(
        allocation.agent,
        allocation.sessionId,
        allocation.wslDistro,
        allocation.metric,
      ),
      allocation.percent,
    )
  }
  return input.entries.flatMap((entry) => {
    if (!entry.sessionId) return []
    const share = (metric: SessionLimitAllocationPayload["metric"]) =>
      shares.get(
        sessionLimitAllocationKey(entry.agent, entry.sessionId!, entry.wslDistro, metric),
      ) ?? null
    const weeklyPercent = share("weekly")
    const fiveHourPercent = share("fiveHour")
    const atEpoch = Date.parse(entry.timestamp) / 1000
    if ((weeklyPercent == null && fiveHourPercent == null) || !Number.isFinite(atEpoch))
      return []
    return [
      {
        key: sessionLimitAllocationKey(entry.agent, entry.sessionId, entry.wslDistro, "weekly"),
        atEpoch,
        title: entry.title || agentDisplayName(entry.agent),
        weeklyPercent,
        fiveHourPercent,
      },
    ]
  })
}

/** The sessions last active in `[from, to)`, biggest share first. */
export function topSessions(
  sessions: readonly UsageSession[],
  from: number,
  to: number,
  metric: "weekly" | "fiveHour" = "weekly",
): UsageSession[] {
  const share = (session: UsageSession) =>
    (metric === "weekly" ? session.weeklyPercent : session.fiveHourPercent) ?? -1
  return sessions
    .filter((session) => session.atEpoch >= from && session.atEpoch < to && share(session) >= 0)
    .sort((left, right) => share(right) - share(left))
}
