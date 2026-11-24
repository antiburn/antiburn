import { expect, it, vi } from "vitest"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

const discovery = {
  groups: [projectGroup()],
  sourceRoots: [
    { sourceKind: "claude_jsonl" as const, path: "/Example/claude/projects/studio" },
  ],
  nextCursor: null,
  partial: false,
}

it.each(["status", "groups"] as const)(
  "rechecks setup automatically and ignores the old %s reply",
  async (stage) => {
    const { port, ongoing, emit } = cloudProjectFixture()
    let finish!: () => void
    if (stage === "status") {
      ongoing.status.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = () => resolve(ongoingStatus({ projectChoices: { studio: false } }))
          }),
      )
    } else {
      ongoing.groups.mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            finish = () => resolve({ ...discovery, groups: [] })
          }),
      )
    }
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.waitFor(() => expect(finish).toBeTypeOf("function"))
      expect(session.getSnapshot().projects?.loading).toBe(true)
      const healthy = session.getSnapshot().cloud
      vi.mocked(port.read).mockResolvedValue(healthy)
      emit({ setupError: "Synthetic setup failure" })
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      expect(session.getSnapshot().projects).toMatchObject({
        loaded: true,
        loading: false,
        choices: { studio: true },
        groups: [expect.objectContaining({ id: "studio", captureSupported: true })],
      })
      finish()
      await Promise.resolve()
      expect(session.getSnapshot().projects).toMatchObject({
        loading: false,
        choices: { studio: true },
        groups: [expect.objectContaining({ id: "studio" })],
      })
      expect(ongoing.groups).toHaveBeenCalledTimes(stage === "groups" ? 2 : 1)
      expect(ongoing.enable).not.toHaveBeenCalled()
      expect(port.save).not.toHaveBeenCalled()
    } finally {
      stop()
    }
  },
)

it("keeps cached exclusions through a setup failure and automatic recheck", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  ongoing.status.mockResolvedValue(ongoingStatus({ projectChoices: { studio: false } }))
  ongoing.groups.mockResolvedValue({ ...discovery, partial: true })
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    let finish!: () => void
    ongoing.groups.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = () => resolve({ ...discovery, groups: [] })
        }),
    )
    const pending = session.loadProjects()
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(4))
    vi.mocked(port.read).mockResolvedValue(session.getSnapshot().cloud)
    let finishRecovery!: () => void
    ongoing.groups.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishRecovery = () => resolve(discovery)
        }),
    )
    emit({ setupError: "Synthetic setup failure" })
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(5))
    finish()
    await pending
    expect(session.getSnapshot().projects).toMatchObject({
      loading: true,
      partial: true,
      choices: { studio: false },
      groups: [expect.objectContaining({ id: "studio" })],
    })
    finishRecovery()
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loading).toBe(false))
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      choices: { studio: false },
      groups: [expect.objectContaining({ id: "studio" })],
    })
    expect(ongoing.groups).toHaveBeenCalledTimes(5)
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(port.save).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("rechecks a partial folder result with rows without enabling uploads", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValueOnce({ ...discovery, partial: true })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects?.loading).toBe(true)

    await vi.advanceTimersByTimeAsync(150)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      partial: false,
      groups: [expect.objectContaining({ id: "studio" })],
    })
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("keeps usable partial rows after two empty partial retries", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups
    .mockResolvedValue({ ...discovery, groups: [], sourceRoots: [], partial: true })
    .mockResolvedValueOnce({ ...discovery, partial: true })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(1_000)
    expect(ongoing.groups).toHaveBeenCalledTimes(3)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      partial: true,
      groups: [expect.objectContaining({ id: "studio", captureSupported: true })],
    })
    expect(session.getSnapshot().projects?.retrying).not.toBe(true)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("ignores an account read failure after a newer signed-in event", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  const read = vi.mocked(port.read)
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    const healthy = session.getSnapshot().cloud
    read.mockRejectedValue(new Error("Synthetic account read failure"))
    const pending = session.load()
    emit({ ...healthy, identity: { ...healthy.identity!, login: "Newer account event" } })
    await vi.advanceTimersByTimeAsync(1_000)
    await pending
    expect(session.getSnapshot()).toMatchObject({
      busy: false,
      error: null,
      cloud: { identity: { login: "Newer account event" } },
    })
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("reloads folders immediately when an event clears a setup error during backoff", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    emit({ setupError: "Synthetic setup failure" })
    await vi.advanceTimersByTimeAsync(32_000)
    expect(session.getSnapshot().projects?.retryStage).toBe("setup")
    expect(ongoing.groups).toHaveBeenCalledOnce()
    const reads = vi.mocked(port.read).mock.calls.length

    emit({ setupError: null })
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects).toMatchObject({ loaded: true, loading: false })

    await vi.advanceTimersByTimeAsync(30_000)
    expect(port.read).toHaveBeenCalledTimes(reads)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("cancels setup backoff when recovery arrives during an account reload", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  const read = vi.mocked(port.read)
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    const healthy = session.getSnapshot().cloud
    emit({ setupError: "Synthetic setup failure" })
    await vi.advanceTimersByTimeAsync(32_000)
    expect(session.getSnapshot().projects?.retryStage).toBe("setup")

    let finishAccountRead!: () => void
    read.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishAccountRead = () => resolve(healthy)
        }),
    )
    const pending = session.load()
    expect(session.getSnapshot().busy).toBe(true)
    emit({ setupError: null })
    expect(ongoing.groups).toHaveBeenCalledOnce()

    finishAccountRead()
    await pending
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects).toMatchObject({ loaded: true, loading: false })
    const reads = read.mock.calls.length
    await vi.advanceTimersByTimeAsync(30_000)
    expect(read).toHaveBeenCalledTimes(reads)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it.each(["sign-out", "source change", "pane exit", "pane hidden"] as const)(
  "cancels a scheduled retry on %s",
  async (change) => {
    const { port, ongoing, emit } = cloudProjectFixture()
    ongoing.groups.mockRejectedValueOnce(new Error("Synthetic file read failure"))
    vi.useFakeTimers()
    const session = new CloudSession(port, false, true)
    const stop = session.subscribe(vi.fn())
    try {
      await vi.advanceTimersByTimeAsync(0)
      expect(ongoing.groups).toHaveBeenCalledOnce()
      if (change === "sign-out") {
        emit({ status: "signed_out", identity: null, destination: null })
      } else if (change === "source change") {
        emit({
          draft: {
            screen: "folders",
            folders: [{ path: "/Example/OldSource", selected: false }],
          },
        })
      } else if (change === "pane exit") stop()
      else session.setVisible(false)
      await vi.advanceTimersByTimeAsync(90_000)
      expect(ongoing.groups).toHaveBeenCalledTimes(change === "source change" ? 2 : 1)
      expect(session.getSnapshot().projects?.loaded ?? false).toBe(change === "source change")
      expect(ongoing.enable).not.toHaveBeenCalled()
    } finally {
      if (change !== "pane exit") stop()
      vi.useRealTimers()
    }
  },
)

it("keeps checking after more than two failures and caps its delay at 30 seconds", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const reads: number[] = []
  ongoing.groups.mockImplementation(async () => {
    reads.push(Date.now())
    if (reads.length < 10) throw new Error("Synthetic folder read failure")
    return discovery
  })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(600)
    expect(session.getSnapshot().projects).toMatchObject({
      loading: true,
      retrying: true,
      error: null,
    })
    await vi.advanceTimersByTimeAsync(100_000)
    expect(reads).toHaveLength(10)
    expect(reads[9]! - reads[8]!).toBe(30_000)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      error: null,
    })
    await vi.advanceTimersByTimeAsync(100_000)
    expect(reads).toHaveLength(10)
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("stops when Desktop rejects the exact account scope", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockRejectedValueOnce("Cloud account changed.")
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(60_000)
    window.dispatchEvent(new Event("focus"))
    await vi.advanceTimersByTimeAsync(60_000)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects?.error).toMatch(/Cloud access changed/)
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("requires a new rendered approval after setup fails and recovers", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    const rendered = session.getSnapshot()
    emit({ setupError: "Synthetic setup failure" })
    emit({ setupError: null })
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loading).toBe(false))
    await session.startSync(rendered)
    expect(ongoing.enable).not.toHaveBeenCalled()
    await session.startSync(session.getSnapshot())
    expect(ongoing.enable).toHaveBeenCalledOnce()
    expect(ongoing.enable).toHaveBeenCalledWith(
      expect.objectContaining({
        selectedSources: discovery.sourceRoots,
        projects: projectGroup().projects,
      }),
    )
  } finally {
    stop()
  }
})
