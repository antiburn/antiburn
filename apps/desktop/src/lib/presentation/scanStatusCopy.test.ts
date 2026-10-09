import { describe, expect, it } from "vitest"

import type { ScanHistoryProgress } from "../ipc"
import { byteLabel, olderSessionsStatus } from "./scanStatusCopy"

describe("byteLabel", () => {
  it("scales to the unit a reader can judge", () => {
    expect(byteLabel(1024)).toBe("1.0 KB")
    expect(byteLabel(3_670_016)).toBe("3.5 MB")
    expect(byteLabel(52 * 1024 * 1024)).toBe("52 MB")
    expect(byteLabel(3 * 1024 ** 3)).toBe("3.0 GB")
  })

  it("says nothing rather than something wrong for an absent figure", () => {
    // A fresh install has no database file yet, and the shell reports zero
    // instead of failing a settings row over it.
    expect(byteLabel(0)).toBe("0 KB")
    expect(byteLabel(-1)).toBe("0 KB")
    expect(byteLabel(Number.NaN)).toBe("0 KB")
  })
})

function history(overrides: Partial<ScanHistoryProgress>): ScanHistoryProgress {
  return { state: "none", completed: 0, total: 0, passRunning: false, ...overrides }
}

describe("olderSessionsStatus", () => {
  it("says nothing before a status arrives", () => {
    expect(olderSessionsStatus(undefined, false)).toBe("")
  })

  it("says why there is nothing to read when retention keeps only the current window", () => {
    expect(olderSessionsStatus(history({ state: "none" }), false)).toBe(
      "Keep session data covers only the last 30 days",
    )
  })

  it("says when a pending pass starts, with monitoring on and paused", () => {
    expect(olderSessionsStatus(history({ state: "pending" }), false)).toBe(
      "Starts once checks finish",
    )
    expect(olderSessionsStatus(history({ state: "pending" }), true)).toBe(
      "Starts when monitoring resumes",
    )
  })

  it("tells looking for sessions apart from processing the ones found", () => {
    expect(olderSessionsStatus(history({ state: "running", passRunning: true }), false)).toBe(
      "Looking for older sessions…",
    )
    expect(
      olderSessionsStatus(history({ state: "running", total: 40, completed: 12 }), false),
    ).toBe("12 of 40 processed")
  })

  it("reports the final count once done, singular and plural, or that there were none", () => {
    expect(olderSessionsStatus(history({ state: "done", total: 1, completed: 1 }), false)).toBe(
      "1 older session processed",
    )
    expect(
      olderSessionsStatus(history({ state: "done", total: 40, completed: 40 }), false),
    ).toBe("40 older sessions processed")
    expect(olderSessionsStatus(history({ state: "done" }), false)).toBe(
      "No older sessions found",
    )
  })
})
