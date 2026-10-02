import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { ClaudeProfilesPayload } from "../../lib/claudeProfiles"
import { SourcesPane } from "./SourcesPane"

const invoke = vi.hoisted(() => vi.fn())
const openDialog = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async () => () => {}),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openDialog }))

const DEFAULT_PROFILE = {
  id: "default",
  label: "Claude",
  path: "/Users/avery/.claude",
  builtIn: true,
}

const WORK_PROFILE = {
  id: "abc123",
  label: "Claude Work",
  path: "/Users/avery/.claude-work",
  builtIn: false,
}

function payload(overrides: Partial<ClaudeProfilesPayload> = {}): ClaudeProfilesPayload {
  return {
    profiles: [DEFAULT_PROFILE],
    suggestions: [],
    maxProfiles: 16,
    maxLabelChars: 80,
    ...overrides,
  }
}

function mockCommands(overrides: Record<string, (args?: unknown) => unknown> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    const override = overrides[command]
    if (override) return Promise.resolve().then(() => override(args))
    switch (command) {
      case "list_claude_profiles":
        return Promise.resolve(payload())
      case "list_scan_roots":
      case "list_repositories":
        return Promise.resolve([])
      default:
        return Promise.resolve(null)
    }
  })
}

async function profilesCard() {
  const heading = await screen.findByRole("heading", { name: "Claude profiles" })
  const section = heading.closest("section") ?? heading.parentElement?.parentElement
  if (!section) throw new Error("Claude profiles section not found")
  return section as HTMLElement
}

beforeEach(() => {
  vi.clearAllMocks()
  mockCommands()
})

describe("Claude profiles in Sources", () => {
  it("lists the built-in profile without a remove action", async () => {
    render(<SourcesPane discoveryPaused={false} />)

    const card = await profilesCard()
    expect(await within(card).findByText("Default")).toBeInTheDocument()
    fireEvent.pointerDown(
      within(card).getByRole("button", { name: "More actions for Claude" }),
      {
        button: 0,
        ctrlKey: false,
      },
    )
    expect(await screen.findByRole("menuitem", { name: "Rename" })).toBeInTheDocument()
    expect(screen.queryByRole("menuitem", { name: "Remove" })).not.toBeInTheDocument()
  })

  it("adds a suggested folder with its suggested name", async () => {
    mockCommands({
      list_claude_profiles: () =>
        payload({
          suggestions: [{ path: WORK_PROFILE.path, label: WORK_PROFILE.label }],
        }),
      add_claude_profile: () => payload({ profiles: [DEFAULT_PROFILE, WORK_PROFILE] }),
    })
    render(<SourcesPane discoveryPaused={false} />)

    fireEvent.click(
      await screen.findByRole("button", {
        name: `Add ${WORK_PROFILE.path} as a Claude profile`,
      }),
    )
    const dialog = await screen.findByRole("dialog", { name: "Add Claude profile" })
    expect(within(dialog).getByRole("textbox", { name: "Name" })).toHaveValue("Claude Work")
    fireEvent.click(within(dialog).getByRole("button", { name: "Add profile" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("add_claude_profile", {
        label: "Claude Work",
        path: WORK_PROFILE.path,
      }),
    )
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument())
    expect(await screen.findByText("Claude Work")).toBeInTheDocument()
  })

  it("adds a picked folder and keeps the dialog open with the shell's reason on failure", async () => {
    openDialog.mockResolvedValue("/Users/avery/.claude")
    mockCommands({
      add_claude_profile: () => {
        throw "This folder is the built-in Claude profile."
      },
    })
    render(<SourcesPane discoveryPaused={false} />)

    fireEvent.click(await screen.findByRole("button", { name: /Add profile/ }))
    const dialog = await screen.findByRole("dialog", { name: "Add Claude profile" })
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), {
      target: { value: "Personal" },
    })
    fireEvent.click(within(dialog).getByRole("button", { name: /Choose/ }))
    expect(await within(dialog).findByText("/Users/avery/.claude")).toBeInTheDocument()
    fireEvent.click(within(dialog).getByRole("button", { name: "Add profile" }))

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "This folder is the built-in Claude profile.",
    )
    expect(invoke).toHaveBeenCalledWith("add_claude_profile", {
      label: "Personal",
      path: "/Users/avery/.claude",
    })
  })

  it("asks for a folder before it calls the shell", async () => {
    render(<SourcesPane discoveryPaused={false} />)

    fireEvent.click(await screen.findByRole("button", { name: /Add profile/ }))
    const dialog = await screen.findByRole("dialog", { name: "Add Claude profile" })
    fireEvent.change(within(dialog).getByRole("textbox", { name: "Name" }), {
      target: { value: "Personal" },
    })
    fireEvent.click(within(dialog).getByRole("button", { name: "Add profile" }))

    expect(await within(dialog).findByRole("alert")).toHaveTextContent(
      "Choose the Claude Code folder for this profile.",
    )
    expect(invoke).not.toHaveBeenCalledWith("add_claude_profile", expect.anything())
  })

  it("hides suggestions and disables adding at the limit", async () => {
    mockCommands({
      list_claude_profiles: () =>
        payload({
          profiles: [DEFAULT_PROFILE, WORK_PROFILE],
          suggestions: [{ path: "/Users/avery/.claude-other", label: "Claude Other" }],
          maxProfiles: 1,
        }),
    })
    render(<SourcesPane discoveryPaused={false} />)

    expect(await screen.findByText("1-profile limit reached.")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: /Add profile/ })).toBeDisabled()
    expect(screen.queryByText("Found on this computer")).not.toBeInTheDocument()
  })
})
