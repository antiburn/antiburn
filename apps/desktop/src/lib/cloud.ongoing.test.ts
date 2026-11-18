import { beforeEach, expect, it, vi } from "vitest"
import { desktopCloudPort, type CloudOngoingConsent } from "./cloud"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  hasShell: vi.fn(() => true),
  appInfo: vi.fn(async () => ({ cloudEnabled: true })),
  listen: vi.fn(async () => vi.fn()),
}))
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }))
vi.mock("./ipc", () => ({ hasShell: mocks.hasShell, appInfo: mocks.appInfo }))
vi.mock("./tauriEvents", () => ({ listen: mocks.listen }))

const binding = { accountId: "synthetic-account", destinationId: "synthetic-personal" }
const consent: CloudOngoingConsent = {
  ...binding,
  version: "desktop-project-sync-v1",
  selectedSources: [
    { sourceKind: "opencode_sqlite_v2", path: "/Example/OpenCode/opencode.db" },
  ],
  projects: [
    {
      sourceKind: "opencode_sqlite_v2",
      sourcePath: "/Example/OpenCode/opencode.db",
      projectPath: "/Example/Studio",
    },
  ],
  projectChoices: { studio: true, excluded: false },
  fullContent: true,
  ongoing: true,
  attachments: false,
}

beforeEach(() => {
  vi.clearAllMocks()
  mocks.hasShell.mockReturnValue(true)
  mocks.appInfo.mockResolvedValue({ cloudEnabled: true })
  mocks.invoke.mockResolvedValue({ status: "ready", storageKnown: true })
})

it("uses exact desktop ongoing commands and event names", async () => {
  const port = desktopCloudPort.ongoing!
  const listener = vi.fn()
  await port.watch(listener)
  expect(mocks.listen).toHaveBeenCalledWith("cloud:sync-changed", expect.any(Function))
  await port.status()
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_sync_status")
  const after = { environment: "native", cwd: "/Example/last-folder" }
  await port.groups(binding, after)
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_project_groups", { binding, after })
  await port.enable(consent)
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_enable_sync", { consent })
  await port.pause(binding)
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_pause_sync", { binding })
  await port.retry(binding)
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_retry_sync", { binding })
  expect(
    mocks.invoke.mock.calls.some(([command]) =>
      ["cloud_prepare_sync", "cloud_start_sync"].includes(command),
    ),
  ).toBe(false)
})

it("submits Start immediately after the desktop capability was verified", async () => {
  await desktopCloudPort.ongoing!.status()
  mocks.invoke.mockClear()
  mocks.appInfo.mockClear()
  const pending = desktopCloudPort.ongoing!.enable(consent)
  expect(mocks.invoke).toHaveBeenCalledWith("cloud_enable_sync", { consent })
  expect(mocks.appInfo).not.toHaveBeenCalled()
  await pending
})

it.each(["browser", "ordinary build"])("keeps ongoing IPC unreachable in %s", async (build) => {
  if (build === "browser") mocks.hasShell.mockReturnValue(false)
  else mocks.appInfo.mockResolvedValue({ cloudEnabled: false })
  const port = desktopCloudPort.ongoing!
  await port.watch(vi.fn())
  expect(await port.status()).toBeNull()
  await port.groups(binding, null)
  await expect(port.enable(consent)).rejects.toThrow("unavailable")
  await expect(port.pause(binding)).rejects.toThrow("unavailable")
  await expect(port.retry(binding)).rejects.toThrow("unavailable")
  expect(mocks.invoke).not.toHaveBeenCalled()
  expect(mocks.listen).not.toHaveBeenCalled()
})
