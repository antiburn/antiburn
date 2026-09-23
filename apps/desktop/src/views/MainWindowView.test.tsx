import { isMacOS } from "../lib/platform"
import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { Activity } from "lucide-react"
import { CollectionDetailPane } from "./main-window/CollectionDetailPane"
import type * as MainActivitySessionModule from "./main-window/MainActivitySession"
import type { SessionListEntry } from "../components/session/SessionList"
import type * as IpcModule from "../lib/ipc"
import type { MainWindowNavigationRequest } from "../lib/ipc"
import type { SessionSubject } from "../lib/sessionSubject"
import type { SessionFilters } from "../lib/sessionFilters"
import capability from "../../src-tauri/capabilities/main.json"
import { noteInteraction, openSettingsWindow } from "../lib/ipc"
import { MainWindowView } from "./MainWindowView"
import { searchApp } from "../lib/appSearch"

vi.mock("./main-window/MainActivityView", () => ({ MainActivityView: () => <p>Sessions</p> }))
vi.mock("./main-window/BurnChecksView", () => ({
  BurnChecksView: () => <p>Burn checks workspace</p>,
}))
vi.mock("./main-window/OverviewView", () => ({
  OverviewView: ({
    onSelectSession,
  }: {
    onSelectSession: (entry: SessionListEntry) => void
  }) => (
    <div>
      <p>Overview workspace</p>
      <button type="button" onClick={() => onSelectSession(overviewMocks.recentEntry)}>
        Recent session
      </button>
    </div>
  ),
}))

vi.mock("./main-window/quota/QuotaView", () => ({
  QuotaView: ({ onSelectSession }: { onSelectSession: (subject: SessionSubject) => void }) => (
    <div>
      <h1>Limits</h1>
      <button
        onClick={() =>
          onSelectSession({ agent: "codex", sessionId: "limits-session", wslDistro: null })
        }
      >
        Open Limits session
      </button>
    </div>
  ),
}))

const overviewMocks = vi.hoisted(() => ({
  recentEntry: {
    agent: "claude",
    sessionId: "recent-1",
    repo: "antiburn",
    timestamp: "2026-09-14T08:00:00Z",
    isActive: false,
    title: "Recent session",
  } satisfies SessionListEntry,
}))

/** Keep the navigation state synchronous so tests can drive view transitions. */
const activityMocks = vi.hoisted(() => {
  class FakeMainActivitySession {
    revealDetail = vi.fn()
    snapshot: {
      entries: SessionListEntry[] | null
      filters: SessionFilters
      subject: SessionSubject | null
    } = {
      entries: null,
      subject: null,
      filters: { agents: [], result: "all", spend: "all" },
    }
    private listeners = new Set<() => void>()
    setFilters = vi.fn((filters: SessionFilters) => {
      this.snapshot = { ...this.snapshot, filters }
      this.notify()
      this.onNavigation?.("user")
    })
    onNavigation?: (origin: "user" | "automatic") => void
    onDeleted?: (subject: SessionSubject) => void
    restoreNavigation = vi.fn((filters: SessionFilters, subject: SessionSubject | null) => {
      this.snapshot = { ...this.snapshot, filters, subject }
      this.notify()
    })
    selectEntry = vi.fn((entry: SessionListEntry) => {
      if (!entry.sessionId) return
      this.snapshot = { ...this.snapshot, subject: { ...entry, sessionId: entry.sessionId } }
      this.notify()
      this.onNavigation?.("user")
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
    subscribeList = (listener: () => void) => {
      activityMocks.listSubscriptions += 1
      return this.subscribe(listener)
    }
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
    listSubscriptions: 0,
  }
})

vi.mock("./main-window/MainActivitySession", async (importOriginal) => {
  const actual = await importOriginal<typeof MainActivitySessionModule>()
  return { ...actual, MainActivitySession: activityMocks.FakeMainActivitySession }
})

const ipcMocks = vi.hoisted(() => ({
  sectionTarget: null as ((request: MainWindowNavigationRequest) => void) | null,
}))

vi.mock("../lib/ipc", async (importOriginal) => ({
  ...(await importOriginal<typeof IpcModule>()),
  openSettingsWindow: vi.fn().mockResolvedValue(undefined),
  noteInteraction: vi.fn(),
  onMainWindowNavigationTarget: vi.fn(
    async (handler: (request: MainWindowNavigationRequest) => void) => {
      ipcMocks.sectionTarget = handler
      return () => {
        ipcMocks.sectionTarget = null
      }
    },
  ),
  peekMainWindowNavigationTarget: vi.fn().mockResolvedValue(null),
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
  activityMocks.listSubscriptions = 0
  if (userAgent) Object.defineProperty(window.navigator, "userAgent", userAgent)
  if (innerWidth) Object.defineProperty(window, "innerWidth", innerWidth)
})

describe("MainWindowView", () => {
  beforeEach(() => {
    localStorage.clear()
    Object.defineProperty(HTMLDialogElement.prototype, "showModal", {
      configurable: true,
      value: function (this: HTMLDialogElement) {
        this.open = true
      },
    })
    Object.defineProperty(HTMLDialogElement.prototype, "close", {
      configurable: true,
      value: function (this: HTMLDialogElement) {
        this.open = false
      },
    })
    HTMLElement.prototype.scrollIntoView = vi.fn()
  })
  it("records only completed catalog navigation and never query text", async () => {
    render(<MainWindowView />)
    fireEvent.keyDown(document, { key: "k", metaKey: isMacOS(), ctrlKey: !isMacOS() })
    expect(noteInteraction).toHaveBeenCalledWith({ kind: "appSearchOpened" })
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "sound" } })
    await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
    expect(openSettingsWindow).toHaveBeenCalledWith("notifications", "sound")
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "appSearchResultOpened",
      category: "setting",
    })
    expect(screen.getByRole("tabpanel", { name: "Overview" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Back" })).toBeDisabled()
  })
  it("keeps toolbar search available and restores its focus", () => {
    setUserAgent("Mozilla/5.0 (Macintosh; Intel Mac OS X)")
    render(<MainWindowView />)
    expect(screen.getByRole("button", { name: "Search antiburn" })).toBeVisible()
    fireEvent.keyDown(document, { key: "k", metaKey: true })
    expect(screen.getByRole("combobox")).toHaveFocus()
    fireEvent.click(screen.getByRole("button", { name: "Close search" }))
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Search antiburn" })).toHaveFocus()
  })
  it("focuses the destination after choosing a view from search", async () => {
    render(<MainWindowView />)
    fireEvent.keyDown(document, { key: "k", metaKey: isMacOS(), ctrlKey: !isMacOS() })
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "burn checks" } })
    await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
    expect(screen.getByRole("tabpanel", { name: "Checks" })).toHaveFocus()
  })
  it.each([
    "Mozilla/5.0 (Macintosh; Intel Mac OS X)",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
    "Mozilla/5.0 (X11; Linux x86_64)",
  ])("searches every sidebar view by its visible label on %s", async (platform) => {
    setUserAgent(platform)
    render(<MainWindowView />)
    const views = screen.getAllByRole("tabpanel", { hidden: true }).map((panel) => ({
      panel,
      label: panel.getAttribute("aria-label")!,
    }))
    for (const { panel, label } of views) {
      const sidebarTab = tab(label)
      expect(sidebarTab).toHaveAttribute("aria-controls", panel.id)
      const matches = searchApp(label).filter((result) => result.label === label)
      expect(matches).toHaveLength(1)
      expect(matches[0]?.target).toMatchObject({ kind: "view" })
      fireEvent.click(sidebarTab)
      expect(panel).toBeVisible()
      fireEvent.click(tab(label === "Overview" ? "Checks" : "Overview"))
      fireEvent.click(screen.getByRole("button", { name: "Search antiburn" }))
      fireEvent.change(screen.getByRole("combobox"), { target: { value: label } })
      await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
      expect(panel).toBeVisible()
      expect(panel).toHaveFocus()
    }
  })
  it("does not report a successful result when Settings fails to open", async () => {
    vi.mocked(openSettingsWindow).mockRejectedValueOnce(new Error("unavailable"))
    render(<MainWindowView />)
    fireEvent.keyDown(document, { key: "k", metaKey: isMacOS(), ctrlKey: !isMacOS() })
    fireEvent.change(screen.getByRole("combobox"), { target: { value: "sound" } })
    await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
    expect(screen.getByRole("alert")).toHaveTextContent("Could not open")
    expect(noteInteraction).not.toHaveBeenCalledWith({
      kind: "appSearchResultOpened",
      category: "setting",
    })
  })

  it("searches agent session filters and selects the Sessions parent", async () => {
    render(<MainWindowView />)
    act(() =>
      activitySession().setEntries([
        sessionEntry({ agent: "claude-code", sessionId: "claude-session" }),
        sessionEntry({ agent: "codex", sessionId: "codex-session" }),
      ]),
    )
    for (const [label, agent] of [
      ["Claude Code Sessions", "claude-code"],
      ["Codex Sessions", "codex"],
    ] as const) {
      fireEvent.click(tab("Sessions"))
      act(() =>
        activitySession().setFilters({
          agents: ["claude-code", "codex"],
          result: "failing",
          spend: "material",
        }),
      )
      fireEvent.click(tab("Overview"))
      fireEvent.click(screen.getByRole("button", { name: "Search antiburn" }))
      fireEvent.change(screen.getByRole("combobox"), { target: { value: label } })
      await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
      expect(activitySession().getSnapshot().filters, label).toEqual({
        agents: [agent],
        result: "all",
        spend: "all",
      })
      expect(tab("Sessions")).toHaveAttribute("aria-selected", "true")
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
      expect(screen.queryByRole("tab", { name: label })).not.toBeInTheDocument()
    }
  })

  it.each(["sidebar", "search"])(
    "restores the selection and the appropriate facets through %s",
    async (source) => {
      render(<MainWindowView />)
      fireEvent.click(tab("Sessions"))
      act(() => activitySession().selectEntry(sessionEntry()))
      const subject = activitySession().getSnapshot().subject
      const selectedFilters: SessionFilters = {
        agents: ["claude-code", "codex"],
        result: "failing",
        spend: "material",
      }
      act(() => activitySession().setFilters(selectedFilters))
      fireEvent.click(tab("Limits"))
      if (source === "sidebar") {
        fireEvent.click(tab("Sessions"))
      } else {
        fireEvent.click(screen.getByRole("button", { name: "Search antiburn" }))
        fireEvent.change(screen.getByRole("combobox"), { target: { value: "Sessions" } })
        await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
      }
      const expectedFilters =
        source === "sidebar" ? selectedFilters : { agents: [], result: "all", spend: "all" }
      expect(activitySession().getSnapshot()).toMatchObject({
        filters: expectedFilters,
        subject,
      })
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Back" }))
      expect(screen.getByRole("tabpanel", { name: "Limits" })).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Back" }))
      expect(activitySession().getSnapshot()).toMatchObject({
        filters: selectedFilters,
        subject,
      })
      fireEvent.click(screen.getByRole("button", { name: "Forward" }))
      fireEvent.click(screen.getByRole("button", { name: "Forward" }))
      expect(activitySession().getSnapshot()).toMatchObject({
        filters: expectedFilters,
        subject,
      })
    },
  )

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

  it("integrates window controls and drag space on Windows", () => {
    setUserAgent("Mozilla/5.0 (Windows NT 10.0)")
    setWindowWidth(900)

    const { container } = render(<MainWindowView />)

    expect(container.querySelector("header[data-tauri-drag-region]")).not.toBeNull()
    expect(screen.getByRole("button", { name: "Close window" })).toBeVisible()
  })

  it("opens Overview by default and keeps Checks and Sessions in the sidebar", () => {
    setWindowWidth(1000)
    render(<MainWindowView />)
    expect(screen.getAllByRole("tab")).toHaveLength(4)
    expect(screen.getByRole("tab", { name: "Limits" })).toBeVisible()
    expect(screen.getByRole("tab", { name: "Overview" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tabpanel", { name: "Overview" })).toBeVisible()
    expect(screen.getByText("Overview workspace")).toBeVisible()
    expect(screen.queryByText("Burn checks workspace")).toBeNull()
    fireEvent.click(screen.getByRole("tab", { name: "Checks" }))
    expect(screen.getByRole("tabpanel", { name: "Checks" })).toBeVisible()
    expect(screen.getByText("Burn checks workspace")).toBeVisible()
    fireEvent.click(screen.getByRole("tab", { name: "Sessions" }))
    expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    expect(screen.getByRole("button", { name: "Settings" })).toBeVisible()
    setWindowWidth(900)
    fireEvent(window, new Event("resize"))
    expect(screen.getByRole("tablist", { name: "Main sections" })).toBeVisible()
    expect(screen.queryByRole("button", { name: "Open navigation" })).toBeNull()
    expect(activityMocks.listSubscriptions).toBe(1)
  })
  it("lands in Sessions with the clicked recent session selected", () => {
    render(<MainWindowView />)
    fireEvent.click(tab("Sessions"))
    act(() =>
      activitySession().setFilters({
        agents: ["codex"],
        result: "failing",
        spend: "material",
      }),
    )
    fireEvent.click(tab("Overview"))
    fireEvent.click(screen.getByRole("button", { name: "Recent session" }))
    expect(screen.getByRole("tab", { name: "Sessions" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    expect(activitySession().getSnapshot().subject).toMatchObject({
      agent: "claude",
      sessionId: "recent-1",
    })
    expect(activitySession().getSnapshot().filters).toEqual({
      agents: [],
      result: "all",
      spend: "all",
    })
  })

  it("opens the existing Settings window without changing the selected section", () => {
    render(<MainWindowView />)
    fireEvent.click(screen.getByRole("button", { name: "Settings" }))
    expect(openSettingsWindow).toHaveBeenCalledExactlyOnceWith()
    expect(screen.getByRole("tab", { name: "Overview" })).toHaveAttribute(
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
      fireEvent.keyDown(screen.getByRole("tabpanel", { name: "Overview" }), {
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

  describe("Sessions navigation", () => {
    it("keeps filters out of the sidebar before and after entries load", () => {
      render(<MainWindowView />)
      const expected = ["Overview", "Limits", "Checks", "Sessions"]
      expect(screen.getAllByRole("tab").map((item) => item.textContent)).toEqual(expected)
      act(() =>
        activitySession().setEntries([
          sessionEntry({ agent: "claude-code", sessionId: "claude-session" }),
          sessionEntry({ agent: "codex", sessionId: "codex-session" }),
        ]),
      )
      expect(screen.getAllByRole("tab").map((item) => item.textContent)).toEqual(expected)
    })

    it("preserves facets while navigating to other sections and back", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Sessions"))
      const filters: SessionFilters = {
        agents: ["claude-code", "codex"],
        result: "failing",
        spend: "material",
      }
      act(() => activitySession().setFilters(filters))
      for (const name of ["Checks", "Overview", "Limits", "Sessions"]) {
        fireEvent.click(tab(name))
      }
      expect(activitySession().getSnapshot().filters).toEqual(filters)
      expect(tab("Sessions")).toHaveAttribute("aria-selected", "true")
    })

    it("lands on Checks when the popover requests that section", async () => {
      render(<MainWindowView />)
      await vi.waitFor(() => expect(ipcMocks.sectionTarget).not.toBeNull())
      expect(screen.getByRole("tabpanel", { name: "Overview" })).toBeVisible()
      act(() => {
        ipcMocks.sectionTarget!({
          revision: 1,
          destination: { section: "burnChecks", target: null },
        })
      })
      expect(screen.getByRole("tab", { name: "Checks" })).toHaveAttribute(
        "aria-selected",
        "true",
      )
      expect(screen.getByRole("tabpanel", { name: "Checks" })).toBeVisible()
    })

    it("keeps the current facets when a cross-window request selects Sessions", async () => {
      render(<MainWindowView />)
      const filters: SessionFilters = {
        agents: ["codex"],
        result: "failing",
        spend: "material",
      }
      act(() => activitySession().setFilters(filters))
      await vi.waitFor(() => expect(ipcMocks.sectionTarget).not.toBeNull())
      act(() => {
        ipcMocks.sectionTarget!({
          revision: 1,
          destination: { section: "activity", target: null },
        })
      })
      expect(activitySession().getSnapshot().filters).toEqual(filters)
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    })

    it("selects Limits, which has no cross-window target, and leaves it on a fresh cross-window request", async () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Limits"))
      expect(tab("Limits")).toHaveAttribute("aria-selected", "true")
      expect(screen.getByRole("tabpanel", { name: "Limits" })).toBeVisible()
      await vi.waitFor(() => expect(ipcMocks.sectionTarget).not.toBeNull())
      act(() => {
        ipcMocks.sectionTarget!({
          revision: 1,
          destination: { section: "burnChecks", target: null },
        })
      })
      expect(screen.getByRole("tab", { name: "Checks" })).toHaveAttribute(
        "aria-selected",
        "true",
      )
    })

    it("leaves Limits on a cross-window request that retargets the section already selected", async () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Sessions"))
      fireEvent.click(tab("Limits"))
      expect(tab("Limits")).toHaveAttribute("aria-selected", "true")
      await vi.waitFor(() => expect(ipcMocks.sectionTarget).not.toBeNull())
      act(() => {
        ipcMocks.sectionTarget!({
          revision: 1,
          destination: { section: "activity", target: null },
        })
      })
      expect(tab("Limits")).toHaveAttribute("aria-selected", "false")
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    })

    it("includes Limits in history and leaves it when search chooses another feature", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Limits"))
      fireEvent.click(screen.getByRole("button", { name: "Back" }))
      expect(screen.getByRole("tabpanel", { name: "Overview" })).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Forward" }))
      expect(screen.getByRole("tabpanel", { name: "Limits" })).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Search antiburn" }))
      fireEvent.change(screen.getByRole("combobox"), { target: { value: "Overview" } })
      fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" })
      expect(screen.getByRole("tabpanel", { name: "Overview" })).toBeVisible()
    })

    it("returns directly to Limits after opening its session with another session retained", () => {
      render(<MainWindowView />)
      fireEvent.click(screen.getByRole("button", { name: "Recent session" }))
      expect(activitySession().getSnapshot().subject?.sessionId).toBe("recent-1")
      act(() =>
        activitySession().setFilters({
          agents: ["claude-code"],
          result: "failing",
          spend: "material",
        }),
      )
      fireEvent.click(tab("Limits"))
      fireEvent.click(screen.getByRole("button", { name: "Open Limits session" }))
      expect(activitySession().getSnapshot().subject?.sessionId).toBe("limits-session")
      expect(activitySession().getSnapshot().filters).toEqual({
        agents: [],
        result: "all",
        spend: "all",
      })
      fireEvent.click(screen.getByRole("button", { name: "Back" }))
      expect(screen.getByRole("tabpanel", { name: "Limits" })).toBeVisible()
    })

    it("keeps Limits mounted after navigating away, instead of unmounting it", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Limits"))
      expect(document.querySelector("#quota-panel h1")).not.toBeNull()
      fireEvent.click(tab("Checks"))
      // Limits is hidden, not selected, but its content stays in the DOM: a
      // return visit must not tear it down and refetch.
      expect(document.querySelector("#quota-panel h1")).not.toBeNull()
    })
  })
})
