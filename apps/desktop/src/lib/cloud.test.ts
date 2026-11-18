import { beforeEach, expect, it, vi } from "vitest"
import {
  cloudProjectSelectionKey,
  cloudProjectChoices,
  cloudDisplayPath,
  selectedCloudProjectPaths,
  desktopCloudPort,
  type CaptureConsent,
  type CloudProjectGroup,
} from "./cloud"
import type { AppInfo } from "./ipcPayloads"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  open: vi.fn(),
  hasShell: vi.fn(() => true),
  appInfo: vi.fn(async (): Promise<Pick<AppInfo, "cloudEnabled" | "homeDirectory">> => ({
    cloudEnabled: true,
  })),
  listen: vi.fn(async (_event: string, _listener: (event: { payload: unknown }) => void) =>
    vi.fn(),
  ),
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.open }))
vi.mock("./ipc", () => ({ hasShell: mocks.hasShell, appInfo: mocks.appInfo }))
vi.mock("./tauriEvents", () => ({ listen: mocks.listen }))

const folders = [{ path: "/Example/claude/projects/project", selected: true }]
const consent: CaptureConsent = {
  version: "native-full-content-v1",
  accountId: "synthetic-account",
  destinationId: "synthetic-personal",
  fullContent: true,
  attachments: false,
  selectedRoots: ["/Example/claude/projects/project"],
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.hasShell.mockReturnValue(true)
  mocks.appInfo.mockResolvedValue({ cloudEnabled: true })
  mocks.invoke.mockResolvedValue({ status: "upload_unavailable" })
  mocks.open.mockResolvedValue(null)
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

it("passes exact request bindings to desktop capture commands and returns their results", async () => {
  expect(await desktopCloudPort.prepareSync(folders, "request", consent)).toBeUndefined()
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_prepare_sync", {
    folders,
    requestId: "request",
    consent,
  })
  expect(await desktopCloudPort.start(folders, "request", consent)).toEqual({
    status: "upload_unavailable",
  })
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_start_sync", { requestId: "request" })
  expect(await desktopCloudPort.reconcile("request")).toEqual({ status: "upload_unavailable" })
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_reconcile_sync", {
    requestId: "request",
  })
  mocks.invoke.mockResolvedValue({ status: "cancelled" })
  expect(await desktopCloudPort.cancelSync("request")).toEqual({ status: "cancelled" })
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_cancel_sync", { requestId: "request" })
})

it.each(["browser", "ordinary build"])(
  "does not invoke capture commands in %s",
  async (build) => {
    if (build === "browser") mocks.hasShell.mockReturnValue(false)
    else mocks.appInfo.mockResolvedValue({ cloudEnabled: false })
    await desktopCloudPort.prepareSync(folders, "request", consent)
    expect(await desktopCloudPort.start(folders, "request", consent)).toEqual({
      status: "unavailable",
    })
    expect(await desktopCloudPort.reconcile("request")).toEqual({ status: "unavailable" })
    expect(await desktopCloudPort.cancelSync("request")).toEqual({ status: "unavailable" })
    expect(await desktopCloudPort.pick("opencode_sqlite_v2")).toEqual([])
    expect(mocks.invoke).not.toHaveBeenCalled()
    expect(mocks.open).not.toHaveBeenCalled()
  },
)

it("does not turn a desktop save failure into acceptance", async () => {
  mocks.invoke.mockRejectedValue(new Error("write failed"))
  await expect(desktopCloudPort.prepareSync(folders, "request", consent)).rejects.toThrow(
    "write failed",
  )
  await expect(desktopCloudPort.start(folders, "request", consent)).rejects.toThrow(
    "write failed",
  )
})

it("keeps legacy Claude directory selection separate from OpenCode file selection", async () => {
  mocks.open.mockResolvedValue("/Example/claude/projects/project")
  expect(await desktopCloudPort.pick()).toEqual(["/Example/claude/projects/project"])
  expect(mocks.open).toHaveBeenLastCalledWith({
    directory: true,
    multiple: true,
    title: "Choose Claude projects/<project> source directories",
  })
  expect(mocks.invoke).not.toHaveBeenCalled()
  mocks.invoke.mockResolvedValue("/Example/opencode")
  mocks.open.mockResolvedValue("/Example/opencode/opencode.db")
  expect(await desktopCloudPort.pick("opencode_sqlite_v2")).toEqual([
    "/Example/opencode/opencode.db",
  ])
  expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("cloud_source_picker_directory")
  expect(mocks.open).toHaveBeenLastCalledWith({
    directory: false,
    multiple: true,
    title: "Choose OpenCode V2 databases",
    defaultPath: "/Example/opencode",
  })
})

it("does not replace a failed picker hint with another provider or enroll a default", async () => {
  mocks.invoke.mockRejectedValueOnce(new Error("busy"))
  await expect(desktopCloudPort.pick("opencode_sqlite_v2")).rejects.toThrow("busy")
  expect(mocks.open).not.toHaveBeenCalled()
  mocks.invoke.mockResolvedValue(null)
  expect(await desktopCloudPort.pick("opencode_sqlite_v2")).toEqual([])
  expect(mocks.open).toHaveBeenLastCalledWith({
    directory: false,
    multiple: true,
    title: "Choose OpenCode V2 databases",
  })
})

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
  expect(mocks.open).not.toHaveBeenCalled()
})

it("excludes a whole project and its worktrees after an explicit local opt-out", () => {
  expect(selectedCloudProjectPaths(projects, { studio: false })).toEqual(["/Example/Website"])
})

it("keeps a confirmed group exclusion ahead of defaults until it is explicitly selected", () => {
  const saved = { studio: false }
  expect(cloudProjectChoices(projects, saved, {}).studio).toBe(false)
  expect(cloudProjectChoices(projects, saved, { studio: true }).studio).toBe(true)
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
