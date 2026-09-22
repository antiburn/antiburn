import { describe, expect, it } from "vitest"

import { axisDayLabel, dayLabel } from "./chartDates"

describe("chartDates", () => {
  it("labels a reader-local date without a timezone shift", () => {
    expect(dayLabel("2026-09-14")).toBe("Mon 14 Sep")
    expect(axisDayLabel("2026-01-02")).toBe("2 Jan")
  })
})
