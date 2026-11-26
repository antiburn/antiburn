import { beforeEach, expect, it, vi } from "vitest"
import {
  cloudProjectSelectionKey,
  cloudProjectChoices,
  cloudDisplayPath,
  selectedCloudProjectPaths,
  desktopCloudPort,
  type CloudProjectGroup,
} from "./cloud"
import type { AppInfo } from "./ipcPayloads"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  hasShell: vi.fn(() => true),
  appInfo: vi.fn(async (): Promise<Pick<AppInfo, "cloudEnabled" | "homeDirectory">> => ({
    cloudEnabled: true,
  })),
  listen: vi.fn(async (_event: string, _listener: (event: { payload: unknown }) => void) =>
    vi.fn(),
  ),
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }))
vi.mock("./ipc", () => ({ hasShell: mocks.hasShell, appInfo: mocks.appInfo }))
vi.mock("./tauriEvents", () => ({ listen: mocks.listen }))

beforeEach(() => {
  vi.clearAllMocks()
  mocks.hasShell.mockReturnValue(true)
  mocks.appInfo.mockResolvedValue({ cloudEnabled: true })
  mocks.invoke.mockResolvedValue({ status: "unavailable" })
})

it.each([
  ["/Users/avery/dev/Studio", "/Users/avery", "~/dev/Studio"],
  ["/Users/avery", "/Users/avery/", "~"],
  ["/Users/avery-other/dev/Studio", "/Users/avery", "/Users/avery-other/dev/Studio"],
  ["/Users/jordan/dev/Studio", "/Users/avery", "/Users/jordan/dev/Studio"],
  ["/Users/avery/dev/Studio", null, "/Users/avery/dev/Studio"],
  ["C:\\Users\\Avery\\dev\\Studio", "C:\\Users\\Avery\\", "~\\dev\\Studio"],
])("abbreviates only the current proved home in %s", (path, home, expected) => {
  expect(cloudDisplayPath(path, home)).toBe(expected)
})

it("reads the display home from the shared desktop AppInfo contract", async () => {
  expect(await desktopCloudPort.displayHome?.()).toBeNull()
  mocks.appInfo.mockResolvedValue({
    cloudEnabled: true,
    homeDirectory: "/Users/avery",
  })
  expect(await desktopCloudPort.displayHome?.()).toBe("/Users/avery")
  mocks.hasShell.mockReturnValue(false)
  expect(await desktopCloudPort.displayHome?.()).toBeNull()
})

it("reads and observes local tools without using vendor integration status", async () => {
  const status = {
    revision: 3,
    phase: "ready",
    cliInstalled: true,
    cliTested: true,
    skillInstalled: true,
    reason: null,
  } as const
  mocks.appInfo.mockResolvedValue({ cloudEnabled: false })
  mocks.invoke.mockResolvedValue(status)
  expect(await desktopCloudPort.localTools.read()).toEqual(status)
  expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("get_local_tools_status")
  const listener = vi.fn()
  const stop = await desktopCloudPort.localTools.watch(listener)
  expect(mocks.listen).toHaveBeenCalledWith("local-tools:changed", expect.any(Function))
  mocks.listen.mock.calls[0]![1]({ payload: status })
  expect(listener).toHaveBeenCalledExactlyOnceWith(status)
  stop()
})

it("does not read or observe desktop tools in a browser preview", async () => {
  mocks.hasShell.mockReturnValue(false)
  expect(await desktopCloudPort.localTools.read()).toMatchObject({
    phase: "unavailable",
    cliInstalled: false,
    cliTested: false,
    skillInstalled: false,
  })
  const stop = await desktopCloudPort.localTools.watch(vi.fn())
  stop()
  expect(mocks.invoke).not.toHaveBeenCalled()
  expect(mocks.listen).not.toHaveBeenCalled()
})

it("opens the dashboard without passing a URL or account proof", async () => {
  await desktopCloudPort.openDashboard?.()
  expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("cloud_open_dashboard")
  await desktopCloudPort.openSetup?.()
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_open_dashboard", { setup: true })
})

it("passes an access update only when starting a scoped reapproval", async () => {
  await desktopCloudPort.begin()
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_begin")
  await desktopCloudPort.begin("session_reads")
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_begin", {
    accessUpdate: "session_reads",
  })
})

it.each(["browser", "ordinary build"])(
  "does not invoke capture or dashboard commands in %s",
  async (build) => {
    if (build === "browser") mocks.hasShell.mockReturnValue(false)
    else mocks.appInfo.mockResolvedValue({ cloudEnabled: false })
    expect(await desktopCloudPort.ongoing.status()).toBeNull()
    await expect(desktopCloudPort.ongoing.setEnabled(true)).rejects.toThrow(
      "Cloud is unavailable",
    )
    await expect(
      desktopCloudPort.ongoing.groups(
        {
          accountId: "synthetic-account",
          destinationId: "synthetic-personal",
        },
        null,
      ),
    ).resolves.toMatchObject({
      groups: [],
    })
    await expect(desktopCloudPort.openDashboard?.()).rejects.toThrow("Cloud is unavailable.")
    await expect(desktopCloudPort.openSetup?.()).rejects.toThrow("Cloud is unavailable.")
    expect(mocks.invoke).not.toHaveBeenCalled()
  },
)

const projects: readonly CloudProjectGroup[] = [
  {
    id: "studio",
    name: "Studio",
    path: "/Example/Studio",
    memberPaths: ["/Example/Studio", "/Example/worktrees/studio-fix"],
    sessionCount: 8,
    worktreeCount: 1,
  },
  {
    id: "website",
    name: "Website",
    path: "/Example/Website",
    memberPaths: ["/Example/Website"],
    sessionCount: 3,
    worktreeCount: 0,
  },
]

it("selects new project groups in memory without saving or granting consent", () => {
  const choices = Object.freeze({})
  expect(selectedCloudProjectPaths(projects, choices)).toEqual([
    "/Example/Studio",
    "/Example/Website",
    "/Example/worktrees/studio-fix",
  ])
  expect(choices).toEqual({})
  expect(mocks.invoke).not.toHaveBeenCalled()
})

it("excludes a whole project and its worktrees after an explicit local opt-out", () => {
  expect(selectedCloudProjectPaths(projects, { studio: false })).toEqual(["/Example/Website"])
})

it("keeps a confirmed group exclusion ahead of defaults until it is explicitly selected", () => {
  const saved = { studio: false }
  expect(cloudProjectChoices(projects, saved, {}).studio).toBe(false)
  expect(cloudProjectChoices(projects, saved, { studio: true }).studio).toBe(true)
})

it("carries an old group opt-out without carrying an old opt-in", () => {
  const updated = projects.map((project) => ({ ...project, legacyOptOutId: "old-id" }))
  expect(cloudProjectChoices(updated, { "old-id": false }, {}).studio).toBe(false)
  expect(cloudProjectChoices(updated, { "old-id": true }, {}, false).studio).toBe(false)
  expect(cloudProjectChoices(updated, { "old-id": false, studio: true }, {}).studio).toBe(true)
})

it("freezes only current member paths and requires a new key for an external worktree", () => {
  const selected = selectedCloudProjectPaths(projects, {})
  const key = cloudProjectSelectionKey(projects, {})
  const expanded = projects.map((project) =>
    project.id === "studio"
      ? {
          ...project,
          memberPaths: [...project.memberPaths, "/Example/external/studio-review"],
          worktreeCount: 2,
        }
      : project,
  )
  expect(selected).not.toContain("/Example/external/studio-review")
  expect(cloudProjectSelectionKey(expanded, {})).not.toBe(key)
  expect(selectedCloudProjectPaths(expanded, {})).toContain("/Example/external/studio-review")
})

it("keeps confirmation keys stable when counts or display order change", () => {
  const reordered = projects
    .map((project) => ({
      ...project,
      sessionCount: 20,
      memberPaths: [...project.memberPaths].reverse(),
    }))
    .reverse()
  expect(cloudProjectSelectionKey(reordered, {})).toBe(cloudProjectSelectionKey(projects, {}))
})

it("includes multiple source-owned CWD paths without inferring files or sibling folders", () => {
  const group: CloudProjectGroup = {
    ...projects[0]!,
    memberPaths: ["/Example/Studio", "/Example/Studio/packages/web"],
  }
  const before = cloudProjectSelectionKey([group], {})
  const changedCwd = {
    ...group,
    memberPaths: [...group.memberPaths, "/Example/another-cwd"],
  }
  expect(selectedCloudProjectPaths([changedCwd], {})).toEqual([
    "/Example/Studio",
    "/Example/Studio/packages/web",
    "/Example/another-cwd",
  ])
  expect(cloudProjectSelectionKey([changedCwd], {})).not.toBe(before)
  expect(selectedCloudProjectPaths([{ ...group, memberPaths: [] }], {})).toEqual([])
})
