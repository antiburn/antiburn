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

for (const stage of ["status", "groups"] as const) {
  it.each(["load", "an account event"] as const)(
    `recovers discovery after setup invalidates a pending ${stage} reply through %s`,
    async (recovery) => {
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
        emit({ setupError: "Synthetic setup failure" })
        expect(session.getSnapshot().projects?.loading).toBe(false)
        await session.loadProjects()
        expect(ongoing.groups).toHaveBeenCalledTimes(stage === "groups" ? 1 : 0)
        if (recovery === "load") {
          vi.mocked(port.read).mockResolvedValue(healthy)
          await session.load()
        } else {
          emit({ setupError: null })
        }
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
}

it("keeps cached exclusions through setup failure and a full account reload", async () => {
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
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(2))
    emit({ setupError: "Synthetic setup failure" })
    expect(session.getSnapshot().projects).toMatchObject({
      loading: false,
      partial: true,
      choices: { studio: false },
      groups: [expect.objectContaining({ id: "studio" })],
    })
    await session.load()
    expect(session.getSnapshot().busy).toBe(false)
    expect(ongoing.groups).toHaveBeenCalledTimes(2)
    vi.mocked(port.read).mockResolvedValue({
      ...session.getSnapshot().cloud,
      setupError: null,
    })
    let finishRecovery!: () => void
    ongoing.groups.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishRecovery = () => resolve(discovery)
        }),
    )
    const recovery = session.load()
    await vi.waitFor(() => expect(ongoing.groups).toHaveBeenCalledTimes(3))
    finish()
    await pending
    expect(session.getSnapshot().projects).toMatchObject({
      loading: true,
      partial: true,
      choices: { studio: false },
      groups: [expect.objectContaining({ id: "studio" })],
    })
    finishRecovery()
    await recovery
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
