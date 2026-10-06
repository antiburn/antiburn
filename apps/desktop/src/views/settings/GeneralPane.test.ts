import { describe, expect, it } from "vitest"

import type { ScanHistoryProgress, ScanStatus } from "../../lib/ipc"
import { byteLabel, historyScanSummary, scanSummary } from "./GeneralPane"

function status(overrides: Partial<ScanStatus> = {}): ScanStatus {
  return {
    running: false,
    completedAgents: 0,
    totalAgents: 0,
    sessions: 0,
    finishedAt: null,
    cancelled: false,
    error: null,
    agents: [],
    listChanged: false,
    reDescribed: 0,
    ...overrides,
  }
}

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

describe("scanSummary", () => {
  it("distinguishes every outcome a pass can have", () => {
    const finishedAt = new Date(Date.now() - 60_000).toISOString()
    expect(scanSummary(null)).toBe("Nothing has been scanned yet.")
    expect(scanSummary(status({ running: true }))).toBe("Scanning now…")
    expect(scanSummary(status({ finishedAt, error: "disk went away" }))).toBe(
      "The last scan did not finish.",
    )
    expect(scanSummary(status({ finishedAt, cancelled: true }))).toBe(
      "The last scan was stopped before it finished.",
    )
    expect(scanSummary(status({ finishedAt }))).toBe("Last scanned 1m ago.")
  })
})

function history(overrides: Partial<ScanHistoryProgress>): ScanHistoryProgress {
  return { state: "none", completed: 0, total: 0, ...overrides }
}

describe("historyScanSummary", () => {
  it("says nothing when there is no status, or retention keeps only the current window", () => {
    expect(historyScanSummary(undefined, false)).toBe("")
    expect(historyScanSummary(history({ state: "none" }), false)).toBe("")
  })

  it("tells a waiting reader the historical pass has not started yet", () => {
    expect(historyScanSummary(history({ state: "pending" }), false)).toBe(
      " It will also read your full history once the current scan is caught up.",
    )
  })

  it("tells a reader with monitoring paused how the historical pass can start", () => {
    expect(historyScanSummary(history({ state: "pending" }), true)).toBe(
      " It will read your full history when monitoring resumes, or when you scan now.",
    )
  })

  it("reports what the historical pass has found and processed so far, singular and plural", () => {
    expect(
      historyScanSummary(history({ state: "running", total: 1, completed: 0 }), false),
    ).toBe(" It has also found 1 older session so far, 0 processed.")
    expect(
      historyScanSummary(history({ state: "running", total: 40, completed: 12 }), false),
    ).toBe(" It has also found 40 older sessions so far, 12 processed.")
  })

  it("reports the final count once done, or nothing when there was no history to find", () => {
    expect(
      historyScanSummary(history({ state: "done", total: 40, completed: 40 }), false),
    ).toBe(" It has also processed 40 older sessions.")
    expect(historyScanSummary(history({ state: "done", total: 0, completed: 0 }), false)).toBe(
      "",
    )
  })
})
