import { fireEvent, render, screen, within } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type { IntegrationPhase, IntegrationStatus } from "../../../../lib/agentIntegrationsIpc"
import {
  AgentConnectionDetails,
  integrationBadge,
} from "../../../settings/AgentConnectionDetails"
import { AgentSettingsList, type AgentSettingsRow } from "./AgentSettingsList"

const status: IntegrationStatus = {
  vendor: "codex",
  phase: "registered",
  reason: "registered",
  owned: true,
  hostVersion: null,
  nodeVersion: null,
  profile: null,
}
const rows: AgentSettingsRow[] = [
  {
    slug: "codex",
    sessions: 3,
    status: { found: true, facts: "3 sessions", note: "Signed in" },
    searched: true,
    locations: [{ path: "~/.codex/sessions", found: true }],
    enabled: true,
    connection: status,
  },
  {
    slug: "cursor",
    sessions: 0,
    status: { found: false, facts: "", note: "" },
    searched: true,
    locations: [],
    enabled: true,
    connection: { ...status, vendor: "cursor", phase: "pending_native_gate", owned: false },
  },
  {
    slug: "antigravity",
    sessions: 0,
    status: { found: false, facts: "", note: "" },
    searched: false,
    locations: [],
    enabled: true,
    connection: undefined,
  },
]
function setup(overrides: Partial<Parameters<typeof AgentSettingsList>[0]> = {}) {
  const change = vi.fn()
  render(
    <AgentSettingsList
      rows={rows}
      busy={false}
      onOpenCli={vi.fn()}
      onEnabledChange={change}
      {...overrides}
    />,
  )
  return change
}

describe("expanded Agents rows", () => {
  it("starts collapsed and permits only one expanded row", () => {
    setup()
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    expect(screen.getAllByRole("tab").map((el) => el.textContent)).toEqual([
      "Session locations",
      "Integration",
    ])
    fireEvent.click(screen.getByRole("button", { name: "Expand Cursor" }))
    expect(screen.getByRole("button", { name: "Expand Codex" })).toHaveAttribute(
      "aria-expanded",
      "false",
    )
    expect(screen.getAllByRole("tablist")).toHaveLength(1)
    fireEvent.click(screen.getByRole("tab", { name: "Session locations" }))
    expect(screen.getByText("No session locations available.")).toBeInTheDocument()
  })
  it.each([
    ["registered", "registered", "success", "Integration connected."],
    ["awaiting_trust", "awaiting_trust", "attention", "Approval required"],
    ["reload_required", "reload_required", "attention", "Restart required"],
    ["user_changed", "user_changed", "attention", "Setup changed"],
  ] as const)(
    "keeps the %s status icon when switching tabs",
    (phase, reason, tone, description) => {
      setup({ rows: [{ ...rows[0]!, connection: { ...status, phase, reason } }] })
      const warning = screen.queryByRole("button", {
        name: "Open Antiburn integration for Codex",
      })
      if (tone === "attention") expect(warning).toBeInTheDocument()
      else expect(warning).not.toBeInTheDocument()
      fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
      const tab = screen.getByRole("tab", { name: "Integration" })
      expect(tab).toHaveAttribute("aria-selected", "false")
      expect(tab).toHaveAccessibleDescription(description)
      expect(tab.querySelector("svg")).toHaveClass(`agent-settings-badge-${tone}`)
      fireEvent.click(screen.getByRole("tab", { name: "Session locations" }))
      expect(tab).toHaveAttribute("aria-selected", "false")
      expect(tab.querySelector("svg")).toHaveClass(`agent-settings-badge-${tone}`)
    },
  )
  it("omits the status slot when an unavailable integration has no icon", () => {
    setup({ rows: [rows[1]!] })
    fireEvent.click(screen.getByRole("button", { name: "Expand Cursor" }))
    const tab = screen.getByRole("tab", { name: "Integration" })
    expect(tab.querySelector("span")).toBeNull()
    expect(tab.querySelector("svg")).toBeNull()
    fireEvent.click(tab)
    expect(tab.querySelector("span")).toBeNull()
  })

  it("retains closing content for motion but removes it from keyboard and accessibility access", () => {
    setup()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    const tab = screen.getByRole("tab", { name: "Session locations" })
    const collapse = tab.closest(".agent-settings-collapse")!
    fireEvent.click(screen.getByRole("button", { name: "Expand Cursor" }))
    expect(collapse).toHaveAttribute("inert")
    expect(collapse).toHaveAttribute("aria-hidden", "true")
    expect(collapse).toHaveAttribute("data-expanded", "false")
    expect(collapse).toContainElement(tab)
    expect(screen.getAllByRole("tablist")).toHaveLength(1)
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    expect(collapse).not.toHaveAttribute("inert")
    expect(screen.getByRole("tab", { name: "Session locations" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
  })

  it("supports arrow keys, wrapping, Home and End with panel associations", () => {
    setup()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    const locations = screen.getByRole("tab", { name: "Session locations" })
    const integration = screen.getByRole("tab", { name: "Integration" })
    fireEvent.keyDown(locations, { key: "ArrowRight" })
    expect(integration).toHaveFocus()
    expect(integration).toHaveAttribute("aria-selected", "true")
    expect(screen.getByRole("tabpanel")).toHaveAttribute("aria-labelledby", integration.id)
    expect(integration).toHaveAttribute("aria-controls", screen.getByRole("tabpanel").id)
    fireEvent.keyDown(integration, { key: "ArrowRight" })
    expect(locations).toHaveFocus()
    fireEvent.keyDown(locations, { key: "End" })
    expect(integration).toHaveFocus()
    fireEvent.keyDown(integration, { key: "Home" })
    expect(locations).toHaveFocus()
    fireEvent.keyDown(locations, { key: "ArrowLeft" })
    expect(integration).toHaveFocus()
  })
  it("groups desktop detection and login in a muted row badge", () => {
    setup({
      rows: [{ ...rows[0]!, accountLabel: "Signed in", desktopAppLabel: "Codex Desktop" }],
    })
    const badge = screen.getByText("Desktop · Signed in")
    expect(badge).toHaveClass("bg-surface-card", "text-label-secondary", "type-caption")
    expect(screen.getByText("Searched 1 location")).toBeInTheDocument()
    expect(screen.getByText("3 sessions")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Expand Codex" })).toHaveAccessibleDescription(
      "Desktop · Signed in · Searched 1 location · 3 sessions",
    )
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    expect(screen.getAllByText("Desktop · Signed in")).toHaveLength(1)
    expect(screen.queryByText("Codex Desktop")).not.toBeInTheDocument()
  })

  it("does not infer a desktop login or a Terminal source", () => {
    setup({ rows: [{ ...rows[0]!, accountLabel: "", desktopAppLabel: "Codex Desktop" }] })
    expect(screen.getByText("Desktop")).toBeInTheDocument()
    expect(screen.queryByText(/Signed in|Terminal/)).not.toBeInTheDocument()
  })

  it("shows empty-session guidance only when search locations exist", () => {
    setup({ rows: [{ ...rows[1]!, locations: [{ path: "~/.cursor", found: false }] }] })
    expect(screen.getByText("No sessions yet")).toBeInTheDocument()
    expect(screen.queryByText("Not found")).not.toBeInTheDocument()
  })
  it("omits empty-session guidance and its separator without search locations", () => {
    setup({ rows: [rows[1]!] })
    expect(screen.queryByText("No sessions yet")).not.toBeInTheDocument()
    expect(screen.queryByText("·")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Expand Cursor" })).toHaveAccessibleDescription(
      "Searched 0 locations",
    )
  })

  it("keeps unconfigured status inside Integration without a row badge", () => {
    setup({
      rows: [
        {
          ...rows[0]!,
          connection: {
            ...status,
            phase: "unavailable",
            reason: "setup_not_registered",
            owned: false,
          },
        },
      ],
    })
    expect(
      screen.queryByRole("button", { name: "Open Antiburn integration for Codex" }),
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    expect(screen.getByRole("tab", { name: "Integration" })).toHaveAttribute(
      "aria-selected",
      "true",
    )
    expect(screen.getByRole("tab", { name: "Integration" }).querySelector("svg")).toHaveClass(
      "lucide-circle",
    )
    expect(
      within(screen.getByRole("region", { name: "Codex Antiburn integration" })).getByText(
        "Integration isn’t set up.",
      ),
    ).toBeInTheDocument()
    expect(screen.getByText(/Reopen Antiburn to retry automatic setup/)).toBeInTheDocument()
    expect(screen.queryByText("Setup unavailable")).not.toBeInTheDocument()
  })

  it("keeps switches separate from disclosures and badge controls", () => {
    const change = setup()
    fireEvent.click(screen.getByRole("switch", { name: "Show Codex sessions" }))
    expect(change).toHaveBeenCalledWith("codex", false)
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument()
    expect(screen.getByText("Searched 0 locations")).toBeInTheDocument()
    expect(screen.getByText("3 sessions")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Expand Codex" })).toHaveAccessibleDescription(
      "Signed in · Searched 1 location · 3 sessions",
    )
    expect(screen.queryByText("Not found")).not.toBeInTheDocument()
    const unavailable = screen.getByRole("switch", { name: "Show Cursor sessions" })
    expect(unavailable).toBeDisabled()
    expect(unavailable).not.toBeChecked()
    expect(unavailable).toHaveAccessibleDescription(
      "Unavailable until Antiburn detects this agent.",
    )
    fireEvent.click(unavailable)
    expect(change).toHaveBeenCalledTimes(1)
    expect(screen.getByText("Searching…")).toBeInTheDocument()
  })
  it.each(["account", "location"])(
    "enables a detected empty agent from %s evidence",
    (evidence) => {
      const change = setup({
        rows: [
          {
            ...rows[1]!,
            enabled: false,
            status: { found: evidence === "account", facts: "", note: "" },
            locations: evidence === "location" ? [{ path: "~/.cursor", found: true }] : [],
          },
        ],
      })
      expect(
        screen.getByText(
          evidence === "location" ? "Searched 1 location" : "Searched 0 locations",
        ),
      ).toBeInTheDocument()
      const toggle = screen.getByRole("switch", { name: "Show Cursor sessions" })
      expect(toggle).toBeEnabled()
      expect(toggle).not.toBeChecked()
      fireEvent.click(toggle)
      expect(change).toHaveBeenCalledWith("cursor", true)
    },
  )
  it("does not treat a missing session root or integration setup as agent detection", () => {
    setup({
      rows: [
        {
          ...rows[1]!,
          connection: { ...status, vendor: "cursor" },
          locations: [{ path: "~/.cursor", found: false }],
        },
      ],
    })
    expect(screen.getByText("Searched 1 location")).toBeInTheDocument()
    expect(screen.getByRole("switch", { name: "Show Cursor sessions" })).toBeDisabled()
  })
  it("keeps an incomplete scan distinct from a completed empty scan", () => {
    setup({ rows: [{ ...rows[0]!, sessions: 0, searched: false }] })
    expect(screen.getByText("Searching…")).toBeInTheDocument()
    expect(screen.queryByText(/Searched \d+ locations?/)).not.toBeInTheDocument()
    expect(screen.getByRole("switch", { name: "Show Codex sessions" })).toBeEnabled()
  })
  it("offers discovery without Integration for unsupported agents", () => {
    setup()
    fireEvent.click(screen.getByRole("button", { name: "Expand Antigravity" }))
    expect(screen.getByRole("tab", { name: "Session locations" })).toBeInTheDocument()
    expect(screen.queryByRole("tab", { name: "Integration" })).not.toBeInTheDocument()
  })
  it("omits missing technical metadata and labels the method accurately", () => {
    setup()
    fireEvent.click(screen.getByRole("button", { name: "Expand Codex" }))
    fireEvent.click(screen.getByRole("tab", { name: "Integration" }))
    expect(screen.getByText("Hooks")).toBeInTheDocument()
    for (const label of ["Agent version", "Node.js", "Profile"])
      expect(screen.queryByText(label)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Expand Cursor" }))
    expect(
      within(screen.getByRole("tablist")).getByRole("tab", { name: "Session locations" }),
    ).toHaveAttribute("aria-selected", "true")
  })
})

describe("integration badge semantics", () => {
  it("explains automatic updates for a missing runtime without an unavailable refresh action", () => {
    render(
      <AgentConnectionDetails
        status={{ ...status, phase: "unavailable", reason: "managed_cli_unavailable" }}
        name="Codex"
        busy={false}
        onOpenCli={vi.fn()}
        onFocusIntegration={vi.fn()}
      />,
    )
    expect(
      screen.getByText(/Repair it in CLI settings. This status updates automatically./),
    ).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Refresh status" })).not.toBeInTheDocument()
  })
  it.each<[IntegrationPhase, boolean, string | null]>([
    ["registered", true, "success"],
    ["active", true, "success"],
    ["registered", false, null],
    ["active", false, null],
    ["awaiting_trust", true, "attention"],
    ["reload_required", true, "attention"],
    ["user_changed", false, "attention"],
    ["not_detected", false, "neutral"],
    ["pending_native_gate", false, null],
    ["unavailable", true, null],
    ["disabled", true, "neutral"],
  ])("maps %s (owned=%s) to %s", (phase, owned, tone) => {
    expect(integrationBadge({ ...status, phase, owned }, "Codex")?.tone ?? null).toBe(tone)
  })
  it.each([
    "development_build",
    "startup_pending",
    "compatible_node_22_required",
    "managed_cli_unavailable",
  ])("suppresses badges for shared blocker %s", (reason) => {
    expect(integrationBadge({ ...status, reason }, "Codex")).toBeNull()
  })
  it("explains integration setup and names the host, without claiming execution", () => {
    expect(integrationBadge(status, "Codex")?.description).toBe(
      "Antiburn is integrated with Codex. Open Integration for details.",
    )
    expect(
      integrationBadge({ ...status, vendor: "claude", phase: "awaiting_trust" }, "Claude")
        ?.description,
    ).toContain("Claude Code needs approval")
  })
})
