// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../../lib/ipc"
import type { SessionAnalysisPayload } from "../../lib/ipc"
import { ANALYSIS_POLL_MS, PopoverSession, sessionKey } from "./PopoverSession"
import type { SessionSubject } from "./SessionPane"

const getSessionAnalysis = vi.hoisted(() => vi.fn())
const getSessionAnalysisFingerprint = vi.hoisted(() => vi.fn())
const getSubagentAnalysis = vi.hoisted(() => vi.fn())

// The analysis commands are overridden. All other wrappers keep their real
// no-shell fallback because `hasShell()` is false outside Tauri.
vi.mock("../../lib/ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof Ipc>()
  return { ...actual, getSessionAnalysis, getSessionAnalysisFingerprint, getSubagentAnalysis }
})

/**
 * `sessionKey` tags the analysis load a subject's payload belongs to. Get it
 * wrong and a subject that moves between environments — or a sub-agent whose
 * id happens to collide with one from a different parent — shows another
 * session's cached (or in-flight) analysis instead of its own.
 */

describe("sessionKey", () => {
  it("scopes by environment: the same agent and session id in different WSL distros are distinct", () => {
    const native = sessionKey({ agent: "claude-code", sessionId: "same-id", wslDistro: null })
    const ubuntu = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "Ubuntu",
    })
    const debian = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "Debian",
    })

    expect(native).not.toBe(ubuntu)
    expect(ubuntu).not.toBe(debian)
  })

  it("is case-insensitive on the WSL distribution name", () => {
    const lower = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "ubuntu",
    })
    const upper = sessionKey({
      agent: "claude-code",
      sessionId: "same-id",
      wslDistro: "UBUNTU",
    })

    expect(lower).toBe(upper)
  })

  it("scopes a sub-agent key by its parent session, not just the sub-agent id", () => {
    const parentOne = sessionKey({
      agent: "claude-code",
      sessionId: "same-subagent-id",
      wslDistro: null,
      subagent: { parentSessionId: "parent-one", subagentId: "same-subagent-id" },
    })
    const parentTwo = sessionKey({
      agent: "claude-code",
      sessionId: "same-subagent-id",
      wslDistro: null,
      subagent: { parentSessionId: "parent-two", subagentId: "same-subagent-id" },
    })

    expect(parentOne).not.toBe(parentTwo)
  })

  it("does not collide a sub-agent key with a top-level session of the same id", () => {
    const topLevel = sessionKey({
      agent: "claude-code",
      sessionId: "shared-id",
      wslDistro: null,
    })
    const subagent = sessionKey({
      agent: "claude-code",
      sessionId: "shared-id",
      wslDistro: null,
      subagent: { parentSessionId: "shared-id", subagentId: "sub-1" },
    })

    expect(topLevel).not.toBe(subagent)
  })
})

/**
 * The live poll behind an open detail pane: it re-loads the analysis only
 * when the transcript's fingerprint actually moves, and it never runs
 * against an empty stack.
 */
describe("PopoverSession live analysis poll", () => {
  const subject: SessionSubject = {
    agent: "claude-code",
    sessionId: "session-1",
    wslDistro: null,
  }

  const analysisPayload = (
    overrides: Partial<SessionAnalysisPayload> = {},
  ): SessionAnalysisPayload => ({
    summary: null,
    supportsAnalysis: true,
    title: null,
    wslDistro: null,
    isActive: false,
    cost: null,
    topLevelCost: null,
    subagentsCost: null,
    inclusiveTokens: null,
    subagentsTokens: null,
    efficiency: null,
    models: [],
    modelRuns: [],
    orchestration: null,
    relations: null,
    sourcePath: null,
    ...overrides,
  })

  const usableAnalysis = (): SessionAnalysisPayload =>
    analysisPayload({
      summary: {
        sessionCount: 1,
        avgDurationSecs: 1,
        avgActiveSecs: 1,
        tokensInTotal: 0,
        tokensOutTotal: 0,
        peakContextTokens: 0,
        contextWindow: 0,
        buckets: [],
        sessions: [],
      },
    })

  beforeEach(() => {
    vi.useFakeTimers()
    getSessionAnalysis.mockReset()
    getSessionAnalysis.mockResolvedValue(null)
    getSessionAnalysisFingerprint.mockReset()
    getSessionAnalysisFingerprint.mockResolvedValue("fingerprint-1")
    getSubagentAnalysis.mockReset()
    getSubagentAnalysis.mockResolvedValue(null)
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it("re-loads the analysis when the fingerprint changes on a tick", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    // Let `loadAnalysisFor` and the fingerprint seed that follows it settle,
    // so the poll's own baseline is in place before the first tick.
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionAnalysis).toHaveBeenCalledTimes(1)

    getSessionAnalysisFingerprint.mockResolvedValue("fingerprint-2")
    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("does not re-load the analysis when the fingerprint is unchanged", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)
    expect(getSessionAnalysis).toHaveBeenCalledTimes(1)

    // The mock keeps returning "fingerprint-1" from the seed above.
    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)
    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(1)
    unsubscribe()
  })

  it("re-loads on the next tick after an unavailable read, fingerprint unchanged", async () => {
    getSessionAnalysis.mockResolvedValue(analysisPayload())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("stops retrying once a usable analysis lands", async () => {
    getSessionAnalysis
      .mockResolvedValueOnce(analysisPayload())
      .mockResolvedValue(usableAnalysis())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS * 4)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("latches after three unavailable retries", async () => {
    getSessionAnalysis.mockResolvedValue(analysisPayload())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS * 12)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(4)
    unsubscribe()
  })

  it("re-arms the budget after a usable analysis settles", async () => {
    getSessionAnalysis
      .mockResolvedValueOnce(analysisPayload())
      .mockResolvedValueOnce(usableAnalysis())
      .mockResolvedValueOnce(analysisPayload())
      .mockResolvedValue(usableAnalysis())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)
    getSessionAnalysisFingerprint.mockResolvedValue("fingerprint-2")
    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)
    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(4)
    unsubscribe()
  })

  it("does not retry a sub-agent subject", async () => {
    getSubagentAnalysis.mockResolvedValue(analysisPayload())
    const subagent: SessionSubject = {
      ...subject,
      subagent: { parentSessionId: subject.sessionId, subagentId: "subagent-1" },
    }
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subagent)
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS * 3)
    expect(getSubagentAnalysis).toHaveBeenCalledTimes(1)

    getSessionAnalysisFingerprint.mockResolvedValue("fingerprint-2")
    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)

    expect(getSubagentAnalysis).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("does not retry when the agent does not support analysis", async () => {
    getSessionAnalysis.mockResolvedValue(analysisPayload({ supportsAnalysis: false }))
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS * 2)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(1)
    unsubscribe()
  })

  it("retries a rejected read", async () => {
    getSessionAnalysis.mockRejectedValueOnce(new Error("read failed"))
    getSessionAnalysis.mockResolvedValue(usableAnalysis())
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)

    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS)

    expect(getSessionAnalysis).toHaveBeenCalledTimes(2)
    unsubscribe()
  })

  it("stops polling once the detail pane closes", async () => {
    const session = new PopoverSession()
    const unsubscribe = session.subscribe(() => {})
    session.openSession(subject)
    await vi.advanceTimersByTimeAsync(0)
    getSessionAnalysisFingerprint.mockClear()

    session.goBack()
    await vi.advanceTimersByTimeAsync(ANALYSIS_POLL_MS * 3)

    expect(getSessionAnalysisFingerprint).not.toHaveBeenCalled()
    unsubscribe()
  })
})
