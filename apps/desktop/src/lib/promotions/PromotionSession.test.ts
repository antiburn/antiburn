import { describe, expect, it, vi } from "vitest"
import { noteInteraction } from "../ipc"
vi.mock("../ipc", () => ({ noteInteraction: vi.fn() }))
import type { PromotionDefinition } from "../../promotions/types"
import { PromotionSession, type PromotionEnvironment } from "./PromotionSession"

const promo: PromotionDefinition = {
  schema_version: 1,
  id: "example",
  revision: 1,
  title: "Example",
  screens: {
    announcement: { title: "Example", description: "Description" },
    setup: { title: "Setup", description: "Instructions", steps: [] },
  },
}
function environment(overrides: Partial<PromotionEnvironment> = {}) {
  let ready: (value: boolean) => void = () => undefined
  let visible: (value: boolean) => void = () => undefined
  let foreground: () => void = () => undefined
  const stop = vi.fn()
  const adapter: PromotionEnvironment = {
    isManualReady: () => true,
    readLauncherOpened: () => null,
    writeLauncherOpened: vi.fn(),
    loadReady: async () => true,
    loadVisible: async () => true,
    onReady: async (listener) => {
      ready = listener
      return stop
    },
    onVisible: async (listener) => {
      visible = listener
      return stop
    },
    isForeground: () => true,
    onForeground: (listener) => {
      foreground = listener
      return stop
    },
    readSeen: () => null,
    writeSeen: vi.fn(),
    ...overrides,
  }
  return {
    adapter,
    stop,
    ready: (value: boolean) => ready(value),
    visible: (value: boolean) => visible(value),
    foreground: () => foreground(),
  }
}
async function settle() {
  for (let i = 0; i < 8; i += 1) await Promise.resolve()
}

describe("PromotionSession", () => {
  it("waits for onboarding and foreground visibility, then persists only actual presentation", async () => {
    let foreground = false
    const env = environment({ loadReady: async () => false, isForeground: () => foreground })
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    expect(session.getSnapshot().selected).toBeNull()
    env.ready(true)
    expect(session.getSnapshot().selected).toBeNull()
    foreground = true
    env.foreground()
    expect(session.getSnapshot().selected?.id).toBe("example")
    expect(env.adapter.writeSeen).not.toHaveBeenCalled()
    session.presented()
    expect(env.adapter.writeSeen).toHaveBeenCalledWith('["example@1"]')
    session.close()
    env.visible(false)
    env.visible(true)
    expect(session.getSnapshot().selected).toBeNull()
    stop()
    expect(env.stop).toHaveBeenCalledTimes(3)
  })
  it.each([false, true])("keeps an open promo visible on blur (manual: %s)", async (manual) => {
    let foreground = true
    const env = environment({
      isForeground: () => foreground,
      readSeen: () => (manual ? '["example@1"]' : null),
    })
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    if (manual) session.open()
    session.navigate("setup")
    foreground = false
    env.foreground()
    expect(session.getSnapshot()).toMatchObject({
      selected: promo,
      visible: true,
      screen: "setup",
    })
    env.visible(false)
    expect(session.getSnapshot().visible).toBe(false)
    env.visible(true)
    expect(session.getSnapshot()).toMatchObject({
      selected: promo,
      visible: true,
      screen: "setup",
    })
    foreground = true
    env.foreground()
    expect(session.getSnapshot()).toMatchObject({
      selected: promo,
      visible: true,
      screen: "setup",
    })
    stop()
  })
  it("reveals once per opening without replaying on navigation or visibility changes", async () => {
    const env = environment()
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    expect(session.takeReveal()).toBe(true)
    session.navigate("setup")
    expect(session.takeReveal()).toBe(false)
    env.foreground()
    env.visible(false)
    env.visible(true)
    expect(session.takeReveal()).toBe(false)
    session.close()
    session.open()
    expect(session.takeReveal()).toBe(true)
    expect(session.takeReveal()).toBe(false)
    stop()
  })
  it("retains seen history across restarts and treats a new revision as eligible", async () => {
    const env = environment({ readSeen: () => '["example@1"]' })
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    expect(session.getSnapshot().selected).toBeNull()
    session.open()
    expect(session.getSnapshot()).toMatchObject({ selected: promo, manual: true })
    session.navigate("setup")
    expect(session.getSnapshot().screen).toBe("setup")
    session.close()
    session.open()
    expect(session.getSnapshot().screen).toBe("announcement")
    stop()
    const revised = new PromotionSession([{ ...promo, revision: 2 }], env.adapter)
    const stopRevised = revised.subscribe(() => undefined)
    await settle()
    expect(revised.getSnapshot().selected?.revision).toBe(2)
    stopRevised()
  })
  it("excludes development promos from automatic display but offers a manual catalog", async () => {
    const dummy = { ...promo, id: "dummy", development_only: true }
    const session = new PromotionSession([dummy, promo], environment().adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    expect(session.getSnapshot().selected?.id).toBe("example")
    session.open()
    expect(session.getSnapshot().screen).toBe("catalog")
    session.select("dummy")
    expect(session.getSnapshot().selected?.id).toBe("dummy")
    session.select("missing")
    expect(session.getSnapshot().selected?.id).toBe("dummy")
    stop()
  })
  it("handles unavailable or malformed storage without repeated announcements", async () => {
    const env = environment({
      readSeen: () => "not json",
      writeSeen: () => {
        throw new Error("blocked")
      },
    })
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    expect(() => session.presented()).not.toThrow()
    session.close()
    env.foreground()
    expect(session.getSnapshot().selected).toBeNull()
    stop()
  })
  it("does not let an initial read overwrite a newer settings or visibility event", async () => {
    let resolveReady!: (value: boolean) => void
    let resolveVisible!: (value: boolean) => void
    const env = environment({
      loadReady: () =>
        new Promise((resolve) => {
          resolveReady = resolve
        }),
      loadVisible: () =>
        new Promise((resolve) => {
          resolveVisible = resolve
        }),
    })
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    env.ready(true)
    env.visible(false)
    resolveReady(false)
    resolveVisible(true)
    await settle()
    expect(session.getSnapshot().selected).toBeNull()
    env.visible(true)
    expect(session.getSnapshot().selected).toEqual(promo)
    stop()
  })
  it("ignores late async subscriptions after disposal", async () => {
    const unlisten = vi.fn()
    let resolve!: (stop: () => void) => void
    const env = environment({
      onReady: () =>
        new Promise((done) => {
          resolve = done
        }),
    })
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    stop()
    resolve(unlisten)
    await settle()
    expect(unlisten).toHaveBeenCalledOnce()
    expect(session.getSnapshot().selected).toBeNull()
  })
})

describe("promotion analytics enrollment", () => {
  it("uses YAML enrollment independently of promo ID and preserves opening origin", async () => {
    vi.mocked(noteInteraction).mockClear()
    const opted: PromotionDefinition = {
      ...promo,
      id: "different-id",
      analytics: { campaign: "claude_mod" },
    }
    const session = new PromotionSession([opted], environment().adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    session.presented()
    session.presented()
    session.navigate("setup")
    session.presented()
    session.navigate("announcement")
    session.presented()
    const context = session.analyticsContext()
    session.close()
    session.open()
    session.record("command_copied", context)
    session.record("command_copied", context)
    expect(vi.mocked(noteInteraction).mock.calls.map(([event]) => event)).toEqual([
      {
        kind: "promotionInteraction",
        campaign: "claude_mod",
        action: "announcement_viewed",
        origin: "automatic",
      },
      {
        kind: "promotionInteraction",
        campaign: "claude_mod",
        action: "setup_viewed",
        origin: "automatic",
      },
      {
        kind: "promotionInteraction",
        campaign: "claude_mod",
        action: "dismissed",
        origin: "automatic",
      },
      {
        kind: "promotionInteraction",
        campaign: "claude_mod",
        action: "command_copied",
        origin: "automatic",
      },
    ])
    stop()
  })
  it("keeps omitted enrollment and development fixtures silent", async () => {
    for (const definition of [
      { ...promo, id: "claude-mod" },
      { ...promo, development_only: true, analytics: { campaign: "claude_mod" as const } },
    ]) {
      vi.mocked(noteInteraction).mockClear()
      const session = new PromotionSession([definition], environment().adapter)
      const stop = session.subscribe(() => undefined)
      await settle()
      session.open()
      session.presented()
      session.record("command_copied")
      session.close()
      expect(noteInteraction).not.toHaveBeenCalled()
      stop()
    }
  })
})

describe("sidebar promotion opening", () => {
  it("opens the named campaign directly with user analytics during the automatic cooldown", async () => {
    vi.mocked(noteInteraction).mockClear()
    const campaign = { ...promo, analytics: { campaign: "claude_mod" as const } }
    const env = environment({ loadReady: async () => false })
    const session = new PromotionSession([{ ...promo, id: "other" }, campaign], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    expect(session.getSnapshot().selected).toBeNull()
    expect(session.openFromLauncher("example")).toBe(true)
    expect(session.getSnapshot()).toMatchObject({
      selected: campaign,
      screen: "announcement",
      manual: true,
      visible: true,
      focusPanel: true,
      launcherOpened: ["example@1"],
    })
    expect(env.adapter.writeSeen).not.toHaveBeenCalled()
    expect(env.adapter.writeLauncherOpened).toHaveBeenCalledWith('["example@1"]')
    session.presented()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "promotionInteraction",
      campaign: "claude_mod",
      action: "announcement_viewed",
      origin: "user",
    })
    session.navigate("setup")
    session.presented()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "promotionInteraction",
      campaign: "claude_mod",
      action: "setup_viewed",
      origin: "user",
    })
    session.close()
    env.foreground()
    expect(session.getSnapshot().selected).toBeNull()
    expect(session.openFromLauncher("example")).toBe(true)
    expect(session.getSnapshot().screen).toBe("announcement")
    stop()
  })

  it("does not clear NEW after automatic presentation or dismissal", async () => {
    const env = environment()
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    session.presented()
    session.close()
    expect(session.getSnapshot().launcherOpened).toEqual([])
    expect(env.adapter.writeLauncherOpened).not.toHaveBeenCalled()
    stop()
  })

  it("persists sidebar activation independently of seen state and restores NEW for a revision", async () => {
    let opened: string | null = null
    const env = environment({
      readSeen: () => '["example@1"]',
      readLauncherOpened: () => opened,
      writeLauncherOpened: (value) => {
        opened = value
      },
    })
    const first = new PromotionSession([promo], env.adapter)
    const stop = first.subscribe(() => undefined)
    await settle()
    expect(first.openFromLauncher("example")).toBe(true)
    stop()
    const next = new PromotionSession([promo], env.adapter)
    const stopNext = next.subscribe(() => undefined)
    await settle()
    expect(next.getSnapshot().launcherOpened).toEqual(["example@1"])
    expect(next.getSnapshot().selected).toBeNull()
    expect(next.openFromLauncher("example")).toBe(true)
    stopNext()
    const revised = new PromotionSession([{ ...promo, revision: 2 }], env.adapter)
    const stopRevised = revised.subscribe(() => undefined)
    await settle()
    expect(revised.getSnapshot().launcherOpened).not.toContain("example@2")
    stopRevised()
  })

  it.each(["onboarding", "hidden", "missing", "development"])(
    "rejects %s opening without changing history",
    async (reason) => {
      const env = environment({
        loadReady: async () => false,
        isManualReady: () => reason !== "onboarding",
        loadVisible: async () => reason !== "hidden",
      })
      const session = new PromotionSession(
        [{ ...promo, development_only: reason === "development" }],
        env.adapter,
      )
      const stop = session.subscribe(() => undefined)
      await settle()
      expect(session.openFromLauncher(reason === "missing" ? "absent" : "example")).toBe(false)
      expect(session.getSnapshot().selected).toBeNull()
      expect(session.getSnapshot().launcherOpened).toEqual([])
      expect(env.adapter.writeLauncherOpened).not.toHaveBeenCalled()
      stop()
    },
  )

  it("keeps manual visibility gated by FTUE completion across readiness events", async () => {
    let complete = true
    const env = environment({ loadReady: async () => false, isManualReady: () => complete })
    const session = new PromotionSession([promo], env.adapter)
    const stop = session.subscribe(() => undefined)
    await settle()
    session.openFromLauncher("example")
    env.ready(false)
    expect(session.getSnapshot().visible).toBe(true)
    complete = false
    env.ready(false)
    expect(session.getSnapshot().visible).toBe(false)
    complete = true
    env.ready(false)
    expect(session.getSnapshot().visible).toBe(true)
    stop()
  })

  it.each(["not json", "{}", '[null,7,"unrelated@1"]'])(
    "handles invalid launcher history %s",
    async (stored) => {
      const env = environment({
        readLauncherOpened: () => stored,
        writeLauncherOpened: () => {
          throw new Error("blocked")
        },
      })
      const session = new PromotionSession([promo], env.adapter)
      const stop = session.subscribe(() => undefined)
      await settle()
      expect(session.getSnapshot().launcherOpened).toEqual([])
      expect(session.openFromLauncher("example")).toBe(true)
      expect(session.getSnapshot().launcherOpened).toEqual(["example@1"])
      stop()
    },
  )
})
