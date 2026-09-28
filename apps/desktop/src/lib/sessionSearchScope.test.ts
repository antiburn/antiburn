import { describe, expect, it } from "vitest"
import { activitySearchScope, searchScopeLabel } from "./sessionSearchScope"
import { isWithinActivityDays } from "../components/activity/activityWindow"

describe("session search calendar scope", () => {
  it.each([1, 7, 14])("matches activity calendar semantics for %i days", (days) => {
    const now = new Date(2026, 8, 25, 14, 20)
    const scope = activitySearchScope(days, now)
    for (let offset = -16; offset <= 1; offset++) {
      for (const hour of [0, 12, 23]) {
        const date = new Date(2026, 8, 25 + offset, hour)
        const epoch = date.getTime() / 1000
        expect(epoch >= scope.fromEpoch && epoch <= scope.throughEpoch).toBe(
          isWithinActivityDays(date.toISOString(), days, now),
        )
      }
    }
    expect(searchScopeLabel(scope)).toBe(`Last ${days} ${days === 1 ? "day" : "days"}`)
  })
  it("offers an explicit all-retained label", () =>
    expect(searchScopeLabel(null)).toBe("All retained content"))
})

it.each([new Date(2026, 2, 10, 0, 30), new Date(2026, 10, 3, 0, 30)])(
  "uses midnight across a daylight-saving boundary: %s",
  (now) => {
    const scope = activitySearchScope(7, now)
    const expected = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 6)
    expect(scope.fromEpoch).toBe(expected.getTime() / 1000)
    expect(isWithinActivityDays(expected.toISOString(), 7, now)).toBe(true)
    expect(
      isWithinActivityDays(new Date(expected.getTime() - 1000).toISOString(), 7, now),
    ).toBe(false)
  },
)
