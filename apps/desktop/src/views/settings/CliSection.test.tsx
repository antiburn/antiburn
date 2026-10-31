import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { CliSection } from "./CliSection"
import { SettingsTargetFocus } from "./SettingsTargetFocus"
import { writeClipboardText } from "../../lib/clipboard"
import {
  getCliInstallState,
  onCliInstallState,
  repairCliInstallation,
  testCliInstallation,
  type CliInstallState,
} from "../../lib/cliInstallIpc"

vi.mock("../../lib/cliInstallIpc", () => ({
  onCliInstallState: vi.fn(),
  getCliInstallState: vi.fn(),
  repairCliInstallation: vi.fn(),
  testCliInstallation: vi.fn(),
}))
vi.mock("../../lib/clipboard", () => ({ writeClipboardText: vi.fn() }))

const state: CliInstallState = {
  runtime: {
    executable: "/synthetic/cli/bin/antiburn-cli",
    version: "0.1.0",
    sha256: "a".repeat(64),
    deliveryProtocols: ["delivery-v1"],
  },
  diagnostic: null,
}
let publish: ((value: CliInstallState) => void) | undefined

beforeEach(() => {
  vi.resetAllMocks()
  vi.mocked(getCliInstallState).mockResolvedValue(state)
  vi.mocked(testCliInstallation).mockResolvedValue(state.runtime!)
  vi.mocked(repairCliInstallation).mockResolvedValue(state)
  vi.mocked(writeClipboardText).mockResolvedValue()
  vi.mocked(onCliInstallState).mockImplementation(async (onState) => {
    publish = onState
    return () => undefined
  })
})

async function ready() {
  const rendered = render(<CliSection />)
  await screen.findByText("Installed")
  return rendered
}

function openDetails() {
  const button = screen.getByRole("button", { name: "Technical details" })
  if (button.getAttribute("aria-expanded") !== "true") fireEvent.click(button)
}
function testButton() {
  openDetails()
  return screen.getByRole("button", { name: "Test" })
}
function repairButton() {
  openDetails()
  return screen.getByRole("button", { name: "Repair" })
}

describe("CLI Settings", () => {
  it("shows loading with Test disabled until inspection completes", async () => {
    let finish: ((value: CliInstallState) => void) | undefined
    vi.mocked(getCliInstallState).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    render(<CliSection />)
    expect(screen.getByText("Loading CLI status…")).toBeInTheDocument()
    expect(testButton()).toBeDisabled()
    await waitFor(() => expect(finish).toBeDefined())
    await act(async () => finish?.(state))
    expect(screen.getByText("Installed")).toBeInTheDocument()
  })

  it("disables actions during runtime repair", async () => {
    let finish: ((value: CliInstallState) => void) | undefined
    vi.mocked(repairCliInstallation).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await ready()
    fireEvent.click(repairButton())
    expect(screen.getByText("Repairing CLI…")).toBeInTheDocument()
    expect(testButton()).toBeDisabled()
    expect(screen.getByRole("button", { name: "Repairing…" })).toBeDisabled()
    await act(async () => finish?.(state))
    expect(screen.getByText("Installed")).toBeInTheDocument()
  })

  it("announces unchanged repair completion without claiming integration success", async () => {
    await ready()
    fireEvent.click(repairButton())
    expect(await screen.findByText("Repair complete. Run Test to verify.")).toHaveAttribute(
      "role",
      "status",
    )
    expect(screen.getByText("Installed")).toBeInTheDocument()
    expect(screen.queryByText("Test passed")).not.toBeInTheDocument()
    act(() => publish?.({ ...state, diagnostic: "runtime needs attention" }))
    expect(screen.queryByText(/Repair complete/)).not.toBeInTheDocument()
    act(() => publish?.(state))
    expect(screen.queryByText(/Repair complete/)).not.toBeInTheDocument()
  })

  it("rejects repeat presses while repair is pending and reserves button geometry", async () => {
    let finish: ((value: CliInstallState) => void) | undefined
    vi.mocked(repairCliInstallation).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await ready()
    const button = repairButton()
    expect(button).toHaveClass("min-w-28")
    act(() => {
      button.click()
      button.click()
      button.click()
    })
    expect(repairCliInstallation).toHaveBeenCalledOnce()
    const pending = screen.getByRole("button", { name: "Repairing…" })
    expect(pending).toHaveClass("min-w-28")
    expect(pending).not.toHaveTextContent("Repair CLI")
    expect(pending.querySelector("svg")).toHaveClass("motion-safe:animate-spin")
    expect(screen.getByRole("button", { name: "Copy path" })).toBeDisabled()
    await act(async () => finish?.(state))
    expect(repairButton().querySelector(".motion-safe\\:animate-spin")).toBeNull()
  })

  it("does not show repair success for rejected or diagnostic results", async () => {
    await ready()
    vi.mocked(repairCliInstallation).mockRejectedValueOnce(new Error("repair denied"))
    fireEvent.click(repairButton())
    expect(await screen.findByRole("alert")).toHaveTextContent("repair denied")
    expect(screen.queryByText(/Repair complete/)).not.toBeInTheDocument()
    vi.mocked(repairCliInstallation).mockResolvedValueOnce({
      ...state,
      diagnostic: "repair incomplete",
    })
    fireEvent.click(repairButton())
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("repair incomplete"),
    )
    expect(screen.queryByText(/Repair complete/)).not.toBeInTheDocument()
  })

  it("clears successful test and copy feedback when diagnostics change", async () => {
    await ready()
    fireEvent.click(testButton())
    await screen.findByText("Test passed")
    fireEvent.click(screen.getByRole("button", { name: "Copy path" }))
    await screen.findByText("Copied!")
    act(() => publish?.({ ...state, diagnostic: "permission changed" }))
    expect(screen.queryByText("Test passed")).not.toBeInTheDocument()
    expect(screen.queryByText("Copied!")).not.toBeInTheDocument()
  })

  it("shows copy progress, prevents duplicate copies and reports failures", async () => {
    let fail: ((reason: Error) => void) | undefined
    vi.mocked(writeClipboardText).mockImplementation(
      () =>
        new Promise((_resolve, reject) => {
          fail = reject
        }),
    )
    await ready()
    openDetails()
    const copy = screen.getByRole("button", { name: "Copy path" })
    act(() => {
      copy.click()
      copy.click()
    })
    expect(writeClipboardText).toHaveBeenCalledOnce()
    expect(screen.getByRole("button", { name: "Copying path…" })).toBeDisabled()
    expect(testButton()).toBeDisabled()
    await act(async () => fail?.(new Error("clipboard blocked")))
    expect(
      await screen.findByText("Couldn’t copy the path. Select it to copy manually."),
    ).toBeInTheDocument()
    expect(screen.queryByText("Copied!")).not.toBeInTheDocument()
  })

  it("shows user-facing purpose and version beside the runtime name", async () => {
    await ready()
    expect(
      screen.getByText("Helps antiburn share check findings with your coding agents."),
    ).toBeInTheDocument()
    expect(screen.getByText("Managed automatically by antiburn.")).toBeInTheDocument()
    expect(screen.getByText("v0.1.0").parentElement).toHaveTextContent("Antiburn CLI")
    openDetails()
    expect(screen.queryByText(/CLI version/)).not.toBeInTheDocument()
  })

  it("badges Installed but keeps Test passed as plain text", async () => {
    await ready()
    const installed = screen.getByText("Installed")
    expect(installed).toHaveClass(
      "rounded-control",
      "bg-system-green/10",
      "border-system-green/25",
      "type-caption",
    )
    expect(installed.querySelector("svg")).toBeNull()
    fireEvent.click(testButton())
    const passed = await screen.findByText("Test passed")
    expect(passed).toHaveClass("text-system-green")
    expect(passed).not.toHaveClass("rounded-control", "bg-system-green/10")
    expect(passed.querySelector("svg")).toBeNull()
  })

  it("keeps integration management out of the CLI pane", async () => {
    await ready()
    expect(
      screen.queryByRole("button", { name: "Manage integrations" }),
    ).not.toBeInTheDocument()
    openDetails()
    expect(
      screen.queryByRole("button", { name: "Manage integrations" }),
    ).not.toBeInTheDocument()
  })

  it("shows Copied in the reserved icon control slot and resets after two seconds", async () => {
    await ready()
    openDetails()
    vi.useFakeTimers()
    try {
      const button = screen.getByRole("button", { name: "Copy path" })
      expect(button).toHaveClass("size-8", "after:-inset-1")
      expect(button.parentElement).toHaveClass("gap-1")
      await act(async () => {
        fireEvent.click(button)
      })
      expect(screen.getByRole("button", { name: "Copied!" })).toBe(button)
      expect(button.querySelector(".lucide-check")).not.toBeNull()
      expect(screen.getByText("Copied!")).toHaveAttribute("role", "status")
      expect(screen.getByText("Copied!")).toHaveClass("min-w-14")
      await act(async () => vi.advanceTimersByTime(2_000))
      expect(screen.getByRole("button", { name: "Copy path" })).toBe(button)
      expect(screen.queryByText("Copied!")).not.toBeInTheDocument()
    } finally {
      vi.useRealTimers()
    }
  })

  it("offers direct repair with technical details collapsed", async () => {
    vi.mocked(getCliInstallState).mockResolvedValue({
      runtime: null,
      diagnostic: "missing payload",
    })
    render(<CliSection />)
    await screen.findByText("CLI unavailable. Try Repair.")
    expect(screen.getByRole("button", { name: "Technical details" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
    const repair = screen.getByRole("button", { name: "Repair" })
    repair.focus()
    fireEvent.click(repair)
    expect(screen.getByRole("button", { name: "Technical details" })).toHaveFocus()
    await screen.findByText("Repair complete. Run Test to verify.")
    expect(screen.getByRole("button", { name: "Technical details" })).toHaveFocus()
    expect(screen.getByRole("button", { name: "Technical details" })).toHaveAttribute(
      "aria-expanded",
      "true",
    )
    expect(repairCliInstallation).toHaveBeenCalledOnce()
  })

  it("uses maintenance icons and places feedback beside its action", async () => {
    await ready()
    expect(testButton().querySelector(".lucide-stethoscope")).not.toBeNull()
    expect(repairButton().querySelector(".lucide-wrench")).not.toBeNull()
    const copy = screen.getByRole("button", { name: "Copy path" })
    expect(copy).not.toHaveClass("ui-push-button")
    expect(copy).toHaveClass("hover:bg-surface-hover")
    expect(copy.parentElement?.parentElement).toHaveTextContent("CLI location")
    fireEvent.click(testButton())
    const result = await screen.findByText("Test passed")
    expect(result.parentElement).toContainElement(testButton())
    fireEvent.click(screen.getByRole("button", { name: "Technical details" }))
    expect(screen.queryByText("Test passed")).not.toBeInTheDocument()
    expect(screen.getByText("Installed")).toBeInTheDocument()
  })

  it("keeps technical details and maintenance actions out of the default view", async () => {
    await ready()
    expect(screen.getByRole("heading", { name: "CLI" })).toBeInTheDocument()
    expect(screen.getByText("Antiburn CLI")).toBeInTheDocument()
    expect(screen.queryByText(/from your terminal/)).not.toBeInTheDocument()
    expect(screen.queryByText(/CLI version/)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Repair" })).not.toBeInTheDocument()
    expect(screen.queryByText(/Test passed/)).not.toBeInTheDocument()
    expect(testCliInstallation).not.toHaveBeenCalled()
  })

  it("tests explicitly, reports progress and explains the scope in Details", async () => {
    let finish: ((value: NonNullable<CliInstallState["runtime"]>) => void) | undefined
    vi.mocked(testCliInstallation).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await ready()
    const button = testButton()
    act(() => {
      button.click()
      button.click()
      button.click()
    })
    expect(screen.getByRole("button", { name: "Testing…" })).toBeDisabled()
    expect(repairButton()).toBeDisabled()
    await act(async () => finish?.(state.runtime!))
    expect(screen.getByText("Test passed")).toBeInTheDocument()
    expect(screen.getByText("Installed")).toBeInTheDocument()
    openDetails()
    expect(screen.getByText("Tests CLI startup only.")).toBeInTheDocument()
    expect(testCliInstallation).toHaveBeenCalledOnce()
  })

  it("offers recovery after failure and keeps raw diagnostics inside Details", async () => {
    vi.mocked(testCliInstallation).mockRejectedValue(new Error("CLI test timed out"))
    await ready()
    fireEvent.click(testButton())
    expect(await screen.findByText("Couldn’t start the CLI. Try Repair.")).toBeInTheDocument()
    expect(screen.getByText("Installed")).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Technical details" }))
    expect(screen.queryByRole("alert")).not.toBeInTheDocument()
    expect(repairButton()).toBeEnabled()
    openDetails()
    expect(screen.getByRole("alert")).toHaveTextContent("CLI test timed out")
    fireEvent.click(repairButton())
    expect(await screen.findByText("Installed")).toBeInTheDocument()
    expect(screen.queryByText(/Test passed/)).not.toBeInTheDocument()
  })

  it("does not retain a passed result when the installed executable changes", async () => {
    await ready()
    fireEvent.click(testButton())
    await screen.findByText("Test passed")
    act(() => publish?.({ ...state, runtime: { ...state.runtime!, sha256: "b".repeat(64) } }))
    expect(screen.getByText("Installed")).toBeInTheDocument()
    expect(screen.queryByText(/Test passed/)).not.toBeInTheDocument()
  })

  it("ignores a failed test for a runtime replaced while the test was running", async () => {
    let fail: ((reason: Error) => void) | undefined
    vi.mocked(testCliInstallation).mockImplementation(
      () =>
        new Promise((_resolve, reject) => {
          fail = reject
        }),
    )
    await ready()
    fireEvent.click(testButton())
    act(() => publish?.({ ...state, runtime: { ...state.runtime!, sha256: "b".repeat(64) } }))
    await act(async () => fail?.(new Error("old runtime failed")))
    expect(screen.getByText("Installed")).toBeInTheDocument()
    expect(screen.queryByText(/Couldn’t start/)).not.toBeInTheDocument()
  })

  it("reveals and copies the exact CLI path only when requested", async () => {
    await ready()
    openDetails()
    expect(screen.getByText(state.runtime!.executable)).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Copy path" }))
    expect(await screen.findByText("Copied!")).toBeInTheDocument()
    expect(writeClipboardText).toHaveBeenCalledWith(state.runtime!.executable)
    expect(screen.queryByText(/from your terminal/)).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Terminal setup" })).not.toBeInTheDocument()
  })

  it("discards a copy result if the runtime changed while copying", async () => {
    let finish: (() => void) | undefined
    vi.mocked(writeClipboardText).mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = resolve
        }),
    )
    await ready()
    openDetails()
    fireEvent.click(screen.getByRole("button", { name: "Copy path" }))
    act(() => publish?.({ ...state, runtime: { ...state.runtime!, sha256: "b".repeat(64) } }))
    await act(async () => finish?.())
    act(() => publish?.(state))
    expect(screen.queryByText("Copied!")).not.toBeInTheDocument()
  })

  it("search focuses Details without running the CLI", async () => {
    const { container } = await ready()
    HTMLElement.prototype.scrollIntoView = vi.fn()
    const stop = new SettingsTargetFocus().attach(
      container as HTMLDivElement,
      "cli",
      "cliCommand",
      1,
    )
    await act(async () => {})
    expect(screen.getByRole("button", { name: "Technical details" })).toHaveFocus()
    expect(testCliInstallation).not.toHaveBeenCalled()
    expect(repairCliInstallation).not.toHaveBeenCalled()
    stop()
  })

  it("keeps the path selectable when copying fails", async () => {
    vi.mocked(writeClipboardText).mockRejectedValue(new Error("clipboard unavailable"))
    await ready()
    openDetails()
    fireEvent.click(screen.getByRole("button", { name: "Copy path" }))
    expect(
      await screen.findByText("Couldn’t copy the path. Select it to copy manually."),
    ).toBeInTheDocument()
    expect(screen.getByText(state.runtime!.executable)).toHaveClass("select-text")
  })

  it("offers recovery when installation status cannot be read", async () => {
    vi.mocked(getCliInstallState).mockRejectedValue(new Error("installation is busy"))
    render(<CliSection />)
    await screen.findByText("CLI status unavailable. Try again.")
    expect(testButton()).toBeDisabled()
    openDetails()
    expect(screen.getByRole("alert")).toHaveTextContent("installation is busy")
    fireEvent.click(repairButton())
    expect(await screen.findByText("Installed")).toBeInTheDocument()
  })

  it("offers repair when no runtime is installed", async () => {
    vi.mocked(getCliInstallState).mockResolvedValue({
      ...state,
      runtime: null,
    })
    render(<CliSection />)
    await screen.findByText("CLI unavailable. Try Repair.")
    expect(testButton()).toBeDisabled()
    expect(repairButton()).toBeEnabled()
  })

  it("shows an actionable runtime summary with details on demand", async () => {
    vi.mocked(getCliInstallState).mockResolvedValue({
      ...state,
      diagnostic: "Internal runtime needs repair",
    })
    render(<CliSection />)
    await screen.findByText("CLI needs attention. Open technical details for troubleshooting.")
    expect(screen.queryByText(/Internal runtime needs repair/)).not.toBeInTheDocument()
    openDetails()
    expect(screen.getByRole("alert")).toHaveTextContent("Internal runtime needs repair")
    fireEvent.click(testButton())
    expect(await screen.findByText("Test passed")).toBeInTheDocument()
    expect(
      screen.getByText(
        "Installation needs attention. Open technical details for troubleshooting.",
      ),
    ).toBeInTheDocument()
  })
})
