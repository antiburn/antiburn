import { expect, it, vi } from "vitest"
import type { CloudOngoingPort, DesktopCloudProjectGroup } from "../lib/cloud"
import { CloudSession } from "./CloudSession"
import {
  cloudProjectFixture,
  ongoingStatus,
  projectGroup,
} from "./__tests__/cloudProjectFixtures"

type ProjectPage = Awaited<ReturnType<CloudOngoingPort["groups"]>>

const firstCursor = { environment: "native", cwd: "/Example/First" }
const secondCursor = { environment: "native", cwd: "/Example/Second" }
const sourceRoot = {
  sourceKind: "claude_jsonl" as const,
  path: "/Example/claude/projects/studio",
}

function projectPage(
  groups: DesktopCloudProjectGroup[],
  nextCursor: ProjectPage["nextCursor"] = null,
): ProjectPage {
  return { groups, sourceRoots: [sourceRoot], nextCursor, partial: false }
}

it("loads every Settings page in cursor order without saving folder choices", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const first = projectGroup({ sessionCount: 3, captureSessionCount: 99 })
  const later = projectGroup({ id: "later", sessionCount: 8 })
  ongoing.groups
    .mockResolvedValueOnce(projectPage([first], firstCursor))
    .mockResolvedValueOnce(projectPage([later], secondCursor))
    .mockResolvedValueOnce(projectPage([{ ...first, sessionCount: 4 }]))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      firstCursor,
      secondCursor,
    ])
    expect(ongoing.status).toHaveBeenCalledOnce()
    expect(port.workspaces).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      hasMore: false,
      paging: undefined,
      groups: [
        { id: "studio", sessionCount: 7 },
        { id: "later", sessionCount: 8 },
      ],
    })
    expect(ongoing.enable).not.toHaveBeenCalled()
    expect(ongoing.setProject).not.toHaveBeenCalled()
    expect(ongoing.backfill).not.toHaveBeenCalled()
    expect(port.save).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("keeps verified Settings choices usable while later folders load", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (page: ProjectPage) => void
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      loading: false,
      paging: "loading",
      groups: [{ id: "studio", sessionCount: null }],
    })
    session.selectProject("studio", false)
    finish(projectPage([projectGroup({ id: "later" })]))
    await vi.advanceTimersByTimeAsync(0)
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      paging: undefined,
      choices: { studio: false, later: true },
    })
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it.each(["same", "changed"] as const)(
  "refreshes the full Settings chain with %s page boundaries without a smaller intermediate count",
  async (boundary) => {
    const { port, ongoing } = cloudProjectFixture()
    const first = projectGroup({ sessionCount: 3 })
    const oldWorktree = "/Example/old-worktree"
    const oldContribution = {
      ...first,
      sessionCount: 4,
      folderPaths: [...first.folderPaths, oldWorktree],
      projects: [...first.projects, { ...first.projects[0]!, projectPath: oldWorktree }],
    }
    const later = projectGroup({ id: "later", sessionCount: 8 })
    let finish!: (page: ProjectPage) => void
    ongoing.groups
      .mockResolvedValueOnce(projectPage([first], firstCursor))
      .mockResolvedValueOnce(projectPage([oldContribution, later]))
      .mockResolvedValueOnce(
        projectPage([{ ...first, sessionCount: 5 }], {
          ...firstCursor,
          cwd: boundary === "changed" ? "/Example/NewBoundary" : firstCursor.cwd,
        }),
      )
      .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    vi.useFakeTimers()
    const session = new CloudSession(port, false, true, "automatic")
    const counts: (number | null | undefined)[] = []
    const stop = session.subscribe(() =>
      counts.push(
        session.getSnapshot().projects?.groups.find(({ id }) => id === "studio")?.sessionCount,
      ),
    )
    try {
      await vi.advanceTimersByTimeAsync(0)
      expect(session.getSnapshot().projects?.groups[0]?.sessionCount).toBe(7)
      const previousRevision = session.getSnapshot().contextRevision
      counts.length = 0
      const refresh = session.loadProjects()
      await vi.advanceTimersByTimeAsync(0)
      expect(session.getSnapshot().projects).toMatchObject({
        paging: "loading",
        groups: [
          { id: "studio", sessionCount: 7 },
          {
            id: "later",
            discoveryStale: boundary === "changed",
            captureSupported: boundary === "same",
          },
        ],
      })
      expect(
        session.getSnapshot().projects?.groups[0]?.memberPaths?.includes(oldWorktree),
      ).toBe(boundary === "same")
      if (boundary === "changed")
        expect(session.getSnapshot().contextRevision).toBeGreaterThan(previousRevision)
      finish(projectPage([{ ...first, sessionCount: 6 }, later]))
      await refresh
      expect(ongoing.groups).toHaveBeenCalledTimes(4)
      expect(session.getSnapshot().projects).toMatchObject({
        hasMore: false,
        groups: [
          { id: "studio", sessionCount: 11 },
          { id: "later", sessionCount: 8, discoveryStale: false },
        ],
      })
      expect(counts.every((count) => count === 7 || count === 11)).toBe(true)
    } finally {
      stop()
      vi.useRealTimers()
    }
  },
)

it("continues through an empty partial Settings page to its next cursor", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockResolvedValueOnce({
      ...projectPage([], secondCursor),
      sourceRoots: [],
      partial: true,
    })
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "later" })]))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      firstCursor,
      secondCursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      partial: true,
      groups: [{ id: "studio" }, { id: "later" }],
    })
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("stops Settings paging after the bounded same-page recheck and exposes recovery", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValue({
    ...projectPage([projectGroup()]),
    partial: true,
    retrySamePage: true,
  })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(150)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      hasMore: true,
      paging: undefined,
      error: null,
      refreshError: "Could not check session folders.",
    })
    await vi.advanceTimersByTimeAsync(30_000)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    session.selectProject("studio", false)
    expect(session.getSnapshot().projects?.choices.studio).toBe(false)
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("retries a failed Settings page without skipping or duplicating its cursor", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockRejectedValueOnce(new Error("Synthetic folder read failure"))
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "later" })]))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(150)
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      firstCursor,
      firstCursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      error: null,
      groups: [{ id: "studio" }, { id: "later" }],
    })
    expect(session.getSnapshot().projects?.refreshError).toBeUndefined()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("rechecks a cached deferred later page when Settings retries folder discovery", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const first = projectPage([projectGroup()], firstCursor)
  const deferred = {
    ...projectPage([projectGroup({ id: "later" })]),
    partial: true,
    retrySamePage: true,
  }
  ongoing.groups
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce(deferred)
    .mockResolvedValueOnce(deferred)
    .mockResolvedValueOnce(first)
    .mockResolvedValueOnce({ ...deferred, retrySamePage: false, partial: false })
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(150)
    expect(session.getSnapshot().projects?.refreshError).toBe(
      "Could not check session folders.",
    )
    await session.retryFolderScope()
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      firstCursor,
      firstCursor,
      null,
      firstCursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      paging: undefined,
      groups: [{ id: "studio" }, { id: "later" }],
    })
    expect(session.getSnapshot().projects?.refreshError).toBeUndefined()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("keeps loaded Settings folders and reports a failed later page after retries stop", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups
    .mockRejectedValue(new Error("Synthetic folder read failure"))
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(3_550)
    expect(ongoing.groups).toHaveBeenCalledTimes(6)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      hasMore: true,
      paging: undefined,
      error: null,
      refreshError: "Could not check session folders.",
      groups: [{ id: "studio" }],
    })
    await vi.advanceTimersByTimeAsync(30_000)
    expect(ongoing.groups).toHaveBeenCalledTimes(6)
    session.selectProject("studio", false)
    expect(session.getSnapshot().projects?.choices.studio).toBe(false)
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("resumes Settings after a hidden pending page releases without overlapping reads", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (page: ProjectPage) => void
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "later" })], secondCursor))
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "last" })]))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    session.setVisible(false)
    await vi.advanceTimersByTimeAsync(0)
    session.setVisible(true)
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    finish(projectPage([projectGroup({ id: "ignored" })], secondCursor))
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      firstCursor,
      firstCursor,
      secondCursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      paging: undefined,
      groups: [{ id: "studio" }, { id: "later" }, { id: "last" }],
    })
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it.each(["same", "changed"] as const)(
  "checks the %s account before resuming Settings paging after a save",
  async (account) => {
    const { port, ongoing, emit } = cloudProjectFixture()
    const savedPolicy = ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projectChoices: { studio: false },
    })
    ongoing.status.mockResolvedValue(savedPolicy)
    let finishPage!: (page: ProjectPage) => void
    let finishSave!: (status: typeof savedPolicy) => void
    ongoing.groups
      .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
      .mockImplementationOnce(() => new Promise((resolve) => (finishPage = resolve)))
      .mockResolvedValueOnce(projectPage([projectGroup({ id: "later" })]))
    ongoing.setProject.mockImplementationOnce(
      () => new Promise((resolve) => (finishSave = resolve)),
    )
    vi.useFakeTimers()
    const session = new CloudSession(port, false, true, "automatic")
    const stop = session.subscribe(vi.fn())
    try {
      await vi.advanceTimersByTimeAsync(0)
      const save = session.setSavedProject(session.getSnapshot(), "studio", true)
      await vi.advanceTimersByTimeAsync(0)
      expect(ongoing.setProject).toHaveBeenCalledOnce()
      expect(session.getSnapshot().busy).toBe(true)
      finishPage(projectPage([projectGroup({ id: "ignored" })], secondCursor))
      await vi.advanceTimersByTimeAsync(0)
      expect(session.getSnapshot().busy).toBe(true)
      expect(session.getSnapshot().projects?.paging).toBeUndefined()
      expect(ongoing.groups).toHaveBeenCalledTimes(2)
      if (account === "changed")
        emit({
          identity: { id: "second-account", login: "Second" },
          destination: { id: "second-personal", kind: "personal", name: "Personal" },
        })
      finishSave({
        ...savedPolicy,
        projects: projectGroup().projects,
        projectChoices: { studio: true },
      })
      await save
      await vi.advanceTimersByTimeAsync(0)
      expect(session.getSnapshot().busy).toBe(false)
      if (account === "changed") {
        expect(ongoing.groups).toHaveBeenCalledTimes(2)
        expect(session.getSnapshot().projects).toBeNull()
      } else {
        expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
          null,
          firstCursor,
          firstCursor,
        ])
        expect(session.getSnapshot().projects).toMatchObject({
          hasMore: false,
          paging: undefined,
          groups: [{ id: "studio" }, { id: "later" }],
        })
      }
    } finally {
      stop()
      vi.useRealTimers()
    }
  },
)

it("does not restart an errored Settings page when a save finishes", async () => {
  const { port, ongoing } = cloudProjectFixture()
  const savedPolicy = ongoingStatus({
    status: "ready",
    consentVersion: "desktop-project-sync-v1",
    projectChoices: { studio: false },
  })
  ongoing.status.mockResolvedValue(savedPolicy)
  ongoing.groups
    .mockRejectedValue(new Error("Synthetic folder read failure"))
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(3_550)
    expect(ongoing.groups).toHaveBeenCalledTimes(6)
    expect(session.getSnapshot().projects?.refreshError).toBe(
      "Could not check session folders.",
    )
    await session.setSavedProject(session.getSnapshot(), "studio", true)
    await vi.advanceTimersByTimeAsync(30_000)
    expect(ongoing.setProject).toHaveBeenCalledOnce()
    expect(session.getSnapshot().busy).toBe(false)
    expect(ongoing.groups).toHaveBeenCalledTimes(6)
    expect(session.getSnapshot().projects?.refreshError).toBe(
      "Could not check session folders.",
    )
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it.each(["more pages", "a page error", "another account"] as const)(
  "keeps Settings paging safe when a deferred removal ends with %s",
  async (outcome) => {
    const { port, ongoing, emit } = cloudProjectFixture()
    const savedPolicy = ongoingStatus({
      status: "ready",
      consentVersion: "desktop-project-sync-v1",
      projects: projectGroup().projects,
      projectChoices: { studio: true },
    })
    ongoing.status.mockResolvedValue(savedPolicy)
    const later = projectPage([projectGroup({ id: "later" })], secondCursor)
    const deferred = { ...later, partial: true, retrySamePage: true }
    let finishPage!: (page: ProjectPage) => void
    let finishRemoval!: (status: typeof savedPolicy) => void
    ongoing.groups
      .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
      .mockImplementationOnce(() => new Promise((resolve) => (finishPage = resolve)))
      .mockResolvedValueOnce(
        outcome === "a page error" ? deferred : projectPage([projectGroup({ id: "last" })]),
      )
    ongoing.setProject.mockImplementationOnce(
      () => new Promise((resolve) => (finishRemoval = resolve)),
    )
    vi.useFakeTimers()
    const session = new CloudSession(port, false, true, "automatic")
    const stop = session.subscribe(vi.fn())
    try {
      await vi.advanceTimersByTimeAsync(0)
      const removal = session.setSavedProject(session.getSnapshot(), "studio", false)
      await vi.advanceTimersByTimeAsync(0)
      expect(ongoing.setProject).toHaveBeenCalledExactlyOnceWith(
        expect.objectContaining({ groupId: "studio", selected: false }),
      )
      finishPage(outcome === "a page error" ? deferred : later)
      await vi.advanceTimersByTimeAsync(150)
      expect(session.getSnapshot().busy).toBe(true)
      expect(session.getSnapshot().projects?.groups.map(({ id }) => id)).toContain("later")
      expect(ongoing.groups).toHaveBeenCalledTimes(outcome === "a page error" ? 3 : 2)
      if (outcome === "another account")
        emit({
          identity: { id: "second-account", login: "Second" },
          destination: { id: "second-personal", kind: "personal", name: "Personal" },
        })
      finishRemoval({ ...savedPolicy, projects: [], projectChoices: { studio: false } })
      await removal
      await vi.advanceTimersByTimeAsync(30_000)
      expect(session.getSnapshot().busy).toBe(false)
      if (outcome === "another account") {
        expect(ongoing.groups).toHaveBeenCalledTimes(2)
        expect(session.getSnapshot().projects).toBeNull()
      } else if (outcome === "a page error") {
        expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
          null,
          firstCursor,
          firstCursor,
        ])
        expect(session.getSnapshot().projects?.refreshError).toBe(
          "Could not check session folders.",
        )
      } else {
        expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
          null,
          firstCursor,
          secondCursor,
        ])
        expect(session.getSnapshot().projects).toMatchObject({
          hasMore: false,
          paging: undefined,
          groups: [{ id: "studio" }, { id: "later" }, { id: "last" }],
          choices: { studio: false },
        })
      }
    } finally {
      stop()
      vi.useRealTimers()
    }
  },
)

it("verifies Settings scope again after resubscribe instead of treating the load as a save", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (page: ProjectPage) => void
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "later" })]))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  let stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    stop()
    finish(projectPage([projectGroup({ id: "ignored" })], secondCursor))
    await vi.advanceTimersByTimeAsync(0)
    stop = session.subscribe(vi.fn())
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.status).toHaveBeenCalledTimes(2)
    expect(port.workspaces).toHaveBeenCalledTimes(2)
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      firstCursor,
      null,
      firstCursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      paging: undefined,
      groups: [{ id: "studio" }, { id: "later" }],
    })
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("keeps a timed-out Settings page stopped until an explicit retry", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (page: ProjectPage) => void
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "later" })]))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(30_000)
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: true,
      hasMore: true,
      paging: "pending",
      error: null,
      refreshError: "Could not check session folders.",
    })
    finish(projectPage([projectGroup({ id: "ignored" })], secondCursor))
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects?.paging).toBeUndefined()
    await session.retryFolderScope()
    expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
      null,
      firstCursor,
      null,
      firstCursor,
    ])
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      paging: undefined,
      groups: [{ id: "studio" }, { id: "later" }],
    })
    expect(session.getSnapshot().projects?.refreshError).toBeUndefined()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it.each([7_000, 30_000])(
  "rechecks a failed Settings page after focus at %i ms and worker release",
  async (focusTime) => {
    const { port, ongoing } = cloudProjectFixture()
    let finish!: (page: ProjectPage) => void
    ongoing.groups
      .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
      .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
      .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
      .mockResolvedValueOnce(projectPage([projectGroup({ id: "later" })]))
    vi.useFakeTimers()
    const session = new CloudSession(port, false, true, "automatic")
    const stop = session.subscribe(vi.fn())
    try {
      await vi.advanceTimersByTimeAsync(focusTime)
      expect(session.getSnapshot().projects).toMatchObject({
        paging: "pending",
        refreshError: "Could not check session folders.",
      })
      window.dispatchEvent(new Event("focus"))
      window.dispatchEvent(new Event("focus"))
      await vi.advanceTimersByTimeAsync(30_000 - focusTime)
      expect(ongoing.groups).toHaveBeenCalledTimes(2)
      expect(port.workspaces).toHaveBeenCalledOnce()

      finish(projectPage([projectGroup({ id: "ignored" })], secondCursor))
      await vi.advanceTimersByTimeAsync(0)
      expect(ongoing.groups.mock.calls.map(([, cursor]) => cursor)).toEqual([
        null,
        firstCursor,
        null,
        firstCursor,
      ])
      expect(session.getSnapshot().projects).toMatchObject({
        hasMore: false,
        paging: undefined,
        groups: [{ id: "studio" }, { id: "later" }],
      })
      expect(session.getSnapshot().projects?.refreshError).toBeUndefined()
    } finally {
      stop()
      vi.useRealTimers()
    }
  },
)

it("does not recheck a slow Settings page that succeeds after focus", async () => {
  const { port, ongoing } = cloudProjectFixture()
  let finish!: (page: ProjectPage) => void
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(7_000)
    expect(session.getSnapshot().projects?.refreshError).toBe(
      "Could not check session folders.",
    )
    window.dispatchEvent(new Event("focus"))
    finish(projectPage([projectGroup({ id: "later" })]))
    await vi.advanceTimersByTimeAsync(0)

    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(port.workspaces).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      groups: [{ id: "studio" }, { id: "later" }],
    })
    expect(session.getSnapshot().projects?.refreshError).toBeUndefined()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("stops Settings paging when workspace verification denies access", async () => {
  const { port, ongoing } = cloudProjectFixture()
  ongoing.groups.mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
  vi.mocked(port.workspaces!).mockRejectedValueOnce("Cloud workspace access denied.")
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects).toMatchObject({
      loaded: false,
      scopeInvalid: true,
      error: "You do not have access to the selected Cloud workspace.",
    })
    expect(ongoing.enable).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("drops a pending Settings page when the account signs out", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  let finish!: (page: ProjectPage) => void
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    emit({ status: "signed_out", identity: null, destination: null })
    finish(projectPage([projectGroup({ id: "ignored" })], secondCursor))
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    expect(session.getSnapshot().projects).toBeNull()
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("does not reuse the former account's cursor when a pending Settings read releases", async () => {
  const { port, ongoing, emit } = cloudProjectFixture()
  const newCursor = { environment: "native", cwd: "/Example/NewAccount" }
  const newBinding = { accountId: "second-account", destinationId: "second-personal" }
  let finish!: (page: ProjectPage) => void
  ongoing.groups
    .mockResolvedValueOnce(projectPage([projectGroup()], firstCursor))
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "new-first" })], newCursor))
    .mockResolvedValueOnce(projectPage([projectGroup({ id: "new-last" })]))
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    ongoing.status.mockResolvedValue(ongoingStatus(newBinding))
    emit({
      identity: { id: newBinding.accountId, login: "Second" },
      destination: { id: newBinding.destinationId, kind: "personal", name: "Personal" },
    })
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    finish(projectPage([projectGroup({ id: "ignored" })], secondCursor))
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups.mock.calls.slice(2)).toEqual([
      [newBinding, null],
      [newBinding, newCursor],
    ])
    expect(session.getSnapshot().projects).toMatchObject({
      hasMore: false,
      paging: undefined,
      groups: [{ id: "new-first" }, { id: "new-last" }],
    })
  } finally {
    stop()
    vi.useRealTimers()
  }
})

it("does not reload Settings folders when upload and automatic-recovery events alternate", async () => {
  const { port, ongoing, emitSync } = cloudProjectFixture()
  vi.useFakeTimers()
  const session = new CloudSession(port, false, true, "automatic")
  const stop = session.subscribe(vi.fn())
  try {
    await vi.advanceTimersByTimeAsync(0)
    for (const automaticRecoveryCount of [0, 1, 0, 1]) {
      emitSync(
        ongoingStatus({
          status: "syncing",
          consentVersion: "desktop-project-sync-v1",
          projects: projectGroup().projects,
          automaticRecoveryCount,
        }),
      )
    }
    await vi.advanceTimersByTimeAsync(0)
    expect(ongoing.groups).toHaveBeenCalledOnce()
    expect(session.getSnapshot().projects?.groups[0]?.sessionCount).toBe(12)
    expect(ongoing.retry).not.toHaveBeenCalled()
    expect(ongoing.backfill).not.toHaveBeenCalled()
  } finally {
    stop()
    vi.useRealTimers()
  }
})
