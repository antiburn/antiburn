import { describe, expect, it } from "vitest"

import {
  activityDayLabel,
  countGroupedItems,
  groupActivityByDay,
  newestFirst,
  type ActivityFeedItem,
} from "./activityFeedGrouping"
import { activityDayAge, isWithinActivityDays } from "./activityWindow"

const NOW = new Date("2026-03-04T12:00:00.000Z")

/** An item `days` calendar days back, at midday so the timezone cannot flip it. */
function item(key: string, days: number, hour = 12, isActive = false): ActivityFeedItem {
  const at = new Date(NOW)
  at.setDate(at.getDate() - days)
  at.setHours(hour, 0, 0, 0)
  return { key, at: at.toISOString(), isActive }
}

describe("isWithinActivityDays", () => {
  it("includes today and the preceding days, exclusive of the window edge", () => {
    expect(isWithinActivityDays(item("a", 0).at, 7, NOW)).toBe(true)
    expect(isWithinActivityDays(item("a", 6).at, 7, NOW)).toBe(true)
    expect(isWithinActivityDays(item("a", 7).at, 7, NOW)).toBe(false)
  })

  it("excludes future and malformed timestamps", () => {
    const future = new Date(NOW.getTime() + 3_600_000).toISOString()
    expect(isWithinActivityDays(future, 7, NOW)).toBe(false)
    expect(isWithinActivityDays("not a date", 7, NOW)).toBe(false)
    expect(isWithinActivityDays("", 7, NOW)).toBe(false)
  })

  it("reports age in calendar days, or null when it cannot", () => {
    expect(activityDayAge(item("a", 0).at, NOW)).toBe(0)
    expect(activityDayAge(item("a", 3).at, NOW)).toBe(3)
    expect(activityDayAge("not a date", NOW)).toBeNull()
  })
})

describe("newestFirst", () => {
  it("sorts newest first without mutating the input", () => {
    const input = [item("old", 3), item("new", 0), item("mid", 1)]
    expect(newestFirst(input).map((i) => i.key)).toEqual(["new", "mid", "old"])
    expect(input.map((i) => i.key)).toEqual(["old", "new", "mid"])
  })

  it("sorts unparseable timestamps to the bottom deterministically", () => {
    const input = [{ key: "broken", at: "", isActive: false }, item("real", 1)]
    expect(newestFirst(input).map((i) => i.key)).toEqual(["real", "broken"])
  })
})

describe("activityDayLabel", () => {
  it("names the recent days in words and the rest by count", () => {
    expect(activityDayLabel(0)).toBe("Today")
    expect(activityDayLabel(1)).toBe("Yesterday")
    expect(activityDayLabel(3)).toBe("3 days ago")
  })
})

describe("groupActivityByDay", () => {
  it("puts active items before the calendar groups", () => {
    const groups = groupActivityByDay(
      [item("today", 0), item("active", 1, 12, true), item("yesterday", 1)],
      { days: 7, now: NOW },
    )

    expect(groups.map((group) => group.label)).toEqual(["Active", "Today", "Yesterday"])
    expect(groups[0]?.items.map((entry) => entry.key)).toEqual(["active"])
  })

  it("buckets by calendar day, newest day and newest item first", () => {
    const groups = groupActivityByDay(
      [item("yesterday-early", 1, 9), item("today", 0), item("yesterday-late", 1, 18)],
      { days: 7, now: NOW },
    )
    expect(groups.map((g) => g.label)).toEqual(["Today", "Yesterday"])
    expect(groups[1]?.items.map((i) => i.key)).toEqual(["yesterday-late", "yesterday-early"])
  })

  it("produces no group for an empty day", () => {
    const groups = groupActivityByDay([item("today", 0), item("older", 3)], {
      days: 7,
      now: NOW,
    })
    expect(groups.map((g) => g.label)).toEqual(["Today", "3 days ago"])
  })

  it("drops everything outside the window", () => {
    const groups = groupActivityByDay([item("inside", 2), item("outside", 9)], {
      days: 7,
      now: NOW,
    })
    expect(countGroupedItems(groups)).toBe(1)
    expect(groups[0]?.items[0]?.key).toBe("inside")
  })

  it("counts nothing for an empty list", () => {
    expect(groupActivityByDay([], { days: 7, now: NOW })).toEqual([])
    expect(countGroupedItems([])).toBe(0)
  })
})
