import { expect, it, vi } from "vitest"
import { invoke } from "./core"

it("allows synthetic reads and rejects commands outside the demo allowlist", async () => {
  const fetch = vi.spyOn(globalThis, "fetch")
  await expect(invoke("app_info")).resolves.toBeTruthy()
  await expect(invoke("set_settings", { settings: {} })).rejects.toThrow(
    "Demo command is unavailable",
  )
  await expect(invoke("open_settings_window")).rejects.toThrow("Demo command is unavailable")
  expect(fetch).not.toHaveBeenCalled()
  fetch.mockRestore()
})
