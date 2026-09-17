import { afterEach, describe, expect, it, vi } from "vitest"

import { mountWindow } from "./bootstrap"
import { installLivePhase } from "./lib/livePhase"

vi.mock("./lib/livePhase", () => ({ installLivePhase: vi.fn(() => () => {}) }))

afterEach(() => {
  document.body.innerHTML = ""
  vi.clearAllMocks()
})

describe("mountWindow", () => {
  it("gives every window the one phase the live animations share", () => {
    const root = document.createElement("div")
    root.id = "root"
    document.body.append(root)

    mountWindow(<p>A window</p>)

    // Each window runs its own copy of the stylesheets and its own clock. The
    // phase must reach all of them, so it installs here and not in a view.
    expect(installLivePhase).toHaveBeenCalledTimes(1)
  })

  it("reports a window that has no mount point", () => {
    expect(() => mountWindow(<p>A window</p>)).toThrow(/#root/)
  })
})
