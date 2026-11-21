import { expect, it, vi } from "vitest"
import type { LocalToolsStatus } from "../lib/ipcPayloads"
import { CloudSession } from "./CloudSession"
import { cloudProjectFixture, localToolsStatus } from "./__tests__/cloudProjectFixtures"

it("observes tools before reading without blocking folders on a pending status read", async () => {
  const { port, ongoing, emitLocalTools } = cloudProjectFixture()
  let finish!: (status: LocalToolsStatus) => void
  const query = new Promise<LocalToolsStatus>((resolve) => {
    finish = resolve
  })
  vi.mocked(port.localTools.read).mockReturnValueOnce(query)
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(vi.mocked(port.localTools.watch).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(port.localTools.read).mock.invocationCallOrder[0]!,
    )
    expect(session.getSnapshot()).toMatchObject({ busy: false, error: null, localTools: null })
    const rendered = session.getSnapshot()
    emitLocalTools(localToolsStatus("pending", 1))
    emitLocalTools(localToolsStatus("unavailable", 2))
    await session.startSync(rendered)
    expect(ongoing.enable).toHaveBeenCalledOnce()
    finish(localToolsStatus("ready"))
    await query
    expect(session.getSnapshot().localTools?.phase).toBe("unavailable")
    expect(ongoing.groups).toHaveBeenCalledOnce()
  } finally {
    stop()
  }
})

it("does not block folder recovery while tool observation is pending", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  let finishWatch!: (stop: () => void) => void
  vi.mocked(port.localTools.watch).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finishWatch = resolve
      }),
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    emit({ setupError: "Synthetic setup failure" })
    emit({ setupError: null })
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loading).toBe(false))
    expect(port.localTools.watch).toHaveBeenCalledOnce()
    expect(port.localTools.read).not.toHaveBeenCalled()
    finishWatch(vi.fn())
    await vi.waitFor(() => expect(session.getSnapshot().localTools?.phase).toBe("unavailable"))
    expect(session.getSnapshot().error).toBeNull()
  } finally {
    stop()
  }
})

it.each([
  { events: ["ready"], reply: "pending", expected: "ready" },
  { events: ["ready", "pending"], reply: "ready", expected: "pending" },
  { events: ["failed"], reply: "ready", expected: "failed" },
  { events: ["ready", "failed"], reply: "ready", expected: "failed" },
] as const)(
  "keeps newer tool events ahead of a lower-revision query: %j",
  async ({ events, reply, expected }) => {
    const { port, emitLocalTools } = cloudProjectFixture()
    let finish!: (status: LocalToolsStatus) => void
    const query = new Promise<LocalToolsStatus>((resolve) => {
      finish = resolve
    })
    vi.mocked(port.localTools.read).mockReturnValueOnce(query)
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(port.localTools.read).toHaveBeenCalledOnce())
      for (const [index, phase] of events.entries())
        emitLocalTools(localToolsStatus(phase, index + 1))
      finish(localToolsStatus(reply))
      await query
      expect(session.getSnapshot().localTools).toEqual(
        localToolsStatus(expected, events.length),
      )
      expect(session.getSnapshot().localToolsError).toBe(false)
      expect(session.getSnapshot().error).toBeNull()
    } finally {
      stop()
    }
  },
)

it.each([
  { phase: "pending", revision: 4, expected: "ready", expectedRevision: 5 },
  { phase: "ready", revision: 5, expected: "ready", expectedRevision: 5 },
  { phase: "failed", revision: 6, expected: "failed", expectedRevision: 6 },
] as const)(
  "orders a successful query by revision after a ready event: %j",
  async ({ phase, revision, expected, expectedRevision }) => {
    const { port, emitLocalTools } = cloudProjectFixture()
    let finish!: (status: LocalToolsStatus) => void
    const query = new Promise<LocalToolsStatus>((resolve) => {
      finish = resolve
    })
    vi.mocked(port.localTools.read).mockReturnValueOnce(query)
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(port.localTools.read).toHaveBeenCalledOnce())
      emitLocalTools(localToolsStatus("ready", 5))
      const reply = localToolsStatus(phase, revision)
      finish(reply)
      await query
      expect(session.getSnapshot().localTools).toEqual(
        localToolsStatus(expected, expectedRevision),
      )
      if (revision >= 5) expect(session.getSnapshot().localTools).toBe(reply)
      expect(session.getSnapshot().localToolsError).toBe(false)
    } finally {
      stop()
    }
  },
)

it("does not replace a newer tool event with an older query failure", async () => {
  const { port, emitLocalTools } = cloudProjectFixture()
  let fail!: (error: Error) => void
  vi.mocked(port.localTools.read).mockImplementationOnce(
    () =>
      new Promise((_, reject) => {
        fail = reject
      }),
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(port.localTools.read).toHaveBeenCalledOnce())
    emitLocalTools(localToolsStatus("ready", 1))
    fail(new Error("Synthetic status read failure"))
    await Promise.resolve()
    expect(session.getSnapshot().localTools?.phase).toBe("ready")
    expect(session.getSnapshot().localToolsError).toBe(false)
  } finally {
    stop()
  }
})

it("ignores older native events after a newer status query or event", async () => {
  const { port, emitLocalTools } = cloudProjectFixture()
  vi.mocked(port.localTools.read).mockResolvedValue(localToolsStatus("ready", 5))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().localTools?.revision).toBe(5))
    emitLocalTools(localToolsStatus("failed", 4))
    expect(session.getSnapshot().localTools).toEqual(localToolsStatus("ready", 5))
    emitLocalTools(localToolsStatus("pending", 6))
    emitLocalTools(localToolsStatus("ready", 5))
    expect(session.getSnapshot().localTools).toEqual(localToolsStatus("pending", 6))
    emitLocalTools(localToolsStatus("failed", 7))
    emitLocalTools(localToolsStatus("ready", 6))
    expect(session.getSnapshot().localTools).toEqual(localToolsStatus("failed", 7))
  } finally {
    stop()
  }
})

it("ignores a lower-revision query even when no event arrives during that query", async () => {
  const { port } = cloudProjectFixture()
  vi.mocked(port.localTools.read).mockResolvedValueOnce(localToolsStatus("ready", 5))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().localTools?.revision).toBe(5))
    vi.mocked(port.localTools.read).mockResolvedValueOnce(localToolsStatus("pending", 4))
    await session.load()
    expect(port.localTools.read).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().localTools).toEqual(localToolsStatus("ready", 5))
  } finally {
    stop()
  }
})

it("does not let an ignored old event cancel a newer status read", async () => {
  const { port, emitLocalTools } = cloudProjectFixture()
  vi.mocked(port.localTools.read).mockResolvedValueOnce(localToolsStatus("ready", 5))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().localTools?.revision).toBe(5))
    let finish!: (status: LocalToolsStatus) => void
    const query = new Promise<LocalToolsStatus>((resolve) => {
      finish = resolve
    })
    vi.mocked(port.localTools.read).mockReturnValueOnce(query)
    const loading = session.load()
    await vi.waitFor(() => expect(port.localTools.read).toHaveBeenCalledTimes(2))
    emitLocalTools(localToolsStatus("pending", 4))
    finish(localToolsStatus("failed", 6))
    await query
    await loading
    expect(session.getSnapshot().localTools).toEqual(localToolsStatus("failed", 6))
  } finally {
    stop()
  }
})

it("keeps tool read failures separate from folders and accepts later recovery events", async () => {
  const { port, emitLocalTools } = cloudProjectFixture()
  vi.mocked(port.localTools.read).mockRejectedValueOnce(
    new Error("Synthetic status read failure"),
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().localToolsError).toBe(true))
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(session.getSnapshot().error).toBeNull()
    emitLocalTools(localToolsStatus("ready", 1))
    expect(session.getSnapshot().localTools).toEqual(localToolsStatus("ready", 1))
    expect(session.getSnapshot().localToolsError).toBe(false)
  } finally {
    stop()
  }
})

it("retries a failed tool subscription on a later load without blocking account work", async () => {
  const { port } = cloudProjectFixture()
  vi.mocked(port.localTools.watch).mockRejectedValueOnce(new Error("Synthetic event failure"))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().localToolsError).toBe(true))
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    expect(port.localTools.read).not.toHaveBeenCalled()
    await session.load()
    await vi.waitFor(() => expect(session.getSnapshot().localToolsError).toBe(false))
    expect(port.localTools.watch).toHaveBeenCalledTimes(2)
    expect(port.localTools.read).toHaveBeenCalledOnce()
  } finally {
    stop()
  }
})

it("rejects old tool reads and callbacks after observation restarts", async () => {
  const { port } = cloudProjectFixture()
  const oldStop = vi.fn()
  vi.mocked(port.localTools.watch).mockResolvedValueOnce(oldStop)
  let finish!: (status: LocalToolsStatus) => void
  const query = new Promise<LocalToolsStatus>((resolve) => {
    finish = resolve
  })
  vi.mocked(port.localTools.read)
    .mockReturnValueOnce(query)
    .mockResolvedValueOnce(localToolsStatus("failed", 2))
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  await vi.waitFor(() => expect(port.localTools.read).toHaveBeenCalledOnce())
  const oldListener = vi.mocked(port.localTools.watch).mock.calls[0]![0]
  stop()
  expect(oldStop).toHaveBeenCalledOnce()
  const stopAgain = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().localTools?.phase).toBe("failed"))
    oldListener(localToolsStatus("ready", 3))
    finish(localToolsStatus("ready", 3))
    await query
    expect(session.getSnapshot().localTools?.phase).toBe("failed")
    expect(port.localTools.watch).toHaveBeenCalledTimes(2)
  } finally {
    stopAgain()
  }
})

it("releases a late tool subscription after disposal without reading its status", async () => {
  const { port } = cloudProjectFixture()
  let finish!: (stop: () => void) => void
  const subscribed = new Promise<() => void>((resolve) => {
    finish = resolve
  })
  vi.mocked(port.localTools.watch).mockReturnValueOnce(subscribed)
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
  stop()
  const release = vi.fn()
  finish(release)
  await subscribed
  expect(release).toHaveBeenCalledOnce()
  expect(port.localTools.read).not.toHaveBeenCalled()
})
