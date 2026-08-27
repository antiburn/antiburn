// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { beforeEach, describe, expect, it, vi } from "vitest"

import type { InsightsReportPayload } from "../../lib/ipc"
import { InsightsSession } from "./InsightsSession"

/**
 * The session is the imperative boundary behind the Insights pane: it owns
 * the report IPC call, the in-flight and error state, and the cancel that
 * fires when the pane closes. These tests pin that ownership.
 */

const invoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

/** A synthetic empty report: nothing discovered, nothing assessed. */
function report(overrides: Partial<InsightsReportPayload> = {}): InsightsReportPayload {
  return {
    environmentKey: "native",
    windowStartEpoch: 100,
    windowEndEpoch: 200,
    computedAtEpoch: 200,
    coverage: {
      discovered: 0,
      unknownStart: 0,
      pending: 0,
      processing: 0,
      failed: 0,
      unsupported: 0,
      stale: 0,
      ready: 0,
      activelyGrowing: 0,
      awaitingProviderSupport: 0,
    },
    assessedSessions: 0,
    categories: [],
    quotaPressure: { assessed: false, findings: null },
    catalogRevision: 1,
    ...overrides,
  }
}

const STATUS = { calculating: false, pending: 0, processing: 0 }

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string) => {
    if (command in overrides) {
      const override = overrides[command]
      if (override instanceof Error) return Promise.reject(override)
      return Promise.resolve(override)
    }
    switch (command) {
      case "get_insights_report":
        return Promise.resolve(report())
      case "get_insights_status":
        return Promise.resolve(STATUS)
      default:
        return Promise.resolve(null)
    }
  })
}

async function flush() {
  await Promise.resolve()
  await Promise.resolve()
  await Promise.resolve()
}

beforeEach(() => {
  vi.clearAllMocks()
  mockCommands()
})

describe("InsightsSession", () => {
  it("loads the report on first subscribe and moves loading → ready", async () => {
    const session = new InsightsSession()
    const initial = session.getSnapshot()
    expect(initial.phase).toBe("loading")

    const unsubscribe = session.subscribe(() => {})
    await flush()

    const ready = session.getSnapshot()
    expect(ready.phase).toBe("ready")
    expect(ready.report?.environmentKey).toBe("native")
    expect(ready.status).toEqual(STATUS)
    // Snapshots are immutable: each update is a new object.
    expect(ready).not.toBe(initial)
    unsubscribe()
  })

  it("keeps the loading snapshot until the report resolves", async () => {
    let resolveReport: (value: InsightsReportPayload) => void = () => {}
    mockCommands({
      get_insights_report: undefined,
    })
    invoke.mockImplementation((command: string) => {
      if (command === "get_insights_report") {
        return new Promise((resolve) => {
          resolveReport = resolve
        })
      }
      if (command === "get_insights_status") return Promise.resolve(STATUS)
      return Promise.resolve(null)
    })

    const session = new InsightsSession()
    const unsubscribe = session.subscribe(() => {})
    await flush()

    // In flight: still loading, and nothing that could read as a result.
    expect(session.getSnapshot().phase).toBe("loading")
    expect(session.getSnapshot().report).toBeNull()

    resolveReport(report())
    await flush()
    expect(session.getSnapshot().phase).toBe("ready")
    unsubscribe()
  })

  it("yields an error snapshot on failure, never an empty report", async () => {
    mockCommands({ get_insights_report: new Error("synthetic failure") })

    const session = new InsightsSession()
    const unsubscribe = session.subscribe(() => {})
    await flush()

    const snapshot = session.getSnapshot()
    expect(snapshot.phase).toBe("error")
    expect(snapshot.error).toContain("synthetic failure")
    expect(snapshot.report).toBeNull()
    unsubscribe()
  })

  it("cancels the shell's report work when the last subscriber leaves", async () => {
    const session = new InsightsSession()
    const unsubscribe = session.subscribe(() => {})
    await flush()

    expect(invoke).not.toHaveBeenCalledWith("cancel_insights_report")
    unsubscribe()
    expect(invoke).toHaveBeenCalledWith("cancel_insights_report")
  })

  it("recomputes the report on refresh, passing through loading again", async () => {
    const session = new InsightsSession()
    const phases: string[] = []
    const unsubscribe = session.subscribe(() => {
      phases.push(session.getSnapshot().phase)
    })
    await flush()
    expect(session.getSnapshot().phase).toBe("ready")

    const calls = invoke.mock.calls.filter(
      ([command]) => command === "get_insights_report",
    ).length
    await session.refresh()
    await flush()

    expect(
      invoke.mock.calls.filter(([command]) => command === "get_insights_report").length,
    ).toBe(calls + 1)
    expect(phases).toContain("loading")
    expect(session.getSnapshot().phase).toBe("ready")
    unsubscribe()
  })

  it("a late result from a stopped session never mutates the next one", async () => {
    let resolveReport: (value: InsightsReportPayload) => void = () => {}
    invoke.mockImplementation((command: string) => {
      if (command === "get_insights_report") {
        return new Promise((resolve) => {
          resolveReport = resolve
        })
      }
      if (command === "get_insights_status") return Promise.resolve(STATUS)
      return Promise.resolve(null)
    })

    const session = new InsightsSession()
    const unsubscribe = session.subscribe(() => {})
    await flush()
    unsubscribe()

    resolveReport(report({ environmentKey: "stale" }))
    await flush()
    // The stopped generation's result is dropped.
    expect(session.getSnapshot().report).toBeNull()
    expect(session.getSnapshot().phase).toBe("loading")
  })
})
