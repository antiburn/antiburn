import { afterEach, describe, expect, it, vi } from "vitest"
import { cloudDevScenarios, createCloudDevPort } from "./cloudDevPort"
describe("Cloud Dev simulation", () => {
  afterEach(() => vi.useRealTimers())
  it("loads every menu scenario without selecting local folders", async () => {
    for (const scenario of cloudDevScenarios) {
      const port = createCloudDevPort(scenario)
      const value = await port.read()
      expect(value.draft.folders.every((folder) => folder.path.startsWith("/Example/"))).toBe(
        true,
      )
      const projects = await port.ongoing!.groups(
        { accountId: "cloud-dev-account", destinationId: "cloud-dev-personal" },
        null,
      )
      expect(
        projects.groups.every((group) =>
          group.folderPaths.every((path) => path.startsWith("/Example/")),
        ),
      ).toBe(true)
      if (scenario.startsWith("complete-"))
        expect((await port.localTools.read()).phase).toBe(scenario.slice(9))
    }
  })
  it("cancels a simulated callback and disposes delayed work", async () => {
    vi.useFakeTimers()
    const port = createCloudDevPort("walkthrough")
    const listener = vi.fn()
    const stop = await port.watch(listener)
    expect((await port.begin()).status).toBe("waiting")
    await port.cancel()
    await vi.runAllTimersAsync()
    expect(listener).not.toHaveBeenCalled()
    expect((await port.read()).identity).toBeNull()
    await port.begin()
    stop()
    await vi.runAllTimersAsync()
    expect(listener).not.toHaveBeenCalled()
  })
  it("simulates auth, progress, cancellation and recovery without persistence", async () => {
    vi.useFakeTimers()
    const port = createCloudDevPort("walkthrough")
    const stop = await port.watch(() => {})
    await port.begin()
    await vi.advanceTimersByTimeAsync(1800)
    expect((await port.read()).identity?.login).toBe("example")
    const groups = await port.ongoing!.groups(
      { accountId: "cloud-dev-account", destinationId: "cloud-dev-personal" },
      null,
    )
    await port.ongoing!.enable({
      version: "desktop-project-sync-v1",
      accountId: "cloud-dev-account",
      destinationId: "cloud-dev-personal",
      fullContent: true,
      attachments: false,
      ongoing: true,
      selectedSources: groups.sourceRoots,
      projects: groups.groups.flatMap((group) => group.projects),
      projectChoices: Object.fromEntries(groups.groups.map((group) => [group.id, true])),
    })
    await vi.advanceTimersByTimeAsync(1500)
    expect((await port.ongoing!.status())?.status).toBe("syncing")
    await port.ongoing!.pause({
      accountId: "cloud-dev-account",
      destinationId: "cloud-dev-personal",
    })
    await vi.runAllTimersAsync()
    expect((await port.ongoing!.status())?.status).toBe("paused")
    await port.ongoing!.retry({
      accountId: "cloud-dev-account",
      destinationId: "cloud-dev-personal",
    })
    await vi.runAllTimersAsync()
    expect((await port.ongoing!.status())?.status).toBe("syncing")
    stop()
    expect((await createCloudDevPort("walkthrough").read()).identity).toBeNull()
  })
})
