// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { StrictMode } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { NOTICE_TEXT } from "../lib/legalNotices"
import { SettingsView } from "./SettingsView"

/**
 * The settings window's persistence, through the mocked command layer.
 *
 * The window has no Save button, so "did it persist" is not something a reader
 * can check — these tests are what checks it.
 */

const invoke = vi.hoisted(() => vi.fn())
const openDialog = vi.hoisted(() => vi.fn())
const confirmDialog = vi.hoisted(() => vi.fn())
const checkForUpdate = vi.hoisted(() => vi.fn())
const closeWindow = vi.hoisted(() => vi.fn())
/** Mutable so a test can render the macOS chrome; jsdom itself has no OS. */
const platform = vi.hoisted(() => ({ mac: false }))
/** Shell event handlers the view subscribed to, by event name. */
const listeners = vi.hoisted(() => new Map<string, (event: { payload: unknown }) => void>())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, handler)
    return () => listeners.delete(name)
  }),
}))
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ close: closeWindow }),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: openDialog,
  confirm: confirmDialog,
  save: vi.fn(),
}))
vi.mock("@tauri-apps/plugin-updater", () => ({ check: checkForUpdate }))
vi.mock("../lib/platform", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return { ...actual, isMacOS: () => platform.mac }
})

/** Push a shell event at whatever subscribed to it. */
function emit(name: string, payload: unknown) {
  act(() => listeners.get(name)?.({ payload }))
}

const SETTINGS = {
  theme: "system" as const,
  activityWindowDays: 7,
  onboardingCompleted: true,
  launchAtLogin: false,
  autoUpdate: true,
  discoveryPaused: false,
  notificationsEnabled: true,
  notifyUpdateAvailable: true,
  notifyScanFailure: true,
  nudgePlacement: "menuBar" as const,
  nudgeAutoDismissSecs: 10,
  notificationSound: true,
  diskSpaceDisplay: "whenLow" as const,
  diskSpaceThresholdGb: 50,
  notifyDiskSpaceLow: true,
  milestones5h: [10, 20, 30, 40, 50, 60, 70, 80, 90, 100],
  milestonesWeekly: [10, 20, 30, 40, 50, 60, 70, 80, 90, 100],
  liveUsageEnabled: false,
}

const INFO = {
  appVersion: "0.1.0",
  debugBuild: false,
  arch: "aarch64",
  pricingCatalogVersion: "2026-08-12",
  schemaVersion: 1,
  dataDir: "/home/avery/Library/Application Support/ai.antiburn.desktop",
  indexedSessions: 42,
  databaseBytes: 3_670_016,
  updatesSupported: false,
}

const SCAN_STATUS = {
  running: false,
  completedAgents: 11,
  totalAgents: 11,
  sessions: 42,
  finishedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
  cancelled: false,
  error: null,
  agents: [],
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: Record<string, unknown>) => {
    if (command in overrides) {
      const override = overrides[command]
      if (override instanceof Error) return Promise.reject(override)
      return Promise.resolve(typeof override === "function" ? override(args) : override)
    }
    switch (command) {
      case "get_settings":
        return Promise.resolve(SETTINGS)
      case "set_settings":
        // The store answers with what it actually stored, and that is what the
        // panes must then render.
        return Promise.resolve(args?.["settings"])
      case "app_info":
        return Promise.resolve(INFO)
      case "get_scan_status":
      case "scan_now":
      case "cancel_scan":
        return Promise.resolve(SCAN_STATUS)
      case "list_repositories":
      case "list_scan_roots":
      case "refresh_repositories":
        return Promise.resolve([])
      default:
        return Promise.resolve(null)
    }
  })
}

describe("SettingsView", () => {
  beforeEach(() => {
    invoke.mockReset()
    openDialog.mockReset()
    confirmDialog.mockReset()
    checkForUpdate.mockReset()
    listeners.clear()
    delete document.documentElement.dataset["theme"]
    mockCommands()
  })

  it("persists a theme choice and applies it to the document immediately", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }))
    fireEvent.click(await screen.findByRole("radio", { name: "Dark" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: { ...SETTINGS, theme: "dark" },
      }),
    )
    // The token layer resolves the palette from this attribute, so writing it
    // *is* applying the theme.
    expect(document.documentElement.dataset["theme"]).toBe("dark")
  })

  it('choosing "system" removes the override rather than writing a third value', async () => {
    mockCommands({ get_settings: { ...SETTINGS, theme: "dark" } })
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Appearance" }))
    await waitFor(() => expect(document.documentElement.dataset["theme"]).toBe("dark"))

    fireEvent.click(await screen.findByRole("radio", { name: "System" }))

    await waitFor(() => expect(document.documentElement.dataset["theme"]).toBeUndefined())
  })

  it("persists the launch-at-login preference and describes the applied behavior", async () => {
    render(<SettingsView />)

    const toggle = await screen.findByRole("switch", { name: "Launch antiburn on startup" })
    expect(screen.getByText("Starts automatically in the menu bar.")).toBeInTheDocument()

    fireEvent.click(toggle)

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: { ...SETTINGS, launchAtLogin: true },
      }),
    )
  })

  it("reflects the launch-at-login choice made during onboarding", async () => {
    render(<SettingsView />)

    const toggle = await screen.findByRole("switch", { name: "Launch antiburn on startup" })
    expect(toggle).not.toBeChecked()

    emit("settings:changed", {
      ...SETTINGS,
      onboardingCompleted: false,
      launchAtLogin: true,
    })

    expect(toggle).toBeChecked()
  })

  it("persists the monitoring switch as the same preference the popover pauses", async () => {
    render(<SettingsView />)

    const toggle = await screen.findByRole("switch", {
      name: "Keep looking for new sessions",
    })
    // On by default: the stored preference is `discoveryPaused`, and this
    // control is its inverse, so a reader is never asked to reason about a
    // double negative.
    expect(toggle).toBeChecked()

    fireEvent.click(toggle)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: { ...SETTINGS, discoveryPaused: true },
      }),
    )
  })

  it("reports what the index holds and when it last ran a historical scan", async () => {
    render(<SettingsView />)

    expect(await screen.findByText("42 · 3.5 MB")).toBeInTheDocument()
    expect(screen.getByText(/Last scanned 5m ago/)).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Scan now" }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("scan_now"))
  })

  it("persists the activity window", async () => {
    render(<SettingsView />)

    const slider = await screen.findByRole("slider", { name: "Days of activity to show" })
    fireEvent.change(slider, { target: { value: "14" } })

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: { ...SETTINGS, activityWindowDays: 14 },
      }),
    )
  })

  it("is honest that updates are unavailable in a build without the updater", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))

    const button = await screen.findByRole("button", { name: "Check for updates" })
    // The button exists as soon as the pane renders, but stays disabled until
    // `app_info` resolves; wait for the settled state rather than racing it.
    await waitFor(() => expect(button).toBeDisabled())
    expect(
      screen.getByText(/updater is installed in packaged releases only/i),
    ).toBeInTheDocument()

    fireEvent.click(button)
    expect(checkForUpdate).not.toHaveBeenCalled()
  })

  it("renders no automatic-update control at all in a build that cannot update", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    await screen.findByText(/updater is installed in packaged releases only/i)

    // Not a disabled switch over a preference nothing reads — no switch.
    expect(
      screen.queryByRole("switch", { name: "Check for updates automatically" }),
    ).not.toBeInTheDocument()
    expect(screen.getByText(/never contacts the release feed/i)).toBeInTheDocument()
  })

  it("runs a real check when the build carries the updater", async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } })
    checkForUpdate.mockResolvedValue(null)
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    const button = await screen.findByRole("button", { name: "Check for updates" })
    // The button renders disabled and is enabled only once `app_info` resolves.
    // Clicking before that is swallowed, which is what used to make this test
    // fail on a cold run.
    await waitFor(() => expect(button).toBeEnabled())
    fireEvent.click(button)

    await waitFor(() => expect(checkForUpdate).toHaveBeenCalledTimes(1))
    expect(await screen.findByText("Up to date")).toBeInTheDocument()
  })

  it("offers the automatic-check preference, and persists it, when updates work", async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } })
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    const toggle = await screen.findByRole("switch", {
      name: "Check for updates automatically",
    })
    // The copy describes the schedule the shell actually runs.
    expect(screen.getByText(/every six hours/i)).toBeInTheDocument()

    fireEvent.click(toggle)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: { ...SETTINGS, autoUpdate: false },
      }),
    )
  })

  it("reflects what an automatic check found", async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } })
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    await screen.findByRole("switch", { name: "Check for updates automatically" })

    emit("update:status", {
      kind: "available",
      version: "0.2.0",
      message: null,
      checkedAt: new Date().toISOString(),
      automatic: true,
    })

    expect(await screen.findByText("Version 0.2.0 is available")).toBeInTheDocument()
    // The switch says when the schedule last ran, so "automatic" is an
    // observable fact rather than an assurance.
    expect(screen.getByText(/last checked/i)).toBeInTheDocument()
  })

  it("clears local session data after confirming, and says what went", async () => {
    confirmDialog.mockResolvedValue(true)
    mockCommands({ clear_local_index: 12 })
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Privacy" }))
    fireEvent.click(await screen.findByRole("button", { name: "Clear index…" }))

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1))
    const [message] = confirmDialog.mock.calls[0] as [string]
    // The confirmation answers the two things a reader could reasonably fear.
    expect(message).toMatch(/transcript files are not touched/i)
    expect(message).toMatch(/rediscover/i)

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("clear_local_index"))
    expect(await screen.findByText(/Cleared 12 sessions/)).toBeInTheDocument()
  })

  it("declining the clear confirmation removes nothing", async () => {
    confirmDialog.mockResolvedValue(false)
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Privacy" }))
    fireEvent.click(await screen.findByRole("button", { name: "Clear index…" }))

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1))
    expect(invoke).not.toHaveBeenCalledWith("clear_local_index")
  })

  it("states the local data-handling contract", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Privacy" }))

    // The contract's headlines are always on screen as disclosure labels…
    const stored = await screen.findByRole("button", {
      name: "Visibility data stays on this machine",
    })
    expect(
      screen.getByRole("button", { name: "Your work is never uploaded" }),
    ).toBeInTheDocument()

    // …and each opens into its receipts. Collapsed bodies are unmounted, so
    // the specifics genuinely appear on expansion rather than being hidden.
    fireEvent.click(stored)
    expect(
      await screen.findByText(/may keep session content and derived analysis/i),
    ).toBeInTheDocument()
    expect(screen.getByText(/nothing in this store is uploaded/i)).toBeInTheDocument()
    // Deleting a provider's own files is named as a non-feature rather than
    // left as a silence a reader would have to test for.
    expect(screen.getByText(/antiburn cannot do this, by design/i)).toBeInTheDocument()
  })

  /// The analytics section is the one place this pane describes something
  /// leaving the machine, so it is the one place vagueness would cost the most.
  it("names every field the analytics channel sends, and what it never sends", async () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByRole("tab", { name: "Privacy" }))

    expect(
      await screen.findByRole("switch", { name: "Send anonymised analytics" }),
    ).toBeInTheDocument()

    // What a reader sees without clicking anything: four headlines and
    // nothing else. Collapsed disclosures are unmounted, so this is the
    // entire always-visible contract — worth an assertion of its own, because
    // a label quietly renamed or dropped would otherwise only show up as a
    // missing body far below.
    for (const headline of [
      "Exactly what is sent",
      "The two identifiers",
      "How the starting default is chosen",
      "What happens after it arrives",
    ]) {
      expect(screen.getByRole("button", { name: headline })).toBeInTheDocument()
    }

    // The complete enumeration, one assertion per field on the wire. The
    // earlier version of this test sampled two of them, which is how five
    // fields went unnamed in the pane while the copy claimed to list them all.
    // `analytics::event::Event` is the other half of this pair, and its
    // `the_wire_payload_is_exactly_these_thirteen_fields` pins the same number
    // from the Rust side.
    const enumeration = screen.getByRole("button", { name: "Exactly what is sent" })
    fireEvent.click(enumeration)
    // The count sits beside the list it counts. The Rust guard
    // `every_document_that_counts_the_fields_counts_the_same_number` greps
    // this pane for that phrase, so it has to survive edits to this section.
    expect(screen.getByText(/thirteen fields, and these are all of them/i)).toBeInTheDocument()
    for (const field of [
      /the word .desktop./i,
      /a random id for the message, so a retry/i,
      /a random installation id/i,
      /a random id for this run of the app/i,
      /the event name/i,
      /when it happened/i,
      /when it was delivered/i,
      /your processor architecture/i,
      /a count rounded to a range/i,
      /a short label .* which setting you changed/i,
      /a second such label when an event has two things/i,
      /the app version/i,
      /your operating system/i,
    ]) {
      expect(screen.getByText(field)).toBeInTheDocument()
    }
    // And the count itself, because the copy claims a number. A field added to
    // the payload without a line here leaves the pane saying "thirteen" over a
    // list of fourteen, which is the one way this enumeration can lie while
    // every individual assertion above still passes.
    //
    // Anchored on the disclosure's own `aria-controls` rather than on a
    // neighbouring paragraph: the body is a sibling of nothing predictable,
    // and the id is the component's actual contract.
    const body = document.getElementById(enumeration.getAttribute("aria-controls") ?? "")
    expect(body?.querySelectorAll("li")).toHaveLength(13)
    // The exclusions live in the same body as the list, so a reader checking
    // one against the other does not have to open a second row to find them.
    expect(
      screen.getByText(/file paths, repository or branch names, token counts/i),
    ).toBeInTheDocument()

    // The rest of the receipts, each behind its own label. Opening them is the
    // assertion as much as the text is — a body that failed to mount would
    // read here as missing copy.
    const open = (name: string) => fireEvent.click(screen.getByRole("button", { name }))

    // Both identifiers, and the concession that a 30-day id plus a timestamp
    // on every event reveals something. All three claims share one row, so
    // all three are asserted from it.
    open("The two identifiers")
    expect(screen.getByText(/replaced every 30 days/i)).toBeInTheDocument()
    expect(screen.getByText(/roughly when antiburn is used/i)).toBeInTheDocument()
    expect(screen.getByText(/quitting antiburn ends it/i)).toBeInTheDocument()
    // Shown to everyone, because the locale read it describes happens to
    // everyone. Asserted unconditionally for the same reason — a regression
    // that gated this row would pass a test that only ran it one way.
    open("How the starting default is chosen")
    expect(
      screen.getByText(/nothing is looked up, nothing is asked of you/i),
    ).toBeInTheDocument()
    // Retention is the operator's, and the pane says so rather than promising
    // something this build cannot keep.
    open("What happens after it arrives")
    expect(
      screen.getByText(/are the operator’s decisions rather than the app’s/i),
    ).toBeInTheDocument()
  })

  /// A build with no injected endpoint cannot transmit, and the row says so
  /// rather than offering a live switch over nothing. `app_info` in these
  /// tests reports the shell's real answer, which for a test build is false.
  it("disables the analytics switch in a build that has no endpoint", async () => {
    render(<SettingsView />)
    fireEvent.click(screen.getByRole("tab", { name: "Privacy" }))

    const toggle = await screen.findByRole("switch", {
      name: "Send anonymised analytics",
    })
    expect(toggle).toBeDisabled()
    expect(toggle).not.toBeChecked()
    expect(screen.getByText(/this build has no analytics endpoint/i)).toBeInTheDocument()
  })

  it("adds a scan folder through the directory picker", async () => {
    mockCommands({ add_scan_root: ["/home/avery/work"] })
    openDialog.mockResolvedValue("/home/avery/work")
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Sources" }))
    fireEvent.click(await screen.findByRole("button", { name: "Add a folder…" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("add_scan_root", { path: "/home/avery/work" }),
    )
    expect(await screen.findByText("/home/avery/work")).toBeInTheDocument()
  })

  it("shows where the local database lives", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))

    expect(await screen.findByText(INFO.dataDir)).toBeInTheDocument()
    expect(screen.getByText(INFO.pricingCatalogVersion)).toBeInTheDocument()
  })

  it("states the licence in About without sending anyone to a browser", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))

    expect(await screen.findByText("MPL-2.0")).toBeInTheDocument()
    expect(screen.getByText(/Mozilla Public License 2\.0/)).toBeInTheDocument()
    // This repository is not public yet, so About carries no external link at
    // all rather than links that would open a browser at a 404.
    expect(document.querySelectorAll("a")).toHaveLength(0)
  })

  it("makes the full licence text readable in About, still without links", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    fireEvent.click(await screen.findByRole("button", { name: "Open licence text" }))

    // A phrase from the licence body proper, not from any label or summary.
    expect(
      await screen.findByText(/means Covered Software of a particular Contributor/),
    ).toBeInTheDocument()
    // The licence text contains bare URLs; none may become an anchor.
    expect(document.querySelectorAll("a")).toHaveLength(0)
  })

  it("shows the notice in its own view under Legal notices", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    fireEvent.click(await screen.findByRole("button", { name: "Open legal notices" }))

    // The NOTICE body is asserted by containment rather than by quoting it
    // here: its text is exempt from the source-boundary scan, this file is
    // not.
    const notices = await screen.findByText(/Copyright \(c\) 2026/)
    expect(notices.textContent).toBe(NOTICE_TEXT.trim())
    expect(document.querySelectorAll("a")).toHaveLength(0)
  })

  it("names the bundled third-party material in its own view", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    fireEvent.click(
      await screen.findByRole("button", { name: "Open third-party attributions" }),
    )

    expect(await screen.findByText("Agent brand marks")).toBeInTheDocument()
    expect(screen.getByText(/simple-icons/)).toBeInTheDocument()
    expect(screen.getByText(/CC0-1\.0/)).toBeInTheDocument()
    expect(document.querySelectorAll("a")).toHaveLength(0)
  })

  it("lands the reader on the document heading and takes them back to About", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    const opener = await screen.findByRole("button", { name: "Open legal notices" })
    fireEvent.click(opener)

    // Focus follows the surface: the button that opened this view no longer
    // exists, so leaving it behind would drop a keyboard reader onto <body>.
    const heading = await screen.findByRole("heading", { name: "Legal notices" })
    expect(heading).toHaveFocus()

    fireEvent.click(screen.getByRole("button", { name: "Back to About" }))

    // Back to the card, with focus on the row that was pressed.
    expect(await screen.findByRole("button", { name: "Open licence text" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Open legal notices" })).toHaveFocus()
  })

  it("leaves a document behind when the reader changes section", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    fireEvent.click(await screen.findByRole("button", { name: "Open licence text" }))
    expect(await screen.findByRole("heading", { name: "Licence text" })).toBeInTheDocument()

    fireEvent.click(screen.getByRole("tab", { name: "General" }))
    fireEvent.click(screen.getByRole("tab", { name: "About" }))

    // About opens on itself, not on the document somebody read last time.
    expect(await screen.findByRole("heading", { name: "About" })).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Licence text" })).not.toBeInTheDocument()
  })

  it("links About to the privacy pane instead of a support URL", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))
    fireEvent.click(
      await screen.findByRole("button", { name: "Open privacy and data handling" }),
    )

    expect(
      await screen.findByRole("button", { name: "Visibility data stays on this machine" }),
    ).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Privacy" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
  })

  it("opens About on a masthead that names the build", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "About" }))

    expect(await screen.findByText("Version 0.1.0")).toBeInTheDocument()
    // The platform half of the line depends on the host's user agent, which
    // jsdom fakes; the architecture comes from the shell and is asserted.
    expect(screen.getByText(/· aarch64$/)).toBeInTheDocument()
  })

  it("quits antiburn from the sidebar, through the shell", async () => {
    render(<SettingsView />)

    fireEvent.click(await screen.findByRole("button", { name: "Quit antiburn" }))

    // Through the shell, not by closing a window: a menu-bar app outlives its
    // windows, and only `exit(0)` is a deliberate quit.
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("quit_app"))
  })

  it("opens on the pane the shell was asked for, when the window is new", async () => {
    let requests = 0
    mockCommands({
      take_settings_pane: () => {
        requests += 1
        return requests === 1 ? "sources" : null
      },
    })
    render(
      <StrictMode>
        <SettingsView />
      </StrictMode>,
    )

    await waitFor(() =>
      expect(screen.getByRole("tab", { name: "Sources" })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    )
    expect(requests).toBe(1)
  })

  it("moves an already-open window to a requested pane", async () => {
    render(<SettingsView />)

    await screen.findByRole("switch", { name: "Launch antiburn on startup" })
    emit("settings:pane", "sources")

    await waitFor(() =>
      expect(screen.getByRole("tab", { name: "Sources" })).toHaveAttribute(
        "aria-selected",
        "true",
      ),
    )
  })

  it("resets the shared content viewport to the top when the pane changes", async () => {
    const { container } = render(<SettingsView />)

    await screen.findByRole("switch", { name: "Launch antiburn on startup" })
    const viewport = container.querySelector(".ui-scroll-viewport") as HTMLDivElement
    viewport.scrollTop = 240

    fireEvent.click(screen.getByRole("tab", { name: "About" }))

    expect(viewport.scrollTop).toBe(0)
  })

  it("ignores a pane id it does not recognize rather than rendering nothing", async () => {
    render(<SettingsView />)

    await screen.findByRole("switch", { name: "Launch antiburn on startup" })
    emit("settings:pane", "account")

    expect(screen.getByRole("tab", { name: "General" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
  })

  it("restarts onboarding after explaining what the reset keeps", async () => {
    confirmDialog.mockResolvedValue(true)
    render(<SettingsView />)

    fireEvent.click(await screen.findByRole("button", { name: "Run setup again…" }))

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1))
    const [message] = confirmDialog.mock.calls[0] as [string]
    expect(message).toMatch(/indexed sessions and current settings stay/i)
    expect(message).toMatch(/returns the next time/i)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("restart_onboarding"))
    expect(screen.getByRole("button", { name: "Run setup again…" })).toBeEnabled()
  })

  it("keeps onboarding unchanged when the restart is declined", async () => {
    confirmDialog.mockResolvedValue(false)
    render(<SettingsView />)

    fireEvent.click(await screen.findByRole("button", { name: "Run setup again…" }))

    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1))
    expect(invoke).not.toHaveBeenCalledWith("restart_onboarding")
  })

  it("keeps an actionable error visible when setup cannot open", async () => {
    confirmDialog.mockResolvedValue(true)
    mockCommands({ restart_onboarding: new Error("window failed") })
    render(<SettingsView />)

    fireEvent.click(await screen.findByRole("button", { name: "Run setup again…" }))

    expect(await screen.findByRole("status")).toHaveTextContent(/setup could not open/i)
    expect(screen.getByRole("status")).toHaveTextContent(/try again or restart antiburn/i)
  })
})

/**
 * Window chrome and shortcuts.
 *
 * On macOS the native title bar is hidden and a frontend strip is the window's
 * drag handle; everywhere else the native bar stays and no strip renders.
 * `isMacOS` is mocked mutably because jsdom has no operating system to ask.
 */
describe("SettingsView — window chrome", () => {
  beforeEach(() => {
    invoke.mockReset()
    openDialog.mockReset()
    confirmDialog.mockReset()
    checkForUpdate.mockReset()
    closeWindow.mockReset()
    platform.mac = false
    listeners.clear()
    delete document.documentElement.dataset["theme"]
    mockCommands()
  })

  it("renders the drag strip on macOS, empty and inert", async () => {
    platform.mac = true
    const { container } = render(<SettingsView />)
    await screen.findByRole("switch", { name: "Launch antiburn on startup" })

    const strip = container.querySelector("[data-tauri-drag-region]")
    expect(strip).not.toBeNull()
    // A drag starts only when the mousedown lands on the strip itself, so it
    // must never grow children that would eat the drag.
    expect(strip).toBeEmptyDOMElement()
    expect(strip).toHaveAttribute("aria-hidden", "true")
  })

  it("renders no drag strip where the native title bar exists", async () => {
    const { container } = render(<SettingsView />)
    await screen.findByRole("switch", { name: "Launch antiburn on startup" })

    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
  })

  it("closes the window on ⌘W, as a request the shell may turn into a hide", async () => {
    render(<SettingsView />)
    await screen.findByRole("switch", { name: "Launch antiburn on startup" })

    fireEvent.keyDown(document, { key: "w", metaKey: true })

    await waitFor(() => expect(closeWindow).toHaveBeenCalledTimes(1))
  })

  it("does not close on Escape: a settings window is not a modal", async () => {
    render(<SettingsView />)
    await screen.findByRole("switch", { name: "Launch antiburn on startup" })

    fireEvent.keyDown(document, { key: "Escape" })

    expect(closeWindow).not.toHaveBeenCalled()
  })

  it("orders the sidebar with everyday panes first and provenance last", async () => {
    render(<SettingsView />)
    await screen.findByRole("switch", { name: "Launch antiburn on startup" })

    expect(screen.getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
      "General",
      "Privacy",
      "Notifications",
      "Usage",
      "Sources",
      "Appearance",
      "About",
    ])
  })
})

/**
 * Notifications.
 *
 * The pane's whole job is to let a reader decide what may interrupt them, so
 * these tests check the two-level gate and the same honesty rule the Updates
 * pane follows: no control over a notification this build could never post.
 */
describe("SettingsView — notifications", () => {
  beforeEach(() => {
    invoke.mockReset()
    openDialog.mockReset()
    confirmDialog.mockReset()
    checkForUpdate.mockReset()
    listeners.clear()
    delete document.documentElement.dataset["theme"]
    mockCommands()
  })

  it("names every notification rather than describing a category", async () => {
    mockCommands({ app_info: { ...INFO, updatesSupported: true } })
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Notifications" }))

    expect(await screen.findByRole("switch", { name: "Notify me" })).toBeChecked()
    // The update and scan kinds have no rows; the master switch's own copy
    // names them, so allowing notifications is still informed consent.
    expect(
      screen.getByText(/a newer version, a scan that could not finish/i),
    ).toBeInTheDocument()
    const fiveHour = screen.getByRole("group", {
      name: "Five-hour milestone thresholds",
    })
    expect(within(fiveHour).getAllByRole("checkbox")).toHaveLength(20)
    const five = within(fiveHour).getByRole("checkbox", { name: "5%" })
    const ten = within(fiveHour).getByRole("checkbox", { name: "10%" })
    expect(five).toHaveAttribute("aria-checked", "false")
    expect(five.querySelector('[aria-hidden="true"]')).toHaveClass("bg-input-fill")
    expect(five.querySelector('[aria-hidden="true"]')).not.toHaveClass("bg-accent-fill")
    expect(ten).toHaveAttribute("aria-checked", "true")
    expect(ten.querySelector('[aria-hidden="true"]')).toHaveClass("bg-accent-fill")
    expect(ten.querySelector('[aria-hidden="true"]')).not.toHaveClass("bg-input-fill")
    expect(
      screen.getByRole("group", { name: "Weekly milestone thresholds" }),
    ).toBeInTheDocument()
    // The milestone rows say plainly that no live source ships yet (D-20).
    expect(
      screen.getByText(/fire only while Settings → Usage is set to refresh/i),
    ).toBeInTheDocument()
  })

  it("shows a test notification through the shell", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Notifications" }))
    fireEvent.click(await screen.findByRole("button", { name: "Show test" }))

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("post_test_notification"))
  })

  it("posts a sample of one kind from the debug row", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Notifications" }))
    fireEvent.click(await screen.findByRole("button", { name: "Milestone" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("post_sample_notification", {
        kind: "usageMilestone",
      }),
    )
  })

  it("persists an individual five-percent milestone choice", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Notifications" }))
    const group = await screen.findByRole("group", {
      name: "Five-hour milestone thresholds",
    })
    const five = within(group).getByRole("checkbox", { name: "5%" })
    expect(five).toHaveAttribute("aria-checked", "false")

    fireEvent.click(five)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: {
          ...SETTINGS,
          milestones5h: [5, ...SETTINGS.milestones5h],
        },
      }),
    )
    expect(five).toHaveAttribute("aria-checked", "true")
    expect(five.querySelector('[aria-hidden="true"]')).toHaveClass("bg-accent-fill")
  })

  it("selects and clears every milestone in one window class", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Notifications" }))
    const group = await screen.findByRole("group", {
      name: "Weekly milestone thresholds",
    })

    fireEvent.click(within(group).getByRole("button", { name: "Select all" }))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: {
          ...SETTINGS,
          milestonesWeekly: [
            5, 10, 15, 20, 25, 30, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 85, 90, 95, 100,
          ],
        },
      }),
    )

    fireEvent.click(within(group).getByRole("button", { name: "Clear all" }))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: {
          ...SETTINGS,
          milestonesWeekly: [],
        },
      }),
    )
  })

  it("persists the master switch", async () => {
    render(<SettingsView />)

    fireEvent.click(screen.getByRole("tab", { name: "Notifications" }))
    fireEvent.click(await screen.findByRole("switch", { name: "Notify me" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: { ...SETTINGS, notificationsEnabled: false },
      }),
    )
  })
})
