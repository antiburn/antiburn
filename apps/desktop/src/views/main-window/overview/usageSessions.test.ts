import { describe, expect, it } from "vitest"

import type { SessionListEntry } from "../../../components/session/SessionList"
import type { SessionLimitAllocationPayload } from "../../../lib/providerUsageIpc"
import { topSessions, usageSessions } from "./usageSessions"

function entry(sessionId: string, timestamp: string, title?: string): SessionListEntry {
  return { agent: "claude", sessionId, repo: "web", timestamp, title } as SessionListEntry
}

function allocation(
  sessionId: string,
  metric: SessionLimitAllocationPayload["metric"],
  percent: number,
  accountKey = "me",
): SessionLimitAllocationPayload {
  return {
    agent: "claude",
    sessionId,
    wslDistro: null,
    metric,
    provider: "anthropic",
    accountKey,
    percent,
  } as SessionLimitAllocationPayload
}

const input = {
  entries: [
    entry("a", "2026-09-20T01:00:00Z", "Fix login"),
    entry("b", "2026-09-20T02:00:00Z"),
    entry("c", "2026-09-20T03:00:00Z", "Other account"),
    entry("d", "2026-09-20T04:00:00Z", "No share"),
  ],
  allocations: [
    allocation("a", "weekly", 4),
    allocation("a", "fiveHour", 30),
    allocation("b", "weekly", 9),
    allocation("c", "weekly", 50, "someone-else"),
  ],
}

describe("usageSessions", () => {
  it("keeps only the sessions with a share on the account", () => {
    const sessions = usageSessions(input, "anthropic", "me")
    expect(sessions.map((session) => [session.title, session.weeklyPercent])).toEqual([
      ["Fix login", 4],
      ["Claude", 9],
    ])
    expect(sessions[0]?.fiveHourPercent).toBe(30)
    expect(sessions[0]?.atEpoch).toBe(Date.parse("2026-09-20T01:00:00Z") / 1000)
    expect(usageSessions(input, "openai", "me")).toEqual([])
    expect(usageSessions(undefined, "anthropic", "me")).toEqual([])
  })

  it("lists the sessions of a span by share, biggest first", () => {
    const sessions = usageSessions(input, "anthropic", "me")
    const from = Date.parse("2026-09-20T00:00:00Z") / 1000
    expect(topSessions(sessions, from, from + 86_400).map((session) => session.title)).toEqual([
      "Claude",
      "Fix login",
    ])
    expect(topSessions(sessions, from, from + 7_200).map((session) => session.title)).toEqual([
      "Fix login",
    ])
    expect(
      topSessions(sessions, from, from + 86_400, "fiveHour").map((session) => session.title),
    ).toEqual(["Fix login"])
  })
})
