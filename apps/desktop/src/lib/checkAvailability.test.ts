import { beforeEach, expect, it, vi } from "vitest"

import { emptyCheckAvailability, getCheckAvailability } from "./checkAvailability"

const invoke = vi.hoisted(() => vi.fn())
const hasShell = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke }))
vi.mock("./ipc", () => ({ hasShell }))

beforeEach(() => {
  invoke.mockReset()
  hasShell.mockReset().mockReturnValue(true)
})

it("starts with discovery enabled without inventing installed checks or a provider", () => {
  expect(emptyCheckAvailability).toMatchObject({
    yamlEnabled: true,
    configured: false,
    savedKey: false,
    installedChecks: [],
  })
})

it("uses the same empty discovery state without the native shell", async () => {
  hasShell.mockReturnValue(false)
  expect(await getCheckAvailability()).toBe(emptyCheckAvailability)
  expect(invoke).not.toHaveBeenCalled()
})

it.each([false, true])("preserves the saved native preference %s", async (yamlEnabled) => {
  const saved = { ...emptyCheckAvailability, revision: 1, yamlEnabled }
  invoke.mockResolvedValue(saved)
  expect(await getCheckAvailability()).toBe(saved)
})

it("rejects invalid availability instead of replacing it with the default", async () => {
  invoke.mockResolvedValue({ ...emptyCheckAvailability, yamlEnabled: "true" })
  await expect(getCheckAvailability()).rejects.toThrow("Could not read check status.")
})

it("keeps native read failures visible", async () => {
  invoke.mockRejectedValue(new Error("Could not read preferences."))
  await expect(getCheckAvailability()).rejects.toThrow("Could not read preferences.")
})
