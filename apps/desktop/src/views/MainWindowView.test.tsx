import { isMacOS } from "../lib/platform"
import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { Activity } from "lucide-react"
import { CollectionDetailPane } from "./main-window/CollectionDetailPane"
import type * as MainActivitySessionModule from "./main-window/MainActivitySession"
import type { SessionListEntry } from "../components/session/SessionList"
import type * as IpcModule from "../lib/ipc"
import type { MainWindowNavigationRequest } from "../lib/ipc"
import type { SessionSubject } from "../lib/sessionSubject"
import type { SessionFilter } from "../lib/sessionFilters"
import * as SnoozedBurnChecks from "../lib/snoozedBurnChecks"
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

/**
 * A minimal stand-in for `MainActivitySession`. `MainWindowView` only reads
 * `entries` and `filter` off its snapshot and calls `setFilter`, so the fake
 * covers only that surface and lets a test drive the sidebar's counts and
 * selection without the real class's async IPC-backed engine.
 */
const activityMocks = vi.hoisted(() => {
  class FakeMainActivitySession {
    revealDetail = vi.fn()
    snapshot: {
      entries: SessionListEntry[] | null
      filter: SessionFilter
      subject: SessionSubject | null
    } = {
      entries: null,
      subject: null,
      filter: { kind: "all" },
    }
    private listeners = new Set<() => void>()
    setFilter = vi.fn((filter: SessionFilter) => {
      this.snapshot = { ...this.snapshot, filter }
      this.notify()
    })
    onNavigation?: (origin: "user" | "automatic") => void
    onDeleted?: (subject: SessionSubject) => void
    restoreNavigation = vi.fn((filter: SessionFilter, subject: SessionSubject | null) => {
      this.snapshot = { ...this.snapshot, filter, subject }
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

  it("searches agent session filters and reaches the same sidebar selection", async () => {
    render(<MainWindowView />)
    act(() =>
      activitySession().setEntries([
        sessionEntry({ agent: "claude-code", sessionId: "claude-session" }),
        sessionEntry({ agent: "codex", sessionId: "codex-session" }),
      ]),
    )
    for (const label of ["Claude Code Sessions", "Codex Sessions"]) {
      fireEvent.click(tab(label))
      const selected = activitySession().getSnapshot().filter
      fireEvent.click(tab("Overview"))
      fireEvent.click(screen.getByRole("button", { name: "Search antiburn" }))
      fireEvent.change(screen.getByRole("combobox"), { target: { value: label } })
      await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
      expect(activitySession().getSnapshot().filter, label).toEqual(selected)
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    }
  })

  it.each(["sidebar", "search"])(
    "opens All Sessions with the retained selection through %s",
    async (source) => {
      render(<MainWindowView />)
      fireEvent.click(tab("Sessions"))
      act(() => activitySession().selectEntry(sessionEntry()))
      const subject = activitySession().getSnapshot().subject
      fireEvent.click(tab("Failing Sessions"))
      fireEvent.click(tab("Limits"))
      if (source === "sidebar") {
        fireEvent.click(tab("Sessions"))
      } else {
        fireEvent.click(screen.getByRole("button", { name: "Search antiburn" }))
        fireEvent.change(screen.getByRole("combobox"), { target: { value: "Sessions" } })
        await act(async () => fireEvent.keyDown(screen.getByRole("combobox"), { key: "Enter" }))
      }
      expect(activitySession().getSnapshot()).toMatchObject({
        filter: { kind: "all" },
        subject,
      })
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Back" }))
      expect(screen.getByRole("tabpanel", { name: "Limits" })).toBeVisible()
      fireEvent.click(screen.getByRole("button", { name: "Back" }))
      expect(activitySession().getSnapshot()).toMatchObject({
        filter: { kind: "failing" },
        subject,
      })
      fireEvent.click(screen.getByRole("button", { name: "Forward" }))
      fireEvent.click(screen.getByRole("button", { name: "Forward" }))
      expect(activitySession().getSnapshot()).toMatchObject({
        filter: { kind: "all" },
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
    // Overview, Limits, Checks, Sessions, and Sessions' five fixed filter
    // children (no harness rows yet, since no entries have loaded).
    expect(screen.getAllByRole("tab")).toHaveLength(9)
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
    fireEvent.click(screen.getByRole("button", { name: "Recent session" }))
    // The "all" filter's own child row reads as selected inside Sessions.
    expect(screen.getByRole("tab", { name: "All Sessions" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    expect(activitySession().getSnapshot().subject).toMatchObject({
      agent: "claude",
      sessionId: "recent-1",
    })
    expect(activitySession().getSnapshot().filter).toEqual({ kind: "all" })
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
      expect(activitySession().getSnapshot().filter).toEqual({ kind: "failing" })
      expect(screen.getByRole("tabpanel", { name: "Sessions" })).toBeVisible()
    })

    it("omits sidebar filter counts until snoozes are ready", () => {
      const hook = vi
        .spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks")
        .mockReturnValue({ status: "loading", records: [] })
      render(<MainWindowView />)
      act(() => activitySession().setEntries([sessionEntry()]))

      expect(within(tab("All Sessions")).queryByText("1")).toBeNull()
      expect(within(tab("Failing Sessions")).queryByText("0")).toBeNull()
      hook.mockRestore()
    })

    it("resets the filter to all when the Sessions row itself is clicked", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Failing Sessions"))
      activitySession().setFilter.mockClear()
      fireEvent.click(tab("Sessions"))
      expect(activitySession().getSnapshot().filter).toEqual({ kind: "all" })
    })

    it("highlights the active filter's own child row instead of the parent", () => {
      render(<MainWindowView />)
      fireEvent.click(tab("Failing Sessions"))
      expect(tab("Failing Sessions")).toHaveAttribute("aria-selected", "true")
      expect(tab("Sessions")).toHaveAttribute("aria-selected", "false")
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

    it("keeps the current filter when a cross-window request selects Sessions", async () => {
      render(<MainWindowView />)
      await vi.waitFor(() => expect(ipcMocks.sectionTarget).not.toBeNull())
      act(() => {
        ipcMocks.sectionTarget!({
          revision: 1,
          destination: { section: "activity", target: null },
        })
      })
      expect(activitySession().setFilter).not.toHaveBeenCalled()
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
      fireEvent.click(tab("Limits"))
      fireEvent.click(screen.getByRole("button", { name: "Open Limits session" }))
      expect(activitySession().getSnapshot().subject?.sessionId).toBe("limits-session")
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
