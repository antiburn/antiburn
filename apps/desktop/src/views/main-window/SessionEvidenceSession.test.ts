import { describe, expect, it, vi } from "vitest"
import type {
  EvidenceReference,
  SessionEvidenceContextResponse,
} from "../../lib/sessionEvidenceIpc"
import {
  SessionEvidenceSession,
  type SessionEvidenceDependencies,
} from "./SessionEvidenceSession"

export const evidenceReference: EvidenceReference = {
  key: "native:codex:fixture:1:2",
  environmentKey: "native",
  agent: "codex",
  sessionId: "fixture",
  sourceGeneration: 1,
  publishedFence: 1,
  turnRowId: 1,
  sourceKey: "source",
  threadId: "main",
  scope: "main",
  turnIndex: 2,
  partIndex: 0,
}
const response: SessionEvidenceContextResponse = {
  available: true,
  reason: null,
  session: null,
  previous: null,
  next: null,
  match: {
    reference: evidenceReference,
    kind: "user",
    text: "Recorded supporting text",
    truncated: false,
  },
}
function setup(
  fetch: SessionEvidenceDependencies["fetch"] = vi.fn().mockResolvedValue(response),
) {
  let updated = () => {}
  const stop = vi.fn()
  const deps: SessionEvidenceDependencies = {
    fetch,
    onSessionUpdated: vi.fn(async (handler) => {
      updated = handler
      return stop
    }),
    onSessionIndexChanged: vi.fn(async () => stop),
  }
  return {
    session: new SessionEvidenceSession(evidenceReference, deps),
    deps,
    updated: () => updated(),
    stop,
  }
}
async function settle() {
  for (let i = 0; i < 12; i++) await Promise.resolve()
}

describe("retained evidence reader", () => {
  it("reads only while observed and clears content on disposal", async () => {
    const { session, deps, stop } = setup()
    expect(deps.fetch).not.toHaveBeenCalled()
    const dispose = session.subscribe(() => {})
    await settle()
    expect(session.getSnapshot()).toEqual({ phase: "ready", context: response })
    dispose()
    expect(stop).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().context).toBeNull()
  })
  it("discards a stale in-flight response and revalidates after a source update", async () => {
    let release!: (response: SessionEvidenceContextResponse) => void
    const pending = new Promise<SessionEvidenceContextResponse>((resolve) => {
      release = resolve
    })
    const fetch = vi
      .fn()
      .mockReturnValueOnce(pending)
      .mockResolvedValue({ ...response, available: false, reason: "stale", match: null })
    const { session, updated } = setup(fetch)
    const dispose = session.subscribe(() => {})
    await settle()
    updated()
    release(response)
    await settle()
    expect(fetch).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().phase).toBe("unavailable")
    expect(session.getSnapshot().context?.match).toBeNull()
    dispose()
  })
  it("keeps stable content while revalidating unchanged evidence", async () => {
    const { session, updated } = setup()
    const dispose = session.subscribe(() => {})
    await settle()
    updated()
    expect(session.getSnapshot().phase).toBe("ready")
    await settle()
    expect(session.getSnapshot().phase).toBe("ready")
    dispose()
  })
  it("does not publish an old response after its view closes", async () => {
    let release!: (response: SessionEvidenceContextResponse) => void
    const pending = new Promise<SessionEvidenceContextResponse>((resolve) => {
      release = resolve
    })
    const { session } = setup(vi.fn().mockReturnValue(pending))
    const listener = vi.fn()
    const dispose = session.subscribe(listener)
    dispose()
    const calls = listener.mock.calls.length
    release(response)
    await settle()
    expect(listener).toHaveBeenCalledTimes(calls)
    expect(session.getSnapshot().context).toBeNull()
  })
  it("retries errors without inventing context", async () => {
    const { session } = setup(
      vi.fn().mockRejectedValueOnce(new Error("unavailable")).mockResolvedValue(response),
    )
    const dispose = session.subscribe(() => {})
    await settle()
    expect(session.getSnapshot()).toEqual({ phase: "error", context: null })
    session.retry()
    await settle()
    expect(session.getSnapshot().phase).toBe("ready")
    dispose()
  })
})
