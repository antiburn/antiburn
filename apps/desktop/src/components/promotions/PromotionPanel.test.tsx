import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import catalog from "virtual:promotions"
import { writeClipboardText } from "../../lib/clipboard"
import { openSettingsWindow } from "../../lib/ipc"
import {
  PromotionSession,
  type PromotionEnvironment,
} from "../../lib/promotions/PromotionSession"
import { PromotionHost } from "./PromotionHost"

vi.mock("../../lib/clipboard", () => ({
  writeClipboardText: vi.fn().mockResolvedValue(undefined),
}))
vi.mock("../../lib/ipc", () => ({
  openSettingsWindow: vi.fn().mockResolvedValue(undefined),
  noteInteraction: vi.fn(),
}))

async function ready(session: PromotionSession) {
  render(<PromotionHost session={session} />)
  await act(async () => {
    for (let i = 0; i < 8; i += 1) await Promise.resolve()
  })
}
const env: PromotionEnvironment = {
  isManualReady: () => true,
  readLauncherOpened: () => null,
  writeLauncherOpened: vi.fn(),
  loadReady: async () => true,
  loadVisible: async () => true,
  onReady: async () => () => undefined,
  onVisible: async () => () => undefined,
  isForeground: () => true,
  onForeground: () => () => undefined,
  readSeen: () => null,
  writeSeen: vi.fn(),
}
beforeEach(() => {
  vi.clearAllMocks()
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({ matches: true, addEventListener: vi.fn(), removeEventListener: vi.fn() })),
  )
  vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => undefined)
  vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue(undefined)
})
afterEach(() => {
  vi.useRealTimers()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
})

describe("promotion panel", () => {
  it("uses the approved content and navigates without changing the frame", async () => {
    const session = new PromotionSession(catalog, env)
    const existing = document.createElement("button")
    document.body.append(existing)
    existing.focus()
    await ready(session)
    expect(document.activeElement).toBe(existing)
    const panel = screen.getByRole("dialog", { name: "antiburn Claude Mod" })
    expect(panel).toHaveAttribute("aria-modal", "false")
    expect(screen.queryByText("Desktop & Terminal")).not.toBeInTheDocument()
    expect(
      screen.getByText("See reset times at a glance and pace your usage when you need to."),
    ).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Set up" }))
    expect(screen.getByRole("dialog", { name: "Set up Claude Mod" })).toHaveClass(
      "promotion-panel",
    )
    expect(screen.getAllByRole("listitem")).toHaveLength(3)
    expect(screen.getByText(/Run claude in terminal or desktop/)).toBeVisible()
    expect(screen.queryByRole("button", { name: "Done" })).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    expect(screen.getByRole("button", { name: "Set up" })).toBeVisible()
    fireEvent.click(screen.getByRole("button", { name: "Close promotion" }))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    existing.remove()
  })
  it("copies the exact YAML command and briefly replaces its icon with Copied!", async () => {
    vi.useFakeTimers()
    const session = new PromotionSession(catalog, env)
    await ready(session)
    fireEvent.click(screen.getByRole("button", { name: "Set up" }))
    const copy = screen.getByRole("button", { name: "Copy Add the antiburn marketplace" })
    await act(async () => fireEvent.click(copy))
    expect(writeClipboardText).toHaveBeenCalledWith(
      "claude plugin marketplace add https://github.com/antiburn/antiburn.git",
    )
    expect(copy).toHaveTextContent("Copied!")
    expect(copy).toBeDisabled()
    act(() => vi.advanceTimersByTime(2_000))
    expect(copy).not.toHaveTextContent("Copied!")
    expect(copy).not.toBeDisabled()
  })
  it("reports clipboard failures without reporting success", async () => {
    vi.mocked(writeClipboardText).mockRejectedValueOnce(new Error("unavailable"))
    await ready(new PromotionSession(catalog, env))
    fireEvent.click(screen.getByRole("button", { name: "Set up" }))
    await act(async () =>
      fireEvent.click(screen.getByRole("button", { name: "Copy Install Claude Mod" })),
    )
    expect(screen.getByRole("alert")).toHaveTextContent("Could not copy")
    expect(screen.queryByText("Copied!")).not.toBeInTheDocument()
  })
  it("renders the dummy through the same renderer and runs its app action", async () => {
    const session = new PromotionSession(catalog, env)
    await ready(session)
    act(() => session.open())
    fireEvent.click(screen.getByRole("button", { name: "Sample promotion" }))
    const dummy = catalog.find((promo) => promo.id === "dummy-promo")!
    const panel = screen.getByRole("dialog", { name: dummy.screens.announcement.title })
    expect(within(panel).getByRole("img").querySelector("img")).toHaveAttribute(
      "src",
      expect.stringContaining("dummy"),
    )
    expect(within(panel).getAllByRole("listitem")).toHaveLength(1)
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "View usage" })))
    expect(openSettingsWindow).toHaveBeenCalledWith("usage")
  })
  it("defers presentation under a blocking modal and supports keyboard dismissal", async () => {
    const modal = document.createElement("dialog")
    modal.setAttribute("open", "")
    document.body.append(modal)
    const writeSeen = vi.fn()
    const session = new PromotionSession(catalog, { ...env, writeSeen })
    await ready(session)
    expect(
      screen.queryByRole("dialog", { name: "antiburn Claude Mod" }),
    ).not.toBeInTheDocument()
    expect(writeSeen).not.toHaveBeenCalled()
    await act(async () => modal.remove())
    const panel = screen.getByRole("dialog", { name: "antiburn Claude Mod" })
    expect(writeSeen).toHaveBeenCalledOnce()
    fireEvent.keyDown(panel, { key: "Escape" })
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  })
})

it("dismisses automatic announcements with Escape without moving outside focus", async () => {
  const button = document.createElement("button")
  document.body.append(button)
  button.focus()
  await ready(new PromotionSession(catalog, env))
  expect(document.activeElement).toBe(button)
  fireEvent.keyDown(document, { key: "Escape" })
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
  expect(document.activeElement).toBe(button)
  button.remove()
})

it("preserves button focus and presentation across unrelated host renders", async () => {
  const session = new PromotionSession(catalog, env)
  const presented = vi.spyOn(session, "presented")
  const launcher = document.createElement("button")
  document.body.append(launcher)
  const returnFocusTo = { current: launcher }
  launcher.focus()
  const view = render(<PromotionHost session={session} returnFocusTo={returnFocusTo} />)
  await act(async () => {
    for (let i = 0; i < 8; i += 1) await Promise.resolve()
  })
  act(() => session.openFromLauncher("claude-mod"))
  const setup = screen.getByRole("button", { name: "Set up" })
  setup.focus()
  presented.mockClear()
  view.rerender(<PromotionHost session={session} returnFocusTo={returnFocusTo} />)
  expect(document.activeElement).toBe(setup)
  expect(presented).not.toHaveBeenCalled()

  fireEvent.click(setup)
  expect(presented).toHaveBeenCalledOnce()
  expect(document.activeElement).toBe(screen.getByRole("dialog"))
  const back = screen.getByRole("button", { name: "Back" })
  back.focus()
  presented.mockClear()
  view.rerender(<PromotionHost session={session} returnFocusTo={returnFocusTo} />)
  expect(document.activeElement).toBe(back)
  expect(presented).not.toHaveBeenCalled()
  await act(async () => session.close())
  expect(document.activeElement).toBe(launcher)
  launcher.remove()
})
