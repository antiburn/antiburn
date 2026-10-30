import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { AgentFoundCount } from "../../lib/ipc"
import type { LiveUsageMeterPayload, LiveUsageSummaryPayload } from "../../lib/providerUsageIpc"
import {
  PromotionSession,
  type PromotionEnvironment,
} from "../../lib/promotions/PromotionSession"
import type { PromotionDefinition } from "../../promotions/types"
import { PromotionLauncher } from "./PromotionLauncher"
import { parsePromotionYaml, validatePromotionDefinition } from "../../promotions/validate"
import { PromotionHost } from "./PromotionHost"

const detection = vi.hoisted(() => ({
  counts: [] as AgentFoundCount[],
  listeners: new Set<() => void>(),
  live: { providers: [], meters: [], errors: [], generatedAt: "" } as LiveUsageSummaryPayload,
  changed: (_live: LiveUsageSummaryPayload) => {},
  fail: false,
}))
vi.mock("../../views/main-window/overview/stepSettings/agentSessionCounts", () => ({
  agentSessionCounts: () => detection.counts,
  subscribeAgentSessionCounts: (listener: () => void) => {
    detection.listeners.add(listener)
    return () => detection.listeners.delete(listener)
  },
}))
vi.mock("../../lib/ipc", () => ({
  EMPTY_LIVE_USAGE: { providers: [], meters: [], errors: [], generatedAt: "" },
  getLiveUsage: async () => {
    if (detection.fail) throw new Error("unavailable")
    return detection.live
  },
  onLiveUsageChanged: async (listener: (live: LiveUsageSummaryPayload) => void) => {
    detection.changed = listener
    return () => {
      detection.changed = () => {}
    }
  },
  noteInteraction: vi.fn(),
}))
const promo: PromotionDefinition = {
  schema_version: 1,
  id: "claude-mod",
  revision: 1,
  title: "Claude Mod",
  sidebar: {
    title: "Claude Mod",
    description: "Usage, resets, and pace above your prompt.",
    link_label: "Find out more",
    badge: "NEW",
    icon: "claude-code",
    priority: 0,
    audience: { type: "detected_agents", agents: ["claude-code"] },
  },
  screens: { announcement: { title: "Claude Mod", description: "Description" } },
}
const claude = (fields: Partial<LiveUsageMeterPayload>): LiveUsageMeterPayload => ({
  provider: "anthropic",
  displayName: "Claude",
  shown: true,
  ...fields,
})
const environment = (): PromotionEnvironment => ({
  isManualReady: () => true,
  loadReady: async () => false,
  onReady: async () => () => {},
  loadVisible: async () => true,
  onVisible: async () => () => {},
  isForeground: () => true,
  onForeground: () => () => {},
  readSeen: () => '["claude-mod@1"]',
  writeSeen: vi.fn(),
  readLauncherOpened: () => null,
  writeLauncherOpened: vi.fn(),
})
async function mount(catalog = [promo]) {
  const session = new PromotionSession(catalog, environment())
  const onOpen = vi.fn()
  render(<PromotionLauncher session={session} onOpen={onOpen} />)
  await act(async () => {
    for (let i = 0; i < 12; i++) await Promise.resolve()
  })
  return { session, onOpen }
}
beforeEach(() => {
  detection.counts = []
  detection.live = { providers: [], meters: [], errors: [], generatedAt: "" }
  detection.fail = false
})
afterEach(cleanup)

describe("promotion sidebar launcher", () => {
  it.each([
    ["Claude sessions", 2, undefined, true],
    ["installation", 0, claude({ detection: "installedNotSignedIn" }), true],
    ["sign-in", 0, claude({ detection: "signedIn" }), true],
    [
      "Claude Desktop",
      0,
      claude({ detection: "notInstalled", desktopAppLabel: "Claude Desktop" }),
      true,
    ],
    ["hidden meter", 0, claude({ detection: "signedIn", shown: false }), true],
    ["Pi-only login", 0, claude({ detection: "signedIn", carrierLabel: "Pi" }), false],
    ["unknown detection", 0, undefined, false],
    ["undetected", 0, claude({ detection: "notInstalled" }), false],
  ] as const)(
    "uses shared Agents detection for %s",
    async (_label, sessions, meter, visible) => {
      detection.counts = [{ agent: "claude-code", sessions, done: true }]
      detection.live = { ...detection.live, meters: meter ? [meter] : [] }
      await mount()
      expect(screen.queryByRole("button", { name: "Claude Mod, new" }) !== null).toBe(visible)
    },
  )
  it("keeps unknown and failed detection hidden without treating other agents as Claude", async () => {
    detection.counts = [{ agent: "codex", sessions: 10, done: true }]
    detection.fail = true
    await mount()
    expect(screen.queryByRole("button")).toBeNull()
  })
  it("updates from live detection events without starting a provider refresh", async () => {
    await mount()
    expect(screen.queryByRole("button")).toBeNull()
    act(() =>
      detection.changed({ ...detection.live, meters: [claude({ detection: "signedIn" })] }),
    )
    expect(screen.getByRole("button", { name: "Claude Mod, new" })).toBeVisible()
    act(() => detection.changed(detection.live))
    expect(screen.queryByRole("button")).toBeNull()
  })
  it("opens one named announcement, retains NEW, and keeps the launcher available", async () => {
    detection.counts = [{ agent: "claude-code", sessions: 1, done: true }]
    const { session, onOpen } = await mount()
    fireEvent.click(screen.getByRole("button", { name: "Claude Mod, new" }))
    expect(onOpen).toHaveBeenCalledOnce()
    expect(session.takeReveal()).toBe(false)
    expect(session.getSnapshot()).toMatchObject({
      selected: promo,
      screen: "announcement",
      manual: true,
    })
    expect(screen.getByRole("button", { name: "Claude Mod, new" })).toBeInTheDocument()
    expect(screen.getByText("NEW")).toBeVisible()
    act(() => session.close())
    fireEvent.click(screen.getByRole("button", { name: "Claude Mod, new" }), {
      detail: 1,
    })
    expect(onOpen).toHaveBeenCalledTimes(2)
    expect(session.takeReveal()).toBe(true)
  })
  it("hides the launcher if the shipped campaign is missing or development-only", async () => {
    detection.counts = [{ agent: "claude-code", sessions: 1, done: true }]
    await mount([{ ...promo, development_only: true }])
    expect(screen.queryByRole("button")).toBeNull()
  })
})

const generalPromo = (overrides: Record<string, unknown> = {}) =>
  validatePromotionDefinition({
    ...parsePromotionYaml(
      `schema_version: 1
id: general-feature
revision: 1
title: Feature announcement
sidebar:
  title: A different feature
  description: Available for every vendor.
  audience:
    type: all
screens:
  announcement:
    title: General feature announcement
    description: Details from the selected definition.
`,
      "synthetic-general-promo.yaml",
    ),
    ...overrides,
  })

describe("YAML-driven sidebar selection", () => {
  it("renders a generic card without detection, badge, or link, and opens its own announcement", async () => {
    const general = generalPromo()
    const { session } = await mount([promo, general])
    render(<PromotionHost session={session} />)
    const launcher = screen.getByRole("button", { name: "A different feature" })
    expect(launcher).toHaveTextContent("Available for every vendor.")
    expect(launcher.querySelector(".lucide-megaphone")).not.toBeNull()
    expect(launcher.querySelector(".promotion-launcher-badge")).toBeNull()
    expect(launcher.querySelector(".lucide-arrow-right")).toBeNull()
    fireEvent.click(launcher)
    expect(session.getSnapshot().selected?.id).toBe("general-feature")
    expect(screen.getByRole("heading", { name: "General feature announcement" })).toBeVisible()
  })

  it("chooses higher priority and breaks ties by ascending ID without changing catalog order", async () => {
    const low = generalPromo({ id: "low" })
    const high = generalPromo({
      id: "z-high",
      sidebar: { ...low.sidebar, title: "High priority", priority: 10 },
    })
    const tie = generalPromo({
      id: "a-high",
      sidebar: { ...high.sidebar, title: "Tie winner" },
    })
    const { session } = await mount([low, high, tie])
    fireEvent.click(screen.getByRole("button", { name: "Tie winner" }))
    expect(session.getSnapshot().selected?.id).toBe("a-high")
    expect(session.catalog.map((entry) => entry.id)).toEqual(["low", "z-high", "a-high"])
  })

  it("ignores high-priority promos without matching audiences, sidebar content, or production status", async () => {
    const general = generalPromo()
    const targeted = generalPromo({
      id: "targeted",
      sidebar: {
        ...general.sidebar,
        priority: 100,
        audience: { type: "detected_agents", agents: ["codex"] },
      },
    })
    const dev = generalPromo({
      id: "development",
      development_only: true,
      sidebar: { ...general.sidebar, priority: 200 },
    })
    const { sidebar: _sidebar, ...withoutSidebar } = promo
    await mount([withoutSidebar, targeted, dev, general])
    expect(screen.getAllByRole("button")).toHaveLength(1)
    expect(screen.getByRole("button", { name: "A different feature" })).toBeVisible()
  })

  it("matches any targeted agent and updates when session counts change", async () => {
    const general = generalPromo()
    const targeted = generalPromo({
      id: "targeted",
      sidebar: {
        ...general.sidebar,
        title: "For detected agents",
        priority: 10,
        icon: "codex",
        audience: { type: "detected_agents", agents: ["claude-code", "codex"] },
      },
    })
    const { session } = await mount([general, targeted])
    expect(screen.getByRole("button", { name: "A different feature" })).toBeVisible()
    act(() => {
      detection.counts = [{ agent: "codex", sessions: 2, done: true }]
      for (const listener of detection.listeners) listener()
    })
    fireEvent.click(screen.getByRole("button", { name: "For detected agents" }))
    expect(session.getSnapshot().selected?.id).toBe("targeted")
    act(() => {
      session.close()
      detection.counts = []
      for (const listener of detection.listeners) listener()
    })
    expect(screen.getByRole("button", { name: "A different feature" })).toBeVisible()
  })

  it("selects and retires an agent card as live detection changes", async () => {
    const general = generalPromo()
    const targeted = generalPromo({
      id: "targeted",
      sidebar: {
        ...general.sidebar,
        title: "Codex feature",
        priority: 10,
        audience: { type: "detected_agents", agents: ["codex"] },
      },
    })
    await mount([general, targeted])
    act(() =>
      detection.changed({
        ...detection.live,
        meters: [
          { provider: "openai", displayName: "Codex", shown: false, detection: "signedIn" },
        ],
      }),
    )
    expect(screen.getByRole("button", { name: "Codex feature" })).toBeVisible()
    act(() => detection.changed(detection.live))
    expect(screen.getByRole("button", { name: "A different feature" })).toBeVisible()
  })

  it("supports explicit generic and agent icons independently of audience", async () => {
    const general = generalPromo()
    await mount([
      generalPromo({
        sidebar: {
          ...general.sidebar,
          icon: "megaphone",
          badge: "BETA",
          link_label: "Explore",
        },
      }),
    ])
    const card = screen.getByRole("button", { name: "A different feature, beta" })
    expect(card.querySelector(".lucide-megaphone")).not.toBeNull()
    expect(card).toHaveTextContent("BETA")
    expect(card).toHaveTextContent("Explore")
    expect(card.querySelector(".lucide-arrow-right")).not.toBeNull()
    cleanup()
    await mount([generalPromo({ sidebar: { ...general.sidebar, icon: "claude-code" } })])
    expect(
      screen
        .getByRole("button", { name: "A different feature" })
        .querySelector(".lucide-megaphone"),
    ).toBeNull()
  })

  it("renders no card when every definition omits sidebar metadata", async () => {
    const { sidebar: _sidebar, ...withoutSidebar } = promo
    await mount([withoutSidebar])
    expect(screen.queryByRole("button")).toBeNull()
  })
})
