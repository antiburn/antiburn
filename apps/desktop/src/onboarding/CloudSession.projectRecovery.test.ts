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

it("continues the same page once without blocking usable folder choices", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const incomplete = { ...discovery, partial: true, retrySamePage: true }
  const laterFolder = projectGroup({
    id: "later",
    path: "/Example/Later",
    label: "Later",
    folderPaths: ["/Example/Later"],
    projects: [],
    captureSupported: false,
  })
  let finish!: () => void
  ongoing.groups.mockResolvedValueOnce(incomplete).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        finish = () => resolve({ ...incomplete, groups: [...incomplete.groups, laterFolder] })
      }),
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"))
    expect(session.getSnapshot().projects).toMatchObject({
      loading: false,
      choices: { studio: true },
    })
    session.selectProject("studio", false)
    expect(session.getSnapshot().projects?.choices).toEqual({ studio: false })
    finish()
    await vi.waitFor(() => expect(session.getSnapshot().projects?.retrying).toBe(false))
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(ongoing.groups.mock.calls[1]).toEqual(ongoing.groups.mock.calls[0])
    expect(session.getSnapshot().projects?.groups.map(({ id }) => id)).toContain("later")
    expect(session.getSnapshot().projects?.choices.studio).toBe(false)
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("shows later-page rows while the same-page continuation keeps paging pending", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const cursor = { environment: "native", cwd: "/Example/Studio" }
  const secondPage = {
    ...discovery,
    groups: [projectGroup({ id: "later" })],
    partial: true,
    retrySamePage: true,
  }
  let finish!: () => void
  ongoing.groups
    .mockResolvedValueOnce({ ...discovery, nextCursor: cursor })
    .mockResolvedValueOnce(secondPage)
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = () => resolve({ ...secondPage, partial: false, retrySamePage: false })
        }),
    )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => {
      expect(session.getSnapshot().projects?.hasMore).toBe(true)
      expect(session.getSnapshot().projects?.paging).toBeUndefined()
    })
    const paging = session.loadMoreProjects()
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"))
    expect(session.getSnapshot().projects).toMatchObject({ hasMore: true, paging: "pending" })
    expect(session.getSnapshot().projects?.groups.map(({ id }) => id)).toContain("later")
    expect(ongoing.groups.mock.calls[2]).toEqual(ongoing.groups.mock.calls[1])
    finish()
    await paging
    expect(session.getSnapshot().projects).toMatchObject({ hasMore: false, paging: undefined })
  } finally {
    stop()
  }
})

it("resumes an interrupted later page before reading its next cursor", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const cursor = { environment: "native", cwd: "/Example/Studio" }
  const nextCursor = { environment: "native", cwd: "/Example/Later" }
  const complete = { ...discovery, nextCursor }
  let finish!: () => void
  ongoing.groups
    .mockResolvedValueOnce({ ...discovery, nextCursor: cursor })
    .mockResolvedValueOnce({ ...complete, partial: true, retrySamePage: true })
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = () => resolve(complete)
        }),
    )
    .mockResolvedValueOnce(complete)
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    const interrupted = session.loadMoreProjects()
    await vi.advanceTimersByTimeAsync(30_000)
    await interrupted
    expect(session.getSnapshot().projects?.paging).toBe("pending")
    finish()
    await vi.advanceTimersByTimeAsync(0)
    expect(session.getSnapshot().projects?.paging).toBeUndefined()

    await session.loadMoreProjects()
    expect(ongoing.groups.mock.calls.slice(1).map(([, after]) => after)).toEqual([
      cursor,
      cursor,
      cursor,
    ])
  } finally {
    stop()
    vi.useRealTimers()
  }
})

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
      const groupReads = ongoing.groups.mock.calls.length
      emit({ setupError: "Synthetic setup failure" })
      expect(ongoing.groups).toHaveBeenCalledTimes(groupReads)
      finish()
      await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
      expect(session.getSnapshot().projects).toMatchObject({
        loaded: true,
        loading: false,
        choices: { studio: true },
        groups: [expect.objectContaining({ id: "studio", captureSupported: true })],
      })
      expect(ongoing.groups).toHaveBeenCalledTimes(2)
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
    await vi.waitFor(() => {
      expect(session.getSnapshot().projects?.loaded).toBe(true)
      expect(session.getSnapshot().projects?.retrying).toBe(false)
    })
    let finish!: () => void
    ongoing.groups.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = () => resolve({ ...discovery, groups: [] })
        }),
    )
    const pending = session.loadProjects()
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
    vi.mocked(port.read).mockResolvedValue(session.getSnapshot().cloud)
    let finishRecovery!: () => void
    ongoing.groups.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishRecovery = () => resolve(discovery)
        }),
    )
    emit({ setupError: "Synthetic setup failure" })
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    finish()
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(3))
    await pending
    expect(session.getSnapshot().projects).toMatchObject({
      loading: false,
      retrying: true,
      partial: true,
      choices: { studio: false },
      groups: [expect.objectContaining({ id: "studio" })],
    })
    finishRecovery()
    await vi.waitFor(() => expect(session.getSnapshot().projects?.retrying).toBe(false))
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      choices: { studio: false },
      groups: [expect.objectContaining({ id: "studio" })],
    })
    expect(ongoing.groups).toHaveBeenCalledTimes(3)
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(port.save).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("accepts usable partial rows immediately and rechecks on focus", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValueOnce({ ...discovery, partial: true })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      retrying: false,
      partial: true,
    })

    await vi.advanceTimersByTimeAsync(7_000)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    window.dispatchEvent(new Event("focus"))
    await vi.advanceTimersByTimeAsync(0)
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

it("keeps usable cached rows without retrying an empty partial refresh", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups
    .mockResolvedValue({ ...discovery, groups: [], sourceRoots: [], partial: true })
    .mockResolvedValueOnce({ ...discovery, partial: true })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    await session.loadProjects()
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
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

it("rechecks an empty partial listing once and settles without another retry", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({ ...discovery, groups: [], sourceRoots: [], partial: true })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(150)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      partial: true,
      groups: [],
    })
    await vi.advanceTimersByTimeAsync(7_000)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
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
    await vi.advanceTimersByTimeAsync(600)
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
    await vi.advanceTimersByTimeAsync(600)
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

it("ends automatic retries and can recover on a new check", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const reads: number[] = []
  ongoing.groups.mockImplementation(async () => {
    reads.push(Date.now())
    if (reads.length <= 5) throw new Error("Synthetic folder read failure")
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
    expect(reads).toHaveLength(5)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: false,
      loading: false,
      retrying: false,
      error: "Could not check session folders.",
    })
    await session.loadProjects()
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      error: null,
    })
    await vi.advanceTimersByTimeAsync(100_000)
    expect(reads).toHaveLength(6)
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("stops when Desktop rejects the exact account scope", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockRejectedValue("Cloud account changed.")
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(60_000)
    window.dispatchEvent(new Event("focus"))
    await vi.advanceTimersByTimeAsync(60_000)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects?.error).toMatch(/Cloud access changed/)
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("retries a local authority failure once after workspace verification", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finishWorkspaceCheck!: () => void
  vi.mocked(port.workspaces!).mockImplementationOnce(
    () => new Promise((resolve) => (finishWorkspaceCheck = () => resolve(null))),
  )
  ongoing.groups.mockRejectedValueOnce("Cloud account changed.")
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(600)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: false,
      loading: true,
      scopeInvalid: false,
      error: null,
    })
    finishWorkspaceCheck()
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      error: null,
    })
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("ends cold folder progress after seven seconds but accepts a late valid page", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: () => void
  ongoing.groups.mockImplementationOnce(
    () => new Promise((resolve) => (finish = () => resolve(discovery))),
  )
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(7_000)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: false,
      loading: false,
      retrying: false,
      error: "Could not check session folders.",
    })
    await session.startSync()
    expect(ongoing.enable).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(1_000)
    finish()
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      retrying: false,
      error: null,
      choices: { studio: true },
    })
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("reuses a pending workspace check without setting global busy", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const workspaces = vi.mocked(port.workspaces!)
  let finish!: () => void
  workspaces.mockImplementationOnce(
    () => new Promise((resolve) => (finish = () => resolve(null))),
  )
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(7_000)
    expect(session.getSnapshot()).toMatchObject({
      busy: false,
      projects: { loading: false, error: "Could not check Cloud setup or workspaces." },
    })
    const retry = session.retryFolderScope()
    expect(session.retryFolderScope()).toBe(retry)
    await vi.advanceTimersByTimeAsync(1_000)
    expect(port.read).toHaveBeenCalledOnce()
    expect(workspaces).toHaveBeenCalledOnce()
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().busy).toBe(false)

    finish()
    await retry
    expect(session.getSnapshot()).toMatchObject({
      busy: false,
      projects: { loaded: true, loading: false, error: null },
    })
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("shows local rows before verifying workspaces without selecting or saving them", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finishWorkspaceCheck!: (
    workspaces: Awaited<ReturnType<NonNullable<typeof port.workspaces>>>,
  ) => void
  vi.mocked(port.workspaces!).mockImplementationOnce(
    () => new Promise((resolve) => (finishWorkspaceCheck = resolve)),
  )
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() =>
      expect(session.getSnapshot().projects).toMatchObject({
        loaded: false,
        loading: true,
        choices: { studio: false },
        groups: [expect.objectContaining({ id: "studio" })],
      }),
    )
    expect(ongoing.groups).toHaveBeenCalledOnce()
    const provisional = session.getSnapshot()
    session.selectProject("studio", true)
    await session.startSync(provisional)
    expect(ongoing.enable).not.toHaveBeenCalled()

    finishWorkspaceCheck([
      { id: "synthetic-personal", name: "Personal", kind: "personal" },
      { id: "synthetic-team", name: "Team", kind: "team" },
    ])
    await vi.waitFor(() =>
      expect(session.getSnapshot().projects).toMatchObject({
        loaded: true,
        loading: false,
        choices: { studio: false },
      }),
    )
    session.selectProject("studio", true, "synthetic-personal")
    await session.startSync(provisional)
    expect(ongoing.enable).not.toHaveBeenCalled()
    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledOnce()
  } finally {
    stop()
  }
})

it("blocks approval if a new workspace check denies access to cached rows", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.waitFor(() => expect(session.getSnapshot().projects?.loaded).toBe(true))
    const rendered = session.getSnapshot()
    vi.mocked(port.workspaces!).mockRejectedValueOnce("Cloud workspace access denied.")
    await session.loadProjects()
    expect(session.getSnapshot().projects).toMatchObject({
      loading: false,
      scopeInvalid: true,
      error: "You do not have access to the selected Cloud workspace.",
    })
    await session.startSync(rendered)
    await session.startSync()
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
  }
})

it("keeps verified choices usable after a workspace refresh times out", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const workspaces = vi.mocked(port.workspaces!)
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    const rendered = session.getSnapshot()
    workspaces.mockImplementationOnce(() => new Promise(() => {}))
    const refresh = session.loadProjects()
    await vi.advanceTimersByTimeAsync(30_000)
    await refresh
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      retrying: false,
      scopeInvalid: false,
      error: null,
      refreshError: "Could not check Cloud setup or workspaces.",
      choices: { studio: true },
    })
    expect(ongoing.enable).not.toHaveBeenCalled()

    const save = session.startSync(rendered)
    await vi.advanceTimersByTimeAsync(1_000)
    await save
    expect(ongoing.enable).toHaveBeenCalledOnce()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("keeps cached choices usable when a folder refresh hangs without dispatching another read", async () => {
  const { port, ongoing } = cloudProjectFixture()
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true)
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    let finish!: (page: typeof discovery) => void
    ongoing.groups.mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    const refresh = session.loadProjects()
    await vi.advanceTimersByTimeAsync(0)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      retrying: true,
      choices: { studio: true },
    })
    expect(ongoing.enable).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(30_000)
    await refresh
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      retrying: false,
      error: null,
      refreshError: "Could not check session folders.",
    })
    const recheck = session.loadProjects()
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)

    await session.startSync()
    expect(ongoing.enable).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ projects: projectGroup().projects }),
    )
    finish({ ...discovery, groups: [] })
    await recheck
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects?.groups.map(({ id }) => id)).toEqual(["studio"])
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
