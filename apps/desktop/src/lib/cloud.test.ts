import { beforeEach, expect, it, vi } from "vitest"
import { nativeCloudPort, type CaptureConsent } from "./cloud"

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  open: vi.fn(),
  hasShell: vi.fn(() => true),
  appInfo: vi.fn(async () => ({ cloudEnabled: true })),
  listen: vi.fn(async () => vi.fn()),
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

it("passes exact request bindings to native capture commands and returns their results", async () => {
  expect(await nativeCloudPort.prepareSync(folders, "request", consent)).toBeUndefined()
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_prepare_sync", {
    folders,
    requestId: "request",
    consent,
  })
  expect(await nativeCloudPort.start(folders, "request", consent)).toEqual({
    status: "upload_unavailable",
  })
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_start_sync", { requestId: "request" })
  expect(await nativeCloudPort.reconcile("request")).toEqual({ status: "upload_unavailable" })
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_reconcile_sync", {
    requestId: "request",
  })
  mocks.invoke.mockResolvedValue({ status: "cancelled" })
  expect(await nativeCloudPort.cancelSync("request")).toEqual({ status: "cancelled" })
  expect(mocks.invoke).toHaveBeenLastCalledWith("cloud_cancel_sync", { requestId: "request" })
})

it.each(["browser", "ordinary build"])(
  "does not invoke capture commands in %s",
  async (build) => {
    if (build === "browser") mocks.hasShell.mockReturnValue(false)
    else mocks.appInfo.mockResolvedValue({ cloudEnabled: false })
    await nativeCloudPort.prepareSync(folders, "request", consent)
    expect(await nativeCloudPort.start(folders, "request", consent)).toEqual({
      status: "unavailable",
    })
    expect(await nativeCloudPort.reconcile("request")).toEqual({ status: "unavailable" })
    expect(await nativeCloudPort.cancelSync("request")).toEqual({ status: "unavailable" })
    expect(await nativeCloudPort.pick("opencode_sqlite_v2")).toEqual([])
    expect(mocks.invoke).not.toHaveBeenCalled()
    expect(mocks.open).not.toHaveBeenCalled()
  },
)

it("does not turn a native save failure into acceptance", async () => {
  mocks.invoke.mockRejectedValue(new Error("write failed"))
  await expect(nativeCloudPort.prepareSync(folders, "request", consent)).rejects.toThrow(
    "write failed",
  )
  await expect(nativeCloudPort.start(folders, "request", consent)).rejects.toThrow(
    "write failed",
  )
})

it("keeps legacy Claude directory selection separate from OpenCode file selection", async () => {
  mocks.open.mockResolvedValue("/Example/claude/projects/project")
  expect(await nativeCloudPort.pick()).toEqual(["/Example/claude/projects/project"])
  expect(mocks.open).toHaveBeenLastCalledWith({
    directory: true,
    multiple: true,
    title: "Choose Claude projects/<project> source directories",
  })
  expect(mocks.invoke).not.toHaveBeenCalled()
  mocks.invoke.mockResolvedValue("/Example/opencode")
  mocks.open.mockResolvedValue("/Example/opencode/opencode.db")
  expect(await nativeCloudPort.pick("opencode_sqlite_v2")).toEqual([
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
  await expect(nativeCloudPort.pick("opencode_sqlite_v2")).rejects.toThrow("busy")
  expect(mocks.open).not.toHaveBeenCalled()
  mocks.invoke.mockResolvedValue(null)
  expect(await nativeCloudPort.pick("opencode_sqlite_v2")).toEqual([])
  expect(mocks.open).toHaveBeenLastCalledWith({
    directory: false,
    multiple: true,
    title: "Choose OpenCode V2 databases",
  })
})
