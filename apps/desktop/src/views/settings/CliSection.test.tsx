import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { CliSection } from "./CliSection"
import { SettingsTargetFocus } from "./SettingsTargetFocus"
import {
  getCliInstallState,
  repairCliInstallation,
  removeCliInstallation,
} from "../../lib/cliInstallIpc"

vi.mock("../../lib/cliInstallIpc", () => ({
  onCliInstallState: vi.fn(async () => () => {}),
  getCliInstallState: vi.fn(),
  repairCliInstallation: vi.fn(),
  removeCliInstallation: vi.fn(),
}))

const state = {
  runtime: {
    executable: "/synthetic/cli/bin/antiburn-cli",
    version: "0.1.0",
    sha256: "a".repeat(64),
    deliveryProtocols: ["delivery-v1"],
  },
  terminalPath: "/synthetic/local/bin/antiburn-cli",
  terminalInstalled: true,
  binDirOnAppPath: false,
  diagnostic: null,
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(getCliInstallState).mockResolvedValue(state)
})

describe("CLI installation Settings", () => {
  it("distinguishes ready runtime and terminal command from shell PATH", async () => {
    render(<CliSection />)
    expect(await screen.findByText("Runtime ready · CLI 0.1.0")).toBeInTheDocument()
    expect(screen.getByText("Terminal command installed")).toBeInTheDocument()
    expect(screen.getByText(/Shell PATH is configured separately/)).toBeInTheDocument()
  })

  it("removes and repairs only after an explicit click", async () => {
    vi.mocked(removeCliInstallation).mockResolvedValue({ ...state, terminalInstalled: false })
    vi.mocked(repairCliInstallation).mockResolvedValue(state)
    render(<CliSection />)
    const remove = await screen.findByRole("button", { name: "Remove terminal command" })
    await waitFor(() => expect(remove).toBeEnabled())
    fireEvent.click(remove)
    expect(await screen.findByText("Terminal command not installed")).toBeInTheDocument()
    expect(screen.getByText("Runtime ready · CLI 0.1.0")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Repair" }))
    expect(await screen.findByText("Terminal command installed")).toBeInTheDocument()
    expect(removeCliInstallation).toHaveBeenCalledOnce()
    expect(repairCliInstallation).toHaveBeenCalledOnce()
  })

  it("search focus reaches the control without executing repair", async () => {
    const { container } = render(<CliSection />)
    await screen.findByText("Runtime ready · CLI 0.1.0")
    HTMLElement.prototype.scrollIntoView = vi.fn()
    const stop = new SettingsTargetFocus().attach(
      container as HTMLDivElement,
      "general",
      "cliCommand",
      1,
    )
    await act(async () => {})
    expect(screen.getByRole("button", { name: "Repair" })).toHaveFocus()
    expect(repairCliInstallation).not.toHaveBeenCalled()
    stop()
  })

  it("shows ownership collision diagnostics", async () => {
    vi.mocked(getCliInstallState).mockResolvedValue({
      ...state,
      terminalInstalled: false,
      diagnostic: "Terminal command collides with an unrelated command",
    })
    render(<CliSection />)
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "collides with an unrelated command",
    )
  })
})
