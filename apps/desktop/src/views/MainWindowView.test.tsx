import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { Activity } from "lucide-react"
import { CollectionDetailPane } from "./main-window/CollectionDetailPane"
import type * as MainActivitySessionModule from "./main-window/MainActivitySession"
import type { SessionListEntry } from "../components/session/SessionList"
import type * as IpcModule from "../lib/ipc"
import type { MainWindowSectionRequest } from "../lib/ipc"
import type { SessionFilter } from "../lib/sessionFilters"
import capability from "../../src-tauri/capabilities/main.json"
import { openSettingsWindow } from "../lib/ipc"
import { MainWindowView } from "./MainWindowView"

vi.mock("./main-window/MainActivityView", () => ({ MainActivityView: () => <p>Sessions</p> }))
vi.mock("./main-window/BurnChecksView", () => ({
  BurnChecksView: () => <p>Burn checks workspace</p>,
}))

/**
 * A minimal stand-in for `MainActivitySession`. `MainWindowView` only reads
 * `entries` and `filter` off its snapshot and calls `setFilter`, so the fake
 * covers only that surface and lets a test drive the sidebar's counts and
 * selection without the real class's async IPC-backed engine.
 */
const activityMocks = vi.hoisted(() => {
  class FakeMainActivitySession {
    snapshot: { entries: SessionListEntry[] | null; filter: SessionFilter } = {
      entries: null,
      filter: { kind: "all" },
    }
    private listeners = new Set<() => void>()
    setFilter = vi.fn((filter: SessionFilter) => {
      this.snapshot = { ...this.snapshot, filter }
      this.notify()
    })
    constructor() {
      activityMocks.instances.push(this)
    }
    getSnapshot = () => this.snapshot
    subscribe = (listener: () => void) => {
      this.listeners.add(listener)
      return () => this.listeners.delete(listener)
    }
    subscribeInactive = this.subscribe
    setEntries(entries: SessionListEntry[] | null) {
      this.snapshot = { ...this.snapshot, entries }
      this.notify()
    }
    private notify() {
      for (const listener of [...this.listeners]) listener()
    }
  }
  return {
    FakeMainActivitySession,
    instances: [] as InstanceType<typeof FakeMainActivitySession>[],
  }
})

vi.mock("./main-window/MainActivitySession", async (importOriginal) => {
  const actual = await importOriginal<typeof MainActivitySessionModule>()
  return { ...actual, MainActivitySession: activityMocks.FakeMainActivitySession }
})

const ipcMocks = vi.hoisted(() => ({
  sectionTarget: null as ((request: MainWindowSectionRequest) => void) | null,
}))

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  openSettingsWindow: vi.fn().mockResolvedValue(undefined),
  onMainWindowSectionTarget: vi.fn(
    async (handler: (request: MainWindowSectionRequest) => void) => {
      ipcMocks.sectionTarget = handler
      return () => {
        ipcMocks.sectionTarget = null
      }
    },
  ),
  takeMainWindowSectionTarget: vi.fn().mockResolvedValue(null),
}))

function activitySession() {
  const instance = activityMocks.instances.at(-1)
  if (!instance) throw new Error("MainActivitySession was not constructed")
  return instance
}

function sessionEntry(over: Partial<SessionListEntry> = {}): SessionListEntry {
  return {
    agent: "claude-code",
    sessionId: "session-1",
    repo: "avery/widgets",
    timestamp: "2026-03-04T12:00:00.000Z",
    isActive: false,
    ...over,
  }
}

function tab(name: string) {
  return screen.getByRole("tab", { name })
}

const userAgent = Object.getOwnPropertyDescriptor(window.navigator, "userAgent")
const innerWidth = Object.getOwnPropertyDescriptor(window, "innerWidth")

function setUserAgent(value: string): void {
  Object.defineProperty(window.navigator, "userAgent", { configurable: true, value })
}

function setWindowWidth(value: number): void {
  Object.defineProperty(window, "innerWidth", { configurable: true, value })
}

afterEach(() => {
  vi.clearAllMocks()
  if (userAgent) Object.defineProperty(window.navigator, "userAgent", userAgent)
  if (innerWidth) Object.defineProperty(window, "innerWidth", innerWidth)
})

describe("MainWindowView", () => {
  it("keeps the macOS title-bar clearance as the drag region", () => {
    setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X)")
    setWindowWidth(900)

    const { container } = render(<MainWindowView />)

    expect(screen.getByRole("main", { name: "antiburn main window" })).toHaveClass(
      "main-window",
    )
    expect(container.querySelector("[data-tauri-drag-region]")).toHaveClass(
      "main-window-titlebar",
    )
  })

  it("keeps native title bars clear on other platforms", () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0)")
    setWindowWidth(900)

    const { container } = render(<MainWindowView />)

    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
  })

  it("opens Burn checks by default and keeps Sessions in the sidebar", () => {
    setWindowWidth(1000)
    render(<MainWindowView />)
    // Burn checks, Sessions, and Sessions' five fixed filter children (no
    // harness rows yet, since no entries have loaded).
    expect(screen.getAllByRole("tab")).toHaveLength(7)
    expect(screen.getByRole("tab", { name: "Burn checks" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tabpanel", { name: "Burn checks" })).toBeVisible()
    expect(screen.getByText("Burn checks workspace")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Sessions" }))
    expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Settings" })).toBeVisible()
    setWindowWidth(900)
    fireEvent(window, new Event("resize"))
    expect(screen.getByRole("tablist", { name: "Main sections" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Open navigation" })).toBeNull()
  })
  it("opens the existing Settings window without changing the selected section", () => {
    render(<MainWindowView />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    expect(openSettingsWindow).toHaveBeenCalledExactlyOnceWith()
    expect(screen.getByRole("tab", { name: "Burn checks" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(capability.permissions).toContain("allow-open-settings-window")
    expect(capability.permissions).toContain("allow-open-burn-check-sample")
  })

  it("shows a recoverable Settings error", async () => {
    vi.mocked(openSettingsWindow).mockRejectedValueOnce(new Error("Unavailable"))
    render(<MainWindowView />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not open Settings")
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    expect(screen.queryByRole("alert")).toBeNull()
  })

  it.each(["metaKey", "ctrlKey"])(
    "opens Settings with %s+comma from the detail pane",
    (modifier) => {
      render(<MainWindowView />)
      fireEvent.keyDown(screen.getByRole("tabpanel", { name: "Burn checks" }), {
        key: ",",
        [modifier]: true,
      })
      expect(openSettingsWindow).toHaveBeenCalledExactlyOnceWith()
      fireEvent.keyDown(document, { key: ",", [modifier]: true, repeat: true })
      fireEvent.keyDown(document, { key: "," })
      fireEvent.keyDown(document, { key: ",", [modifier]: true, shiftKey: true })
      expect(openSettingsWindow).toHaveBeenCalledTimes(1)
    },
  )

  it("opens a section's collection and detail without mounting unvisited sections", () => {
    const other = vi.fn(() => <p>Other workspace</p>)
    const sections = [
      {
        id: "activity",
        label: "Activity",
        icon: Activity,
        render: () => (
          <CollectionDetailPane
            title="Items"
            items={[{ id: "test", label: "Example" }]}
            emptyMessage="Empty"
            detailEmptyMessage="Choose an item"
            renderDetail={() => <p>Example detail</p>}
          />
        ),
      },
      { id: "other", label: "Other", icon: Activity, render: other },
    ]
    render(<MainWindowView sections={sections} />)
    expect(other).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole("option", { name: "Example" }))
    expect(screen.getByText("Example detail")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Activity" }))
    expect(screen.getByText("Example detail")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Other" }))
    expect(screen.getByText("Other workspace")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Activity" }))
    expect(screen.getByText("Example detail")).toBeVisible()
  })

  describe("Sessions filter children", () => {
    it("renders the fixed filter children without counts before entries load", () => {
      render(<MainWindowView />)
      for (const name of [
        "Notable Sessions",
        "Material Sessions",
        "Failing Sessions",
        "Passing Sessions",
        "All Sessions",
      ]) {
        expect(tab(name)).not.toBeNull()
      }
      expect(within(tab("Notable Sessions")).queryByText(/^\d+$/)).toBeNull()
    })

    it("shows a live count per fixed filter and one row per loaded harness", () => {
      render(<MainWindowView />)
      act(() => {
        activitySession().setEntries([
          sessionEntry({
            agent: "claude-code",
            sessionId: "s1",
            cost: { totalUsd: 5, figureLabel: "Estimated cost", isHighCost: true },
          }),
          sessionEntry({
            agent: "codex",
            sessionId: "s2",
            cost: { totalUsd: 0.5, figureLabel: "Estimated cost" },
          }),
        ])
      })
      expect(within(tab("Notable Sessions")).getByText("1")).toBeInTheDocument()
      expect(within(tab("Material Sessions")).getByText("1")).toBeInTheDocument()
      expect(within(tab("Claude Code Sessions")).getByText("1")).toBeInTheDocument()
      expect(within(tab("Codex Sessions")).getByText("1")).toBeInTheDocument()
      expect(within(tab("Failing Sessions")).getByText("0")).toBeInTheDocument()
      expect(within(tab("Passing Sessions")).getByText("0")).toBeInTheDocument()
      expect(within(tab("All Sessions")).getByText("2")).toBeInTheDocument()
    })

    it("selects Sessions and applies the filter when a child is clicked", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Failing Sessions"))
      expect(activitySession().setFilter).toHaveBeenCalledWith({ kind: "failing" })
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    })

    it("resets the filter to all when the Sessions row itself is clicked", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Failing Sessions"))
      activitySession().setFilter.mockClear()
      fireEvent.click(tab("Sessions"))
      expect(activitySession().setFilter).toHaveBeenCalledWith({ kind: "all" })
    })

    it("highlights the active filter's own child row instead of the parent", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Failing Sessions"))
      expect(tab("Failing Sessions")).toHaveAttribute("aria-selected", "true")
      expect(tab("Sessions")).toHaveAttribute("aria-selected", "false")
    })

    it("keeps the current filter when a cross-window request selects Sessions", async () => {
      render(<MainWindowView />)
      await vi.waitFor(() => expect(ipcMocks.sectionTarget).not.toBeNull())
      act(() => {
        ipcMocks.sectionTarget!({ revision: 1, section: "activity" })
      })
      expect(activitySession().setFilter).not.toHaveBeenCalled()
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    })
  })
})
