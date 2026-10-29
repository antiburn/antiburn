import { expect, test } from "claude-code/testing"
import type { Engine } from "claude-code/testing"
import type {
  On,
  RenderElement,
  SessionAuthorization,
  CommandInfo,
  HttpResponse,
  HttpInit,
} from "claude-code"
import type { RateLimitReading } from "./usage"

const PLUGIN = "antiburn"
const NOW = Date.parse("2026-01-01T00:00:00Z")
function retryCopy(at: number): string {
  const rounded = Math.ceil(at / 1000) * 1000
  const date = new Date(rounded)
  const time = new Intl.DateTimeFormat("en-US", {
    hour: "numeric",
    minute: "2-digit",
    second: date.getUTCSeconds() === 0 ? undefined : "2-digit",
    hour12: true,
    timeZoneName: "short",
  })
    .format(date)
    .replace(/\s*(AM|PM)\s*/, (_match, period: string) => `${period.toLowerCase()} `)
  const calendar = new Intl.DateTimeFormat("en-CA", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  })
  const day = calendar.format(date) === calendar.format(NOW) ? "" : `${calendar.format(date)} `
  return `Account retry at ${day}${time}`
}
const LIMITS = [
  { kind: "five_hour", percentUsed: 8, resetsAt: "2026-01-01T02:48:00Z" },
  { kind: "seven_day", percentUsed: 2.5 },
]
const props = (bodyColumns = 120, maxRows = 4, hasSurvey = false) => ({
  bodyColumns,
  maxRows,
  hasSurvey,
  isWorking: false,
  scroll: { offset: 0, bodyRows: maxRows },
  view: {},
})
const command = (name: string, args = "") => ({
  command: name,
  args,
  origin: { kind: "composer" as const },
  presentation: { isFullscreen: false, columns: 80 },
})

function fakeEngine(on: On) {
  const fake = {
    authorization: { kind: "bearer", handle: "test-opaque-handle" } as SessionAuthorization,
    authorizationFailure: false,
    authorizations: 0,
    requests: [] as { url: string; init?: HttpInit }[],
    response: {
      status: 200,
      ok: true,
      headers: {},
      text: JSON.stringify({
        five_hour: { utilization: 31, resets_at: "2026-01-01T04:00:00Z" },
        seven_day: { utilization: 42 },
      }),
    } as HttpResponse,
    fetch: undefined as (() => Promise<HttpResponse>) | undefined,
    commandFailure: false,
    commandForwards: 0,
    clockFailure: false,
    readClock: undefined as (() => Promise<number>) | undefined,
    now: NOW,
    rateLimits: LIMITS as RateLimitReading[],
    usageReads: 0,
    attach: undefined as (() => Promise<void>) | undefined,
    attachLimits: undefined as RateLimitReading[] | undefined,
    envReads: 0,
    commandName: PLUGIN,
    commandList: [] as CommandInfo[],
    registrationFailure: [] as string[],
    listFailure: false,
    listReads: 0,
    logs: [] as string[],
    registerCommand: undefined as (() => Promise<string>) | undefined,
    registered: [] as string[],
    argumentHints: [] as (string | undefined)[],
    forwards: 0,
    renders: 0,
    forbidden: [] as string[],
    writes: [] as string[],
    mode: undefined as string | undefined,
    readUsage: undefined as (() => Promise<RateLimitReading[]>) | undefined,
  }
  on("session.authorize", () => {
    fake.authorizations++
    if (fake.authorizationFailure) throw new Error("unsupported authorization")
    return { value: fake.authorization }
  })
  on("http.fetch", async (_$, e) => {
    fake.requests.push({ url: e.url, init: e.init })
    return { value: fake.fetch ? await fake.fetch() : fake.response }
  })
  on("env.get", () => {
    fake.envReads++
    return { value: fake.mode }
  })
  on("session.usage", async () => {
    fake.usageReads++
    return {
      value: {
        startedAt: 0,
        context: { window: 200000 },
        rateLimits: fake.readUsage ? await fake.readUsage() : fake.rateLimits,
      },
    }
  })
  on("clock.now", async () => {
    if (fake.clockFailure) throw new Error("clock unavailable")
    return { value: fake.readClock ? await fake.readClock() : fake.now }
  })
  on("session.start", () => {
    fake.forwards++
    return { cwd: "/work" }
  })
  on("classic.SessionStart", () => ({}) as never)
  on("session.end", (_$, e) => ({ sessionId: e.sessionId }))
  on("session.attach", async (_$, e) => {
    if (fake.attach) await fake.attach()
    if (fake.attachLimits) fake.rateLimits = fake.attachLimits
    return { clientId: e.clientId }
  })
  on("session.detach", (_$, e) => ({ clientId: e.clientId }))
  on("session.measure", (_$, e) => ({ changed: e.changed }))
  on("state.set", (_$, e, next) => {
    fake.writes.push(e.plugin)
    return next(e)
  })
  on("command.list", () => {
    fake.listReads++
    if (fake.listFailure) throw new Error("command list unavailable")
    return { value: fake.commandList }
  })
  on("command.register", async (_$, e) => {
    if (fake.registrationFailure.includes(e.name)) throw new Error("registration unavailable")
    fake.registered.push(e.name)
    fake.argumentHints.push(e.argumentHint)
    return {
      value: {
        command: fake.registerCommand ? await fake.registerCommand() : fake.commandName,
      },
    }
  })
  on("command.run", (_$, e) => {
    fake.commandForwards++
    if (fake.commandFailure) throw new Error("host failure")
    return { text: `host:${e.command}` }
  })
  on("ui.log", (_$, e) => {
    fake.logs.push(e.text)
    return { value: undefined }
  })
  on("ui.message", () => ({}))
  on("ui.close", () => ({ value: undefined }))
  on("ui.render", ($, e) => {
    fake.renders++
    const { Text } = $.ui.resolve(e)
    return h(
      Text,
      {},
      e.component === "CommandOutput" ? e.props.text : "host drawing",
    ) as RenderElement
  })
  on("ui.open", (_$, e) => {
    fake.forbidden.push(`pane:${e.id}`)
    return { value: { isPlaced: true } }
  })
  on("prompt.fill", (_$, _e) => {
    fake.forbidden.push("fill")
    return { isFilled: true }
  })
  on("prompt.submit", (_$, e) => {
    fake.forbidden.push("submit")
    return { text: e.text }
  })
  on("session.compact", () => {
    fake.forbidden.push("compact")
    return { messages: [] }
  })
  return fake
}

const start = ($: Engine) =>
  $.session.start({ cwd: "/work", surface: "terminal", isInteractive: true })
const measure = ($: Engine, rateLimits: RateLimitReading[]) =>
  $.session.measure({ context: { window: 200000 }, rateLimits, changed: ["rateLimits"] })

for (const mode of [undefined, "", "usage", "checks", "CHECKS", "debug", "invalid"]) {
  test(`release ignores development environment ${String(mode)} and rejects check/debug commands`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.mode = mode
    await start($)
    for (const arg of ["checks", "debug", "demo", "demo tour", "demo off"]) {
      expect((await $.command.run(command(PLUGIN, arg))).text).toContain("Unknown arguments.")
    }
    expect(fake.envReads).toBe(0)
    expect(fake.forbidden).toEqual([])
    expect(fake.registered).toEqual([PLUGIN])
    expect(fake.argumentHints).toEqual(["[refresh | help]"])
    expect(fake.writes.every((owner) => owner === PLUGIN)).toBe(true)
    expect((await $.command.run(command("other-plugin"))).text).toBe("host:other-plugin")
  })
}

for (const surface of ["terminal", "desktop"] as const) {
  test(`native readings retain exact values and unknown windows on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    expect((await band.drawn()) !== null).toBe(true)
    const details = (await $.command.run(command(PLUGIN))).text
    expect(details).toContain("5-hour: 8%. RESETS IN 2H 48M")
    expect(details).toContain("Weekly: 2.5%. Reset unavailable")
    await measure($, [{ kind: "five_hour", percentUsed: 105 }])
    expect((await $.command.run(command(PLUGIN, "details"))).text).toContain("5-hour: 105%.")
    expect((await $.command.run(command(PLUGIN))).text).toContain("Weekly: Unavailable.")
    if (surface === "terminal") {
      expect(await band.find({ text: "105%" })).toBeDefined()
      expect(await band.find({ text: "—" })).toBeDefined()
    } else {
      expect(
        (await band.findAll({ type: "Svg" })).some((node) =>
          String(node.props.alt).startsWith("5-hour: 105%."),
        ),
      ).toBe(true)
    }
    expect(fake.usageReads).toBe(1)
  })
}

test("clear and resume refresh through host commands and leave no old display state", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  await band.press({ key: "usage-detail-five_hour" })
  expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeDefined()
  for (const reason of ["clear", "resume"] as const) {
    fake.rateLimits = [{ kind: "five_hour", percentUsed: reason === "clear" ? 9 : 10 }]
    await $.session.end({ reason, sessionId: "s", resume: {} } as never)
    expect((await $.command.run(command(reason))).text).toBe(`host:${reason}`)
    expect(await band.find({ text: reason === "clear" ? "9%" : "10%" })).toBeDefined()
    expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeUndefined()
  }
  expect(fake.usageReads).toBe(3)
})

test("failed reconciliation reports unavailable and a later native event restores data", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  fake.readUsage = async () => {
    throw new Error("unavailable")
  }
  await $.classic.SessionStart({ source: "clear", model: "claude-opus-5" })
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  expect(await band.find({ text: "Usage unavailable" })).toBeDefined()
  await measure($, LIMITS)
  expect(await band.find({ text: "8%" })).toBeDefined()
  await measure($, [])
  expect(await band.find({ text: "Usage unavailable" })).toBeDefined()
  expect(await band.find({ text: "8%" })).toBeUndefined()
})

for (const failure of [false, true]) {
  test(`a delayed initial ${failure ? "failure" : "reading"} cannot supersede native measurement`, async ($, on) => {
    const fake = fakeEngine(on)
    let finish!: (limits: RateLimitReading[]) => void
    let fail!: (error: Error) => void
    let entered!: () => void
    const waiting = new Promise<void>((resolve) => {
      entered = resolve
    })
    fake.readUsage = () => {
      entered()
      return new Promise((resolve, reject) => {
        finish = resolve
        fail = reject
      })
    }
    const starting = start($)
    await waiting
    await measure($, [{ kind: "five_hour", percentUsed: 94 }])
    if (failure) fail(new Error("late failure"))
    else finish(LIMITS)
    await starting
    expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 94%.")
    expect((await $.command.run(command(PLUGIN))).text).not.toContain("Last known")
  })
}

test("session end invalidates a pending read before it can restore old quota", async ($, on) => {
  const fake = fakeEngine(on)
  let finish!: (limits: RateLimitReading[]) => void
  let entered!: () => void
  const waiting = new Promise<void>((resolve) => {
    entered = resolve
  })
  fake.readUsage = () => {
    entered()
    return new Promise((resolve) => {
      finish = resolve
    })
  }
  const starting = start($)
  await waiting
  await $.session.end({ reason: "clear", sessionId: "s", resume: {} } as never)
  finish(LIMITS)
  await starting
  expect((await $.command.run(command(PLUGIN))).text).toContain(
    "Usage isn’t available right now.",
  )
})

test("an unexpected host-assigned name does not create an alternate command", async ($, on) => {
  const fake = fakeEngine(on)
  fake.commandName = `${PLUGIN}-2`
  await start($)
  expect((await $.command.run(command(fake.commandName))).text).toBe(`host:${fake.commandName}`)
  expect(fake.logs).toContain("antiburn: /antiburn could not be registered.")
  expect((await $.command.run(command(PLUGIN))).text).toBe(`host:${PLUGIN}`)
  expect((await $.command.run(command("other-plugin", "debug"))).text).toBe("host:other-plugin")
})

test("hidden bands forward once and narrow layouts keep details available through the command", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  for (const surface of ["terminal", "desktop"] as const) {
    for (const options of [props(80, 0), props(80, 4, true)]) {
      const band = await $.ui.mount({
        plugin: PLUGIN,
        surface,
        component: "AbovePrompt",
        props: options,
      })
      expect(await band.find({ text: "host drawing" })).toBeDefined()
    }
    const narrow = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(12),
    })
    expect(await narrow.find({ text: "8%" })).toBeDefined()
  }
  expect(fake.renders >= 4).toBe(true)
  expect((await $.command.run(command(PLUGIN))).text).toContain("Even pace: 44% used by now")
})

test("terminal hover and activation are scoped to the current binding and disappear after clear", async ($, on) => {
  fakeEngine(on)
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  const key = "usage-hover-reset-five_hour"
  const old = (await band.find({ key }))!.props.props as { binding: string }
  await band.pointer({ in: key, type: "move", x: 0, y: 0 })
  expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeDefined()
  await $.classic.SessionStart({ source: "clear", model: "claude-opus-5" })
  await band.post({ binding: old.binding, hovered: true, activate: true }, { in: key })
  expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeUndefined()
})

test("measurement errors retain labeled last-known values until the next valid event", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  fake.clockFailure = true
  await measure($, [{ kind: "five_hour", percentUsed: 99 }])
  fake.clockFailure = false
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  expect(await band.find({ text: "Usage unavailable · Last known" })).toBeDefined()
  expect(await band.find({ text: "8%" })).toBeDefined()
  expect((await $.command.run(command(PLUGIN))).text).toContain(
    "Usage unavailable · Last known.",
  )
  await measure($, [{ kind: "five_hour", percentUsed: 99 }])
  expect(await band.find({ text: "99%" })).toBeDefined()
  expect(await band.find({ text: "Usage unavailable · Last known" })).toBeUndefined()
})

test("detach discards hover ownership and preserves the host result", async ($, on) => {
  fakeEngine(on)
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  const key = "usage-hover-reset-five_hour"
  await band.pointer({ in: key, type: "move", x: 0, y: 0 })
  expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeDefined()
  expect(
    await $.session.detach({ surface: "terminal", clientId: "test-client", reason: "detach" }),
  ).toEqual({ clientId: "test-client" })
  expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeUndefined()
})

test("a downstream command failure is forwarded once", async ($, on) => {
  const fake = fakeEngine(on)
  fake.commandFailure = true
  await start($)
  let failed = false
  try {
    await $.command.run(command("resume"))
  } catch {
    failed = true
  }
  expect(failed).toBe(true)
  expect(fake.commandForwards).toBe(1)
})

for (const surface of ["terminal", "desktop"] as const) {
  test(`release retains the resting brand and quota layout on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    if (surface === "terminal") {
      const brand = await band.find({ key: "usage-brand-text-slot" })
      expect(brand?.props.width).toBe(11)
      expect(await band.find({ text: " antiburn " })).toBeDefined()
    } else {
      const brand = (await band.findAll({ type: "Svg" })).find(
        (node) => node.props.alt === "antiburn",
      )
      expect(brand?.props.width).toBe(13)
      expect(brand?.props.height).toBe(13)
      expect(String(brand?.props.source)).toContain('viewBox="6.2 5.9 32.4 32.4"')
      const meters = await band.findAll({ type: "Svg" })
      expect(meters.filter((node) => String(node.props.alt).includes("8%.")).length > 0).toBe(
        true,
      )
      expect((await band.find({ key: "usage-band" }))?.props.flexDirection).toBe("row")
    }
    await measure($, [])
    if (surface === "terminal")
      expect(await band.find({ key: "usage-brand-text-slot" })).toBeDefined()
    else
      expect(
        (await band.findAll({ type: "Svg" })).find((node) => node.props.alt === "antiburn"),
      ).toBeDefined()
    expect(fake.forbidden).toEqual([])
  })
}

for (const columns of [24, 40, 59, 60, 61, 80, 120]) {
  test(`Desktop meters fill flexible slots at ${columns} columns`, async ($, on) => {
    fakeEngine(on)
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "desktop",
      component: "AbovePrompt",
      props: props(columns),
    })
    expect((await band.find({ key: "usage-band" }))?.props.width).toBe("100%")
    const meters = await band.find({ key: "usage-meters" })
    expect(meters?.props.width).toBe(0)
    expect(meters?.props.flexGrow).toBe(1)
    expect(meters?.props.minWidth).toBe(0)
    expect(meters?.props.flexDirection).toBe(columns >= 60 ? "row" : "column")
    for (const kind of ["five_hour", "seven_day"]) {
      const slot = await band.find({ key: `meter-${kind}` })
      expect(slot?.props.width).toBe(columns >= 60 ? 0 : "100%")
      expect(slot?.props.flexGrow).toBe(1)
      expect(slot?.props.minWidth).toBe(0)
      const graphic = (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith(kind === "five_hour" ? "5-hour:" : "Weekly:"),
      )
      expect(graphic?.props.width).toBe(4096)
      expect(graphic?.props.height).toBe(18)
      expect(String(graphic?.props.source)).toContain('width="100%"')
      expect(String(graphic?.props.source)).not.toContain("viewBox=")
    }
  })
}

test("working and stacked terminal layouts retain both quota readings and reset activation", async ($, on) => {
  fakeEngine(on)
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: { ...props(60), isWorking: true },
  })
  expect((await band.find({ key: "usage-meters" }))?.props.flexDirection).toBe("column")
  expect(await band.find({ text: "8%" })).toBeDefined()
  expect(await band.find({ text: "2.5%" })).toBeDefined()
  await band.press({ key: "usage-detail-five_hour" })
  expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeDefined()
  await band.press({ key: "usage-detail-five_hour" })
  expect(await band.find({ text: "RESETS IN 2H 48M" })).toBeUndefined()
})

for (const columns of [24, 40, 80, 120]) {
  test(`Desktop branding and geometry survive empty and error states at ${columns} columns`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "desktop",
      component: "AbovePrompt",
      props: props(columns),
    })
    const initial = await band.find({ key: "usage-band" })
    expect(initial?.props.height).toBe(18 / 16)
    const brand = (await band.findAll({ type: "Svg" })).find(
      (node) => node.props.alt === "antiburn",
    )
    expect(brand?.props.width).toBe(13)
    expect(
      (await band.findAll({ type: "Svg" })).some((node) =>
        String(node.props.alt).includes("Usage unavailable"),
      ),
    ).toBe(true)
    expect(await band.find({ key: "usage-refresh" })).toBeDefined()
    fake.readUsage = async () => {
      throw new Error("unavailable")
    }
    await $.classic.SessionStart({ source: "clear", model: "claude-opus-5" })
    expect((await band.find({ key: "usage-band" }))?.props.height).toBe(initial?.props.height)
    expect(
      (await band.findAll({ type: "Svg" })).some((node) =>
        String(node.props.alt).includes("Usage unavailable"),
      ),
    ).toBe(true)
    const refresh = await band.find({ key: "usage-refresh" })
    expect(refresh?.props.label).toBe("↻")
    expect(refresh?.props.dimColor).toBe(true)
    expect(await band.find({ text: "↻" })).toBeDefined()
  })
}

for (const surface of ["desktop", "terminal"] as const) {
  test(`weekly-only limits and numeric boundaries survive the split on ${surface}`, async ($, on) => {
    fakeEngine(on)
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    for (const percentUsed of [0, 1, 2.5, 8, 94, 99.5, 100, 105]) {
      await measure($, [{ kind: "seven_day", percentUsed }])
      const details = (await $.command.run(command(PLUGIN))).text
      expect(details).toContain(`Weekly: ${percentUsed}%.`)
      expect(details).toContain("5-hour: Unavailable. Starts when a message is sent")
      if (surface === "terminal")
        expect(await band.find({ text: `${percentUsed}%` })).toBeDefined()
      else
        expect(
          (await band.findAll({ type: "Svg" })).some((node) =>
            String(node.props.alt).startsWith(`Weekly: ${Math.floor(percentUsed)}%.`),
          ),
        ).toBe(true)
    }
  })
}

test("terminal pace and strip disappearance invalidate stale hover regions", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  const key = "usage-hover-pace-five_hour"
  const old = (await band.find({ key }))!.props.props as { binding: string }
  await band.pointer({ in: key, type: "move", x: 0, y: 0 })
  expect(await band.find({ text: /^Even pace:/ })).toBeDefined()
  fake.now += 3 * 3600000
  await band.redraw()
  expect(await band.find({ key })).toBeUndefined()
  fake.now = NOW
  await band.redraw()
  expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
  await band.post({ binding: old.binding, hovered: true, activate: false }, { in: key })
  expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
  await band.pointer({ in: "usage-hover-reset-five_hour", type: "move", x: 0, y: 0 })
  await band.redraw(props(120, 4, true))
  await band.redraw(props())
  expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
})

for (const surface of ["terminal", "desktop"] as const) {
  test(`secondary-only readings retain unavailable primary meters on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = [{ kind: "seven_day_opus", percentUsed: 8 }]
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    expect(await band.find({ text: "Usage unavailable" })).toBeUndefined()
    expect(await band.find({ text: "Try send a message first" })).toBeUndefined()
    if (surface === "terminal")
      expect(
        (await band.findAll({ type: "Text" })).filter((node) => node.text === "—").length,
      ).toBe(2)
    else
      expect(
        (await band.findAll({ type: "Svg" })).filter((node) =>
          String(node.props.alt).includes("Unavailable."),
        ).length,
      ).toBe(2)
    expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: Unavailable.")
  })
}

for (const surface of ["desktop", "terminal"] as const) {
  test(`website activation survives empty, ready and stale states on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    const launches: string[][] = []
    on("process.run", (_$, e) => {
      launches.push([...e.argv])
      return {
        value: {
          exitCode: 0,
          stdout: "",
          stderr: "",
          isStdoutTruncated: false,
          isStderrTruncated: false,
        },
      }
    })
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    for (const readings of [[], LIMITS]) {
      await measure($, readings)
      expect(launches).toEqual([])
      if (surface === "desktop") {
        const links = (await band.findAll({ type: "Link" })).filter(
          (node) => node.props.href === "https://antiburn.com",
        )
        expect(links).toHaveLength(1)
        expect(links[0]!.props.label).toBe("Open antiburn website")
      } else {
        expect(await band.find({ key: "usage-brand-open" })).toBeDefined()
      }
    }
    if (surface === "terminal") {
      await band.press({ key: "usage-brand-open" })
      expect(launches).toEqual([["open", "https://antiburn.com"]])
      expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
    }
    fake.clockFailure = true
    await measure($, LIMITS)
    fake.clockFailure = false
    if (surface === "desktop")
      expect(
        (await band.findAll({ type: "Link" })).some(
          (node) => node.props.href === "https://antiburn.com",
        ),
      ).toBe(true)
    else expect(await band.find({ key: "usage-brand-open" })).toBeDefined()
    expect(fake.forbidden).toEqual([])
  })
}

for (const columns of [60, 120]) {
  test(`terminal reset and pace hover preserve bounds and activated detail at ${columns} columns`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = [
      { kind: "five_hour", percentUsed: 48, resetsAt: "2026-01-01T03:00:00Z" },
      { kind: "seven_day", percentUsed: 82, resetsAt: "2026-01-04T00:00:00Z" },
    ]
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "terminal",
      component: "AbovePrompt",
      props: props(columns),
    })
    const reset = "usage-hover-reset-seven_day"
    const marker = "usage-hover-pace-seven_day"
    expect((await band.find({ key: reset }))!.props.width).toBe(6)
    expect((await band.find({ key: marker }))!.props.width).toBe(1)
    await band.pointer({ in: reset, type: "move", x: 2, y: 0 })
    expect(await band.find({ text: /^RESETS IN 3D/ })).toBeDefined()
    await band.pointer({ in: reset, type: "leave", x: 2, y: 0 })
    expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
    await band.press({ key: "usage-detail-seven_day" })
    await band.pointer({ in: marker, type: "move", x: 0, y: 0 })
    expect(await band.find({ text: /^Even pace:/ })).toBeDefined()
    expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
    await band.pointer({ in: marker, type: "move", x: 1, y: 0 })
    expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
    expect(await band.find({ text: /^RESETS IN 3D/ })).toBeDefined()
  })
}

for (const scenario of ["windows", "linux-fallback", "unavailable"] as const) {
  test(`terminal website opener stays bounded for ${scenario}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.mode = scenario === "windows" ? "Windows_NT" : undefined
    const launches: { argv: string[]; timeoutMs?: number }[] = []
    const statuses: (string | undefined)[] = []
    on("ui.status", (_$, e) => {
      statuses.push(e.text)
      return { value: undefined }
    })
    on("process.run", (_$, e) => {
      launches.push({ argv: [...e.argv], timeoutMs: e.init?.timeoutMs })
      if (scenario === "unavailable") throw new Error("opener not installed")
      return {
        value: {
          exitCode: scenario === "linux-fallback" && e.argv[0] === "open" ? 1 : 0,
          stdout: "",
          stderr: "",
          isStdoutTruncated: false,
          isStderrTruncated: false,
        },
      }
    })
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "terminal",
      component: "AbovePrompt",
      props: props(),
    })
    expect(launches).toEqual([])
    await band.press({ key: "usage-brand-open" })
    expect(launches).toEqual(
      scenario === "windows"
        ? [{ argv: ["cmd.exe", "/c", "start", "", "https://antiburn.com"], timeoutMs: 5000 }]
        : [
            { argv: ["open", "https://antiburn.com"], timeoutMs: 5000 },
            { argv: ["xdg-open", "https://antiburn.com"], timeoutMs: 5000 },
          ],
    )
    expect(statuses).toEqual(
      scenario === "unavailable" ? ["Open https://antiburn.com in your browser"] : [],
    )
  })
}

for (const columns of [24, 40]) {
  test(`settled terminal usage exposes Refresh at ${columns} columns`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "terminal",
      component: "AbovePrompt",
      props: props(columns),
    })
    expect(await band.find({ text: "Usage unavailable" })).toBeDefined()
    expect(await band.find({ key: "usage-refresh" })).toBeDefined()
    expect(await band.find({ key: "usage-pending-hint" })).toBeUndefined()
  })
}

for (const surface of ["desktop", "terminal"] as const) {
  test(`refresh recovers late host readings without a model request on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    expect(await band.find({ key: "usage-refresh" })).toBeDefined()
    if (surface === "terminal") {
      expect(await band.find({ text: "Usage unavailable" })).toBeDefined()
    }
    fake.rateLimits = LIMITS
    await $.command.run(command(PLUGIN, "refresh"))
    expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 8%.")
    expect(fake.usageReads).toBe(2)
    expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
    expect(fake.forbidden).toEqual([])
  })
}

test("Desktop attachment re-reads after the host attaches without sending a prompt", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  fake.attachLimits = LIMITS
  await $.session.attach({ surface: "desktop", clientId: "desktop:test" })
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 8%.")
  expect(fake.usageReads).toBe(2)
  expect(fake.forbidden).toEqual([])
})

test("manual refresh remains available when the host has no quota yet", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  const reply = (await $.command.run(command(PLUGIN, "refresh"))).text
  expect(reply).toContain("Usage isn’t available right now.")
  expect(reply).not.toContain("Send a message to Claude")
  expect(reply).toContain(`Run \`/${PLUGIN} refresh\` to try again.`)
  expect(fake.usageReads).toBe(2)
  expect(fake.forbidden).toEqual([])
  fake.rateLimits = LIMITS
  expect((await $.command.run(command(PLUGIN, "refresh"))).text).toBe("Usage updated.")
  expect(fake.usageReads).toBe(3)
})

test("terminal attachment forwards without another usage read", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  expect(await $.session.attach({ surface: "terminal", clientId: "terminal:test" })).toEqual({
    clientId: "terminal:test",
  })
  expect(fake.usageReads).toBe(1)
})

test("a pending Desktop refresh cannot overwrite a newer native measurement", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  let finish!: (limits: RateLimitReading[]) => void
  let entered!: () => void
  const waiting = new Promise<void>((resolve) => {
    entered = resolve
  })
  fake.readUsage = () => {
    entered()
    return new Promise((resolve) => {
      finish = resolve
    })
  }
  const attaching = $.session.attach({ surface: "desktop", clientId: "desktop:late" })
  await waiting
  await measure($, [{ kind: "five_hour", percentUsed: 94 }])
  finish(LIMITS)
  await attaching
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 94%.")
  expect(fake.forbidden).toEqual([])
})

const attachDesktop = ($: Engine) =>
  $.session.attach({ surface: "desktop", clientId: "desktop:fallback" })
const refresh = async ($: Engine) => {
  await $.command.run(command(PLUGIN, "refresh"))
  return $.command.run(command(PLUGIN))
}

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((complete) => {
    resolve = complete
  })
  return { promise, resolve }
}

test("Desktop attach fetches account usage with only opaque bearer auth and fixed headers", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  expect(fake.requests.length).toBe(0)
  await attachDesktop($)
  expect(fake.authorizations).toBe(1)
  expect(fake.requests).toEqual([
    {
      url: "https://api.anthropic.com/api/oauth/usage",
      init: {
        auth: "test-opaque-handle",
        headers: { "anthropic-beta": "oauth-2025-04-20", accept: "application/json" },
      },
    },
  ])
  const details = (await $.command.run(command(PLUGIN))).text
  expect(details).toContain("5-hour: 31%. RESETS IN 4H 0M")
  expect(details).toContain("Weekly: 42%. Reset unavailable")
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "desktop",
    component: "AbovePrompt",
    props: props(),
  })
  expect(
    (await band.findAll({ type: "Svg" })).some((node) =>
      String(node.props.alt).startsWith("5-hour: 31%."),
    ),
  ).toBe(true)
  await band.drawn()
  await measure($, [])
  await measure($, [])
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 31%.")
  expect(fake.requests.length).toBe(1)
  expect(fake.forbidden).toEqual([])
})

for (const limits of [
  LIMITS,
  [{ kind: "seven_day", percentUsed: 12 }],
  [{ kind: "seven_day_opus", percentUsed: 0 }],
]) {
  test(`any usable native reading wins without account fill: ${limits[0]!.kind}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = limits
    await start($)
    await attachDesktop($)
    await refresh($)
    expect(fake.authorizations).toBe(0)
    expect(fake.requests.length).toBe(0)
    if (limits.length === 1)
      expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: Unavailable.")
  })
}

for (const auth of [null, { kind: "api-key" as const, handle: "test-api-handle" }]) {
  test(`unsupported ${auth?.kind ?? "absent"} auth settles unavailable`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.authorization = auth
    await start($)
    await attachDesktop($)
    expect((await refresh($)).text).toContain("Usage isn’t available right now.")
    expect(fake.requests.length).toBe(0)
    expect(fake.authorizations).toBe(1)
  })
}

test("empty terminal sessions never authorize account access", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  await $.session.attach({ surface: "terminal", clientId: "terminal:fallback" })
  await refresh($)
  await measure($, [])
  expect(fake.authorizations).toBe(0)
  expect(fake.requests.length).toBe(0)
})

test("native read failures can recover with Desktop account data", async ($, on) => {
  const fake = fakeEngine(on)
  fake.readUsage = async () => {
    throw new Error("native unavailable")
  }
  await start($)
  await attachDesktop($)
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 31%.")
  expect((await $.command.run(command(PLUGIN))).text).not.toContain("Last known")
})

for (const text of [
  "not json",
  "null",
  "[]",
  "{}",
  '{"five_hour":{"utilization":"31"}}',
  '{"five_hour":{"utilization":-1}}',
  '{"unrecognized":{"utilization":31}}',
]) {
  test(`invalid account payload reports unavailable: ${text}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.response.text = text
    await start($)
    await attachDesktop($)
    expect((await refresh($)).text).toContain("Usage isn’t available right now.")
    expect(fake.requests.length).toBe(1)
  })
}

test("account validation preserves valid partial windows and rejects invalid reset values", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  fake.response.text = JSON.stringify({
    five_hour: { utilization: -1 },
    seven_day: { utilization: 105, resets_at: "bad date" },
    unknown: { utilization: 55 },
  })
  await start($)
  await attachDesktop($)
  expect((await refresh($)).text).toContain("5-hour: Unavailable.")
  expect((await refresh($)).text).toContain("Weekly: 105%. Reset unavailable")
})

for (const status of [401, 403, 500]) {
  test(`HTTP ${status} preserves prior reading and backs off explicit refresh`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    await attachDesktop($)
    fake.now += 60_000
    fake.response = { status, ok: false, headers: {}, text: "" }
    expect((await refresh($)).text).toContain("Usage unavailable · Last known.")
    expect((await refresh($)).text).toContain("5-hour: 31%.")
    expect(fake.requests.length).toBe(2)
    fake.now += 60_000
    await refresh($)
    expect(fake.requests.length).toBe(3)
    fake.now += 119_999
    await refresh($)
    expect(fake.requests.length).toBe(3)
    fake.now++
    await refresh($)
    expect(fake.requests.length).toBe(4)
  })
}

for (const failure of ["timeout", "disabled traffic"]) {
  test(`account ${failure} remains unavailable without automatic retry`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.fetch = async () => {
      throw new Error(failure)
    }
    await start($)
    await attachDesktop($)
    expect((await refresh($)).text).toContain("Usage isn’t available right now.")
    fake.now += 600_000
    await measure($, [])
    expect(fake.requests.length).toBe(1)
    await refresh($)
    expect(fake.requests.length).toBe(2)
  })
}

for (const retryAfter of ["1800", "Thu, 01 Jan 2026 00:30:00 GMT"]) {
  test(`429 honors longer Retry-After: ${retryAfter}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.response = { status: 429, ok: false, headers: { "retry-after": retryAfter }, text: "" }
    await start($)
    await attachDesktop($)
    fake.now += 1_799_999
    await refresh($)
    expect(fake.requests.length).toBe(1)
    fake.now++
    await refresh($)
    expect(fake.requests.length).toBe(2)
  })
}

test("successful account data stays fresh for 60 seconds including explicit refresh", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  await attachDesktop($)
  fake.now += 59_999
  await refresh($)
  expect(fake.requests.length).toBe(1)
  fake.now++
  await refresh($)
  expect(fake.requests.length).toBe(2)
})

test("concurrent stale refreshes share one account request and retain the latest result", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  await attachDesktop($)
  fake.now += 60_000
  const entered = deferred<void>()
  const reply = deferred<HttpResponse>()
  fake.fetch = () => {
    entered.resolve()
    return reply.promise
  }
  const first = refresh($)
  await entered.promise
  const second = refresh($)
  await Promise.resolve()
  reply.resolve({ ...fake.response, text: '{"five_hour":{"utilization":55}}' })
  await Promise.all([first, second])
  expect(fake.requests.length).toBe(2)
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 55%.")
})

for (const event of ["native", "empty", "clear", "detach"] as const) {
  test(`account completion respects ${event} event data`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    const entered = deferred<void>()
    const reply = deferred<HttpResponse>()
    fake.fetch = () => {
      entered.resolve()
      return reply.promise
    }
    await start($)
    const attaching = attachDesktop($)
    await entered.promise
    if (event === "native") await measure($, [{ kind: "five_hour", percentUsed: 94 }])
    if (event === "empty") {
      await measure($, [])
      await measure($, [])
    }
    if (event === "clear")
      await $.session.end({ reason: "clear", sessionId: "s", resume: {} } as never)
    if (event === "detach")
      await $.session.detach({
        surface: "desktop",
        clientId: "desktop:fallback",
        reason: "detach",
      })
    reply.resolve(fake.response)
    await attaching
    const details = (await $.command.run(command(PLUGIN))).text
    if (event !== "empty") expect(details).not.toContain("31%")
    expect(details).toContain(
      event === "native"
        ? "5-hour: 94%."
        : event === "empty"
          ? "5-hour: 31%."
          : "Usage isn’t available right now.",
    )
    expect(fake.requests.length).toBe(1)
  })
}

test("native events replace fallback without filling missing windows", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  await attachDesktop($)
  await measure($, [{ kind: "seven_day", percentUsed: 7 }])
  const details = (await $.command.run(command(PLUGIN))).text
  expect(details).toContain("Weekly: 7%.")
  expect(details).toContain("5-hour: Unavailable.")
  expect(fake.requests.length).toBe(1)
})

test("detach clears account readings and a new attachment fetches fresh data", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  await attachDesktop($)
  await $.session.detach({ surface: "desktop", clientId: "desktop:fallback", reason: "detach" })
  expect((await refresh($)).text).toContain("Usage isn’t available right now.")
  expect(fake.requests.length).toBe(1)
  await attachDesktop($)
  expect(fake.requests.length).toBe(2)
})

test("Desktop clear reconciliation resets freshness and uses the same account path", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  await attachDesktop($)
  await $.classic.SessionStart({ source: "clear", model: "claude-opus-5" })
  expect(fake.requests.length).toBe(2)
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 31%.")
})

test("failed fallback preserves a previously valid native reading", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  await attachDesktop($)
  fake.rateLimits = []
  fake.response = { status: 500, ok: false, headers: {}, text: "" }
  const details = (await refresh($)).text
  expect(details).toContain("Usage unavailable · Last known.")
  expect(details).toContain("5-hour: 8%.")
})

test("hosts without authorization support settle unavailable without HTTP", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  fake.authorizationFailure = true
  await start($)
  await attachDesktop($)
  expect((await refresh($)).text).toContain("Usage isn’t available right now.")
  expect(fake.requests.length).toBe(0)
})

test("new refresh applies a cached success while an older display commit waits", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  const entered = deferred<void>()
  const resume = deferred<void>()
  let pause = false
  on("state.get", async (_$, e, next) => {
    if (pause && fake.requests.length > 0) {
      pause = false
      entered.resolve()
      await resume.promise
    }
    return next(e)
  })
  await start($)
  pause = true
  const attaching = attachDesktop($)
  await entered.promise
  expect((await refresh($)).text).toContain("5-hour: 31%.")
  resume.resolve()
  await attaching
  expect(fake.requests.length).toBe(1)
})

test("unavailable native data and absent authorization settle unavailable", async ($, on) => {
  const fake = fakeEngine(on)
  fake.readUsage = async () => {
    throw new Error("native unavailable")
  }
  fake.authorization = null
  await start($)
  await attachDesktop($)
  expect((await refresh($)).text).toContain("Usage isn’t available right now.")
  expect(fake.requests.length).toBe(0)
})

test("failure backoff doubles to fifteen minutes and a success resets it", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  const success = fake.response
  fake.response = { status: 500, ok: false, headers: {}, text: "" }
  await start($)
  await attachDesktop($)
  let requests = 1
  for (const delay of [60_000, 120_000, 240_000, 480_000, 900_000, 900_000]) {
    fake.now += delay - 1
    await refresh($)
    expect(fake.requests.length).toBe(requests)
    fake.now++
    await refresh($)
    expect(fake.requests.length).toBe(++requests)
  }
  fake.response = success
  fake.now += 900_000
  await refresh($)
  fake.response = { status: 500, ok: false, headers: {}, text: "" }
  fake.now += 60_000
  await refresh($)
  const failedRequests = fake.requests.length
  fake.now += 60_000
  await refresh($)
  expect(fake.requests.length).toBe(failedRequests + 1)
})

for (const retryAfter of [
  "invalid",
  "-10",
  "0",
  "Infinity",
  "1e999",
  "Wed, 31 Dec 2025 23:00:00 GMT",
]) {
  test(`invalid or short Retry-After keeps minimum backoff: ${retryAfter}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.response = { status: 429, ok: false, headers: { "retry-after": retryAfter }, text: "" }
    await start($)
    await attachDesktop($)
    fake.now += 59_999
    await refresh($)
    expect(fake.requests.length).toBe(1)
    fake.now++
    await refresh($)
    expect(fake.requests.length).toBe(2)
  })
}

test("late host attachment cannot enable account fetch after detach", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  const entered = deferred<void>()
  const resume = deferred<void>()
  fake.attach = () => {
    entered.resolve()
    return resume.promise
  }
  await start($)
  const attaching = attachDesktop($)
  await entered.promise
  await $.session.detach({ surface: "desktop", clientId: "desktop:fallback", reason: "detach" })
  resume.resolve()
  await attaching
  await refresh($)
  expect(fake.authorizations).toBe(0)
  expect(fake.requests.length).toBe(0)
})

for (const lifecycle of ["reattach", "clear"] as const) {
  for (const oldStatus of [200, 500]) {
    test(`${lifecycle} waits for the old HTTP ${oldStatus} request before automatically fetching current usage`, async ($, on) => {
      const fake = fakeEngine(on)
      fake.rateLimits = []
      const firstEntered = deferred<void>()
      const oldReply = deferred<HttpResponse>()
      const freshRead = deferred<void>()
      let activeRequests = 0
      let maximumRequests = 0
      fake.fetch = async () => {
        activeRequests++
        maximumRequests = Math.max(maximumRequests, activeRequests)
        try {
          if (fake.requests.length === 1) {
            firstEntered.resolve()
            return await oldReply.promise
          }
          return { ...fake.response, text: '{"five_hour":{"utilization":55}}' }
        } finally {
          activeRequests--
        }
      }
      await start($)
      const initialAttach = attachDesktop($)
      await firstEntered.promise
      if (lifecycle === "reattach")
        await $.session.detach({
          surface: "desktop",
          clientId: "desktop:fallback",
          reason: "detach",
        })
      fake.readUsage = async () => {
        freshRead.resolve()
        return []
      }
      const currentEvent =
        lifecycle === "reattach"
          ? attachDesktop($)
          : $.classic.SessionStart({ source: "clear", model: "claude-opus-5" })
      await freshRead.promise
      await measure($, [])
      expect(fake.requests.length).toBe(1)
      oldReply.resolve({
        status: oldStatus,
        ok: oldStatus === 200,
        headers: {},
        text: '{"five_hour":{"utilization":31}}',
      })
      await Promise.all([initialAttach, currentEvent])
      expect(fake.requests.length).toBe(2)
      expect(maximumRequests).toBe(1)
      expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 55%.")
      expect(fake.forbidden).toEqual([])
    })
  }
}

test("a newer usable native event cancels reattachment waiting for an old account request", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  const firstEntered = deferred<void>()
  const oldReply = deferred<HttpResponse>()
  const freshRead = deferred<void>()
  fake.fetch = () => {
    firstEntered.resolve()
    return oldReply.promise
  }
  await start($)
  const initialAttach = attachDesktop($)
  await firstEntered.promise
  await $.session.detach({ surface: "desktop", clientId: "desktop:fallback", reason: "detach" })
  fake.readUsage = async () => {
    freshRead.resolve()
    return []
  }
  const reattaching = attachDesktop($)
  await freshRead.promise
  await measure($, [{ kind: "five_hour", percentUsed: 94 }])
  await measure($, [])
  oldReply.resolve(fake.response)
  await Promise.all([initialAttach, reattaching])
  expect(fake.requests.length).toBe(1)
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 94%.")
})

for (const clockStage of [1, 2]) {
  for (const event of ["empty", "native", "detach"] as const) {
    test(`${event} event ordering survives account clock await ${clockStage}`, async ($, on) => {
      const fake = fakeEngine(on)
      fake.rateLimits = []
      const entered = deferred<void>()
      const resume = deferred<number>()
      let accountClocks = 0
      fake.readClock = async () => {
        if (fake.requests.length > 0 && ++accountClocks === clockStage) {
          entered.resolve()
          return resume.promise
        }
        return fake.now
      }
      await start($)
      const attaching = attachDesktop($)
      await entered.promise
      if (event === "empty") {
        await measure($, [])
        await measure($, [])
      }
      if (event === "native") {
        await measure($, [{ kind: "five_hour", percentUsed: 94 }])
        await measure($, [])
      }
      if (event === "detach")
        await $.session.detach({
          surface: "desktop",
          clientId: "desktop:fallback",
          reason: "detach",
        })
      resume.resolve(fake.now)
      await attaching
      const details = (await $.command.run(command(PLUGIN))).text
      expect(details).toContain(
        event === "empty"
          ? "5-hour: 31%."
          : event === "native"
            ? "5-hour: 94%."
            : "Usage isn’t available right now.",
      )
      expect(fake.requests.length).toBe(1)
      expect(fake.forbidden).toEqual([])
    })
  }
}

for (const status of [200, 500]) {
  test(`pending account usage shows checking until HTTP ${status} completes`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    const entered = deferred<void>()
    const response = deferred<HttpResponse>()
    fake.fetch = () => {
      entered.resolve()
      return response.promise
    }
    const attaching = attachDesktop($)
    await entered.promise
    expect((await $.command.run(command(PLUGIN))).text).toBe("Checking usage…")
    response.resolve(
      status === 200 ? fake.response : { status, ok: false, headers: {}, text: "" },
    )
    await attaching
    const reply = (await $.command.run(command(PLUGIN))).text
    if (status === 200) expect(reply).toContain("5-hour: 31%.")
    else
      expect(reply).toBe(
        `Usage isn’t available right now. Run \`/${PLUGIN} refresh\` to try again.\n${retryCopy(NOW + 60_000)} · Refresh checks session.`,
      )
    expect(reply).not.toContain("Checking usage")
    expect(reply).not.toContain("Last known")
  })
}

test("a pending native refresh shows checking after command registration", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  const entered = deferred<void>()
  const reading = deferred<RateLimitReading[]>()
  fake.readUsage = () => {
    entered.resolve()
    return reading.promise
  }
  const refreshing = refresh($)
  await entered.promise
  expect((await $.command.run(command(PLUGIN))).text).toBe("Checking usage…")
  await measure($, [{ kind: "five_hour", percentUsed: 94 }])
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 94%.")
  reading.resolve([])
  await refreshing
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 94%.")
})

for (const surface of ["desktop", "terminal"] as const) {
  for (const status of [200, 500]) {
    test(`${surface} strip reacts from loading to ${status === 200 ? "meters" : "unavailable"} after account fetch`, async ($, on) => {
      const fake = fakeEngine(on)
      fake.rateLimits = []
      await start($)
      const band = await $.ui.mount({
        plugin: PLUGIN,
        surface,
        component: "AbovePrompt",
        props: props(),
      })
      expect(await band.find({ key: "usage-refresh" })).toBeDefined()
      const entered = deferred<void>()
      const response = deferred<HttpResponse>()
      fake.fetch = () => {
        entered.resolve()
        return response.promise
      }
      const attaching = attachDesktop($)
      await entered.promise
      expect(
        surface === "desktop"
          ? (await band.findAll({ type: "Svg" })).find(
              (node) => node.props.alt === "Checking usage...",
            )
          : await band.find({ key: "usage-checking" }),
      ).toBeDefined()
      expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
      expect(await band.find({ key: "usage-pending-hint" })).toBeUndefined()
      await band.drawn()
      expect(fake.requests.length).toBe(1)
      response.resolve(
        status === 200 ? fake.response : { status, ok: false, headers: {}, text: "" },
      )
      await attaching
      if (status === 200) {
        expect(
          surface === "desktop"
            ? (await band.findAll({ type: "Svg" })).find((node) =>
                String(node.props.alt).startsWith("5-hour: 31%."),
              )
            : await band.find({ text: "31%" }),
        ).toBeDefined()
        expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
      } else {
        expect(
          surface === "desktop"
            ? (await band.findAll({ type: "Svg" })).find(
                (node) => node.props.alt === "Usage unavailable",
              )
            : await band.find({ text: "Usage unavailable" }),
        ).toBeDefined()
        expect(await band.find({ key: "usage-refresh" })).toBeDefined()
      }
      expect(fake.requests.length).toBe(1)
      expect(fake.forbidden).toEqual([])
    })
  }

  test(`${surface} strip settles unavailable after a deferred empty native read`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    const entered = deferred<void>()
    const reading = deferred<RateLimitReading[]>()
    fake.readUsage = () => {
      entered.resolve()
      return reading.promise
    }
    const refreshing = refresh($)
    await entered.promise
    expect(
      surface === "desktop"
        ? (await band.findAll({ type: "Svg" })).find(
            (node) => node.props.alt === "Checking usage...",
          )
        : await band.find({ key: "usage-checking" }),
    ).toBeDefined()
    expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
    reading.resolve([])
    await refreshing
    expect(
      surface === "desktop"
        ? (await band.findAll({ type: "Svg" })).find(
            (node) => node.props.alt === "Usage unavailable",
          )
        : await band.find({ text: "Usage unavailable" }),
    ).toBeDefined()
    expect(await band.find({ key: "usage-refresh" })).toBeDefined()
    expect(fake.authorizations).toBe(0)
  })
}

for (const event of ["native", "reset", "detach"] as const) {
  test(`${event} supersedes strip loading and an old completion cannot restore it`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "terminal",
      component: "AbovePrompt",
      props: props(),
    })
    const entered = deferred<void>()
    const reading = deferred<RateLimitReading[]>()
    fake.readUsage = () => {
      entered.resolve()
      return reading.promise
    }
    const refreshing = refresh($)
    await entered.promise
    expect(await band.find({ key: "usage-checking" })).toBeDefined()
    if (event === "native") await measure($, [{ kind: "five_hour", percentUsed: 94 }])
    if (event === "reset")
      await $.session.end({ reason: "clear", sessionId: "s", resume: {} } as never)
    if (event === "detach")
      await $.session.detach({
        surface: "terminal",
        clientId: "terminal:loading",
        reason: "detach",
      })
    expect(await band.find({ key: "usage-checking" })).toBeUndefined()
    expect(
      await band.find({ text: event === "native" ? "94%" : "Usage unavailable" }),
    ).toBeDefined()
    reading.resolve([])
    await refreshing
    expect(await band.find({ key: "usage-checking" })).toBeUndefined()
    expect(
      await band.find({ text: event === "native" ? "94%" : "Usage unavailable" }),
    ).toBeDefined()
  })
}

test("an older read completion cannot clear a newer reactive loading state", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  const firstEntered = deferred<void>()
  const secondEntered = deferred<void>()
  const firstReading = deferred<RateLimitReading[]>()
  const secondReading = deferred<RateLimitReading[]>()
  let reads = 0
  fake.readUsage = () => {
    reads++
    if (reads === 1) {
      firstEntered.resolve()
      return firstReading.promise
    }
    secondEntered.resolve()
    return secondReading.promise
  }
  const first = refresh($)
  await firstEntered.promise
  const second = refresh($)
  await secondEntered.promise
  firstReading.resolve([])
  await first
  expect(await band.find({ key: "usage-checking" })).toBeDefined()
  expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
  expect((await $.command.run(command(PLUGIN))).text).toBe("Checking usage…")
  secondReading.resolve([])
  await second
  expect(await band.find({ text: "Usage unavailable" })).toBeDefined()
  expect(await band.find({ key: "usage-refresh" })).toBeDefined()
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`${surface} Refresh shows one loading status without a duplicate CTA and shares concurrent presses`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(),
    })
    const entered = deferred<void>()
    const reading = deferred<RateLimitReading[]>()
    fake.readUsage = () => {
      entered.resolve()
      return reading.promise
    }
    const refreshLabel = surface === "terminal" ? "[↻]" : "↻"
    expect((await band.find({ key: "usage-refresh" }))?.props.label).toBe(refreshLabel)
    expect(await band.find({ text: refreshLabel })).toBeDefined()
    const first = band.press({ key: "usage-refresh" })
    const second = band.press({ key: "usage-refresh" })
    await entered.promise
    expect(await band.find({ text: "Checking…" })).toBeUndefined()
    expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
    expect(
      (await band.findAll({ type: "Button" })).some((node) =>
        String(node.props.label).includes("Checking"),
      ),
    ).toBe(false)
    expect(fake.usageReads).toBe(2)
    reading.resolve([])
    await Promise.all([first, second])
    expect(fake.usageReads).toBe(2)
    expect(await band.find({ text: "Checking…" })).toBeUndefined()
    expect(await band.find({ key: "usage-refresh" })).toBeDefined()
    expect(fake.authorizations).toBe(0)
    expect(fake.forbidden).toEqual([])
  })
}

test("account failure exposes an absolute retry time while Refresh still checks native data", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  const success = fake.response
  fake.response = { status: 500, ok: false, headers: {}, text: "" }
  await start($)
  await attachDesktop($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "desktop",
    component: "AbovePrompt",
    props: props(),
  })
  expect(
    (await band.findAll({ type: "Svg" })).some((node) =>
      String(node.props.alt).includes(retryCopy(NOW + 60_000)),
    ),
  ).toBe(true)
  expect((await $.command.run(command(PLUGIN))).text).toContain(retryCopy(NOW + 60_000))
  await band.press({ key: "usage-refresh" })
  expect(fake.usageReads).toBe(3)
  expect(fake.requests.length).toBe(1)
  expect(await band.find({ text: "Checking…" })).toBeUndefined()
  expect(await band.find({ key: "usage-refresh" })).toBeDefined()
  fake.now += 59_999
  await band.press({ key: "usage-refresh" })
  expect(fake.requests.length).toBe(1)
  fake.now++
  expect((await $.command.run(command(PLUGIN))).text).not.toContain("Account retry at")
  expect(fake.requests.length).toBe(1)
  fake.response = success
  await band.press({ key: "usage-refresh" })
  expect(fake.requests.length).toBe(2)
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 31%.")
  expect(await band.find({ text: retryCopy(NOW + 60_000) })).toBeUndefined()
  expect(await band.find({ text: "Checking…" })).toBeUndefined()
  expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
})

test("failure retry text rounds up fractional seconds instead of promising an early retry", async ($, on) => {
  const fake = fakeEngine(on)
  fake.now += 15_123
  fake.rateLimits = []
  fake.response = { status: 500, ok: false, headers: {}, text: "" }
  await start($)
  await attachDesktop($)
  const details = (await $.command.run(command(PLUGIN))).text
  expect(details).toContain(retryCopy(NOW + 75_123))
  expect(details).toContain(":16")
})

for (const retryAfter of ["1800", "Fri, 02 Jan 2026 00:30:00 GMT"]) {
  test(`retry text respects long HTTP429 deadline ${retryAfter}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.response = { status: 429, ok: false, headers: { "retry-after": retryAfter }, text: "" }
    await start($)
    await attachDesktop($)
    expect((await $.command.run(command(PLUGIN))).text).toContain(
      retryAfter === "1800"
        ? retryCopy(NOW + 1_800_000)
        : retryCopy(NOW + 86_400_000 + 1_800_000),
    )
    fake.now += 60_000
    await refresh($)
    expect(fake.requests.length).toBe(1)
  })
}

for (const columns of [24, 40]) {
  test(`account retry guidance stays bounded at ${columns} columns`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.response = { status: 500, ok: false, headers: {}, text: "" }
    await start($)
    await attachDesktop($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "terminal",
      component: "AbovePrompt",
      props: props(columns),
    })
    const hint = (await band.findAll({ type: "Text" })).find(
      (node) => node.text === `${retryCopy(NOW + 60_000)} · Refresh checks session.`,
    )
    expect((await band.find({ key: "usage-retry-copy" }))?.props.height).toBeUndefined()
    expect(hint?.props.wrap).toBe("wrap")
    expect(hint?.props.dimColor).toBe(true)
    expect(
      (await band.findAll({ type: "Box" })).some(
        (node) => node.props.width === columns && node.props.overflow === "hidden",
      ),
    ).toBe(true)
    expect(await band.find({ key: "usage-refresh" })).toBeDefined()
    expect(fake.requests.length).toBe(1)
  })
}

for (const event of ["native", "reset", "detach"] as const) {
  test(`${event} clears account failure retry guidance`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    fake.response = { status: 500, ok: false, headers: {}, text: "" }
    await start($)
    await attachDesktop($)
    expect((await $.command.run(command(PLUGIN))).text).toContain("Account retry at")
    if (event === "native") {
      fake.rateLimits = LIMITS
      await refresh($)
      expect(fake.requests.length).toBe(1)
    }
    if (event === "reset")
      await $.session.end({ reason: "clear", sessionId: "s", resume: {} } as never)
    if (event === "detach")
      await $.session.detach({
        surface: "desktop",
        clientId: "desktop:fallback",
        reason: "detach",
      })
    expect((await $.command.run(command(PLUGIN))).text).not.toContain("Account retry at")
  })
}

test("successful fresh cache is not presented as account failure backoff", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  await attachDesktop($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  for (let index = 0; index < 2; index++) {
    const details = (await refresh($)).text
    expect(details).toContain("5-hour: 31%.")
    expect(details).not.toContain("Account retry at")
  }
  expect(await band.find({ text: "31%" })).toBeDefined()
  expect(await band.find({ text: "Checking…" })).toBeUndefined()
  expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
  expect(fake.requests.length).toBe(1)
})

test("numeric readings stay visible throughout a deferred refresh", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  const entered = deferred<void>()
  const reading = deferred<RateLimitReading[]>()
  fake.readUsage = () => {
    entered.resolve()
    return reading.promise
  }
  const refreshing = refresh($)
  await entered.promise
  expect(await band.find({ text: "8%" })).toBeDefined()
  expect(await band.find({ text: "Checking…" })).toBeUndefined()
  expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
  reading.resolve(LIMITS)
  await refreshing
})

test("concurrent UI Refresh presses share one eligible account request and restore the CTA after failure", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  fake.response = { status: 500, ok: false, headers: {}, text: "" }
  await start($)
  await attachDesktop($)
  fake.now += 60_000
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "desktop",
    component: "AbovePrompt",
    props: props(),
  })
  const entered = deferred<void>()
  const response = deferred<HttpResponse>()
  fake.fetch = () => {
    entered.resolve()
    return response.promise
  }
  expect((await band.find({ key: "usage-refresh" }))?.props.label).toBe("↻")
  expect(await band.find({ text: "↻" })).toBeDefined()
  const first = band.press({ key: "usage-refresh" })
  const second = band.press({ key: "usage-refresh" })
  await entered.promise
  expect(await band.find({ text: "Checking…" })).toBeUndefined()
  expect(await band.find({ key: "usage-refresh" })).toBeUndefined()
  expect(fake.usageReads).toBe(3)
  expect(fake.requests.length).toBe(2)
  response.resolve(fake.response)
  await Promise.all([first, second])
  expect(fake.usageReads).toBe(3)
  expect(fake.requests.length).toBe(2)
  expect(await band.find({ key: "usage-refresh" })).toBeDefined()
  expect(await band.find({ text: "Checking…" })).toBeUndefined()
  expect(
    (await band.findAll({ type: "Svg" })).some((node) =>
      String(node.props.alt).includes(retryCopy(NOW + 180_000)),
    ),
  ).toBe(true)
})

test("terminal checking dots advance locally once per frame timer and disappear when the read settles", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = []
  await start($)
  const entered = deferred<void>()
  const reading = deferred<RateLimitReading[]>()
  fake.readUsage = () => {
    entered.resolve()
    return reading.promise
  }
  const refreshing = refresh($)
  await entered.promise
  let band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  const phases = async () =>
    (await band.findAll({ type: "Text", in: "usage-checking" }))
      .filter((node) => node.text === ".")
      .map((node) => node.props.dimColor)
  expect(await phases()).toEqual([false, true, true])
  expect(await band.find({ text: "Checking usage...", in: "usage-checking" })).toBeDefined()
  await band.advance(300)
  expect(await phases()).toEqual([true, false, true])
  await band.redraw()
  await band.redraw()
  await band.advance(300)
  expect(await phases()).toEqual([true, true, false])
  await band.advance(300)
  expect(await phases()).toEqual([false, true, true])
  expect(fake.usageReads).toBe(2)
  expect(fake.requests.length).toBe(0)
  expect(fake.authorizations).toBe(0)
  await band.unmount()
  band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  expect(await phases()).toEqual([false, true, true])
  reading.resolve([])
  await refreshing
  expect(await band.find({ key: "usage-checking" })).toBeUndefined()
  await band.advance(900)
  expect(await band.find({ key: "usage-checking" })).toBeUndefined()
  expect(await band.find({ text: "Usage unavailable" })).toBeDefined()
  expect(fake.usageReads).toBe(2)
  expect(fake.requests.length).toBe(0)
  expect(fake.forbidden).toEqual([])
})

test("usage spacing separates other plugins without consuming a constrained band", async ($, on) => {
  fakeEngine(on)
  await start($)
  for (const surface of ["desktop", "terminal"] as const) {
    for (const rows of [2, 3, 4, 7, 8]) {
      const band = await $.ui.mount({
        plugin: PLUGIN,
        surface,
        component: "AbovePrompt",
        props: props(80, rows),
      })
      expect((await band.find({ key: "usage-spacing" }))?.props.marginTop).toBe(
        rows < 8 ? 0 : surface === "desktop" ? 0.25 : 1,
      )
      expect(await band.find({ key: "usage-band" })).toBeDefined()
    }
  }
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`narrow ${surface} label matches its visible percentage format`, async ($, on) => {
    fakeEngine(on)
    await start($)
    await measure($, [{ kind: "five_hour", percentUsed: 99.5 }])
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface,
      component: "AbovePrompt",
      props: props(23),
    })
    const shown = surface === "desktop" ? "99%" : "99.5%"
    expect(await band.find({ text: shown })).toBeDefined()
    const control = await band.find({ key: "usage-detail-five_hour" })
    expect(String(control?.props.label)).toContain(`5-hour: ${shown}.`)
    expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 99.5%.")
  })
}

test(
  "the unified entry provides help and usage through one namespace",
  {
    plugins: [
      {
        name: "metadata-consumer",
        register(on) {
          on("command.run", async ($, e, next) => {
            if (e.command !== "metadata-consumer") return next(e)
            return { text: JSON.stringify(await $.antiburn.describe()) }
          })
        },
      },
    ],
  },
  async ($, on) => {
    const fake = fakeEngine(on)
    await start($)
    const description = JSON.parse(
      (await $.command.run(command("metadata-consumer", ""))).text!,
    )
    expect(description?.id).toBe("antiburn")
    expect(description?.capabilities).toMatchObject({ usage: true, usageUiEnabled: true })
    expect((await $.command.run(command(PLUGIN, ""))).text).toContain("5-hour: 8%.")
    for (const args of ["help"]) {
      const result = (await $.command.run(command(PLUGIN, args))).text
      expect(result).toContain("antiburn")
      expect(result).not.toContain("CLI")
      expect(result).not.toContain("App connection")
    }
    expect((await $.command.run(command(PLUGIN, "usage details"))).text).toContain(
      "5-hour: 8%.",
    )
    expect((await $.command.run(command("antiburn-mod", ""))).text).toBe("host:antiburn-mod")
    const reads = fake.usageReads
    await $.command.run(command(PLUGIN, "usage refresh"))
    expect(fake.usageReads).toBe(reads + 1)
    expect(fake.envReads).toBe(0)
    expect(fake.forbidden).toEqual([])
  },
)

test(
  "retired usage option cannot hide the automatic display",
  { options: { usageUiEnabled: false } },
  async ($, on) => {
    const fake = fakeEngine(on)
    await start($)
    expect(fake.usageReads).toBe(1)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "terminal",
      component: "AbovePrompt",
      props: props(),
    })
    expect(await band.find({ key: "usage-band" })).toBeDefined()
    expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 8%.")
    expect(fake.forbidden).toEqual([])
  },
)

for (const source of ["user", "plugin"] as const) {
  test(`canonical registration proceeds with owned ${source} metadata without inventory probing`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.commandList = [{ name: PLUGIN, source, description: "Old antiburn findings" }]
    fake.listFailure = true
    await start($)
    expect(fake.listReads).toBe(0)
    expect(fake.registered).toEqual([PLUGIN])
    expect((await $.command.run(command(PLUGIN, "help"))).text).toContain(
      "Run `/antiburn` to see your usage.",
    )
    for (const name of ["antiburn-mod", "antiburn-plugin", "other-plugin"])
      expect((await $.command.run(command(name, "help"))).text).toBe(`host:${name}`)
  })
}

test("canonical registration failure logs once and leaves quota independent", async ($, on) => {
  const fake = fakeEngine(on)
  fake.registrationFailure = [PLUGIN]
  await start($)
  expect(fake.registered).toEqual([])
  expect(fake.listReads).toBe(0)
  expect(fake.logs).toEqual(["antiburn: /antiburn could not be registered."])
  expect(fake.usageReads).toBe(1)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "terminal",
    component: "AbovePrompt",
    props: props(),
  })
  expect(await band.find({ key: "usage-band" })).toBeDefined()
  expect((await $.command.run(command(PLUGIN, "help"))).text).toBe("host:antiburn")
})

test("quota initialization failure cannot prevent help registration", async ($, on) => {
  const fake = fakeEngine(on)
  fake.clockFailure = true
  await start($)
  expect((await $.command.run(command(PLUGIN, "help"))).text).toContain(
    "Run `/antiburn` to see your usage.",
  )
  fake.clockFailure = false
  expect((await $.command.run(command(PLUGIN, "usage refresh"))).text).toBe("Usage updated.")
})

test(
  "invalid commands and release-only options cannot activate development capabilities",
  { options: { usageUiEnabled: true } },
  async ($, on) => {
    const fake = fakeEngine(on)
    await start($)
    const reads = fake.usageReads
    for (const args of [
      "checks",
      "debug",
      "cli",
      "status --cli",
      "usage debug",
      "usage refresh extra",
      "; process.run",
      "x".repeat(513),
    ]) {
      expect((await $.command.run(command(PLUGIN, args))).text).toContain("Unknown arguments.")
    }
    expect(fake.usageReads).toBe(reads)
    expect(fake.envReads).toBe(0)
    expect(fake.forbidden).toEqual([])
  },
)

test("a restarted session registers only the canonical command again", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  await start($)
  expect(fake.registered).toEqual([PLUGIN, PLUGIN])
  expect(fake.listReads).toBe(0)
  expect((await $.command.run(command(PLUGIN, "help"))).text).toContain(
    "Run `/antiburn` to see your usage.",
  )
})

test("session teardown invalidates a pending canonical registration", async ($, on) => {
  const fake = fakeEngine(on)
  const entered = deferred<void>()
  const finish = deferred<string>()
  fake.registerCommand = () => {
    entered.resolve()
    return finish.promise
  }
  const starting = start($)
  await entered.promise
  await $.session.end({ reason: "exit", sessionId: "s" } as never)
  finish.resolve(PLUGIN)
  await starting
  expect((await $.command.run(command(PLUGIN, "help"))).text).toBe("host:antiburn")
})

for (const button of ["usage-refresh", "usage-brand-open"]) {
  test(`detaching invalidates a queued ${button} action`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    const entered = deferred<void>()
    const release = deferred<void>()
    let launches = 0
    on("process.run", () => {
      launches++
      return {
        value: {
          exitCode: 0,
          stdout: "",
          stderr: "",
          isStdoutTruncated: false,
          isStderrTruncated: false,
        },
      }
    })
    on("ui.press", { element: button }, async (_$, event, next) => {
      entered.resolve()
      await release.promise
      return next(event)
    })
    await start($)
    const band = await $.ui.mount({
      plugin: PLUGIN,
      surface: "terminal",
      component: "AbovePrompt",
      props: props(),
    })
    const reads = fake.usageReads
    const pressing = band.press({ key: button })
    await entered.promise
    await $.session.detach({ clientId: "terminal-test", surface: "terminal", reason: "detach" })
    release.resolve()
    await pressing
    expect(fake.usageReads).toBe(reads)
    expect(launches).toBe(0)
    expect(fake.forbidden).toEqual([])
  })
}

test("public replies omit repeated branding and refresh confirms without repeating usage", async ($, on) => {
  const fake = fakeEngine(on)
  await start($)
  const band = await $.ui.mount({
    plugin: PLUGIN,
    surface: "desktop",
    component: "AbovePrompt",
    props: props(),
  })
  const help = (await $.command.run(command(PLUGIN, "help"))).text
  expect(help).toBe("Run `/antiburn` to see your usage.\nRun `/antiburn refresh` to update it.")
  expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 8%.")
  const reads = fake.usageReads
  expect((await $.command.run(command(PLUGIN, "refresh"))).text).toBe("Usage updated.")
  expect(fake.usageReads).toBe(reads + 1)
  expect(await band.find({ key: "usage-band" })).toBeDefined()
  for (const args of ["status", "diagnostics", "help --ascii", "help --flame"])
    expect((await $.command.run(command(PLUGIN, args))).text).toContain("Unknown arguments.")
  expect(fake.forbidden).toEqual([])
})

test(
  "refresh ignores the retired display option",
  { options: { usageUiEnabled: false } },
  async ($, on) => {
    fakeEngine(on)
    await start($)
    expect((await $.command.run(command(PLUGIN, "refresh"))).text).toBe("Usage updated.")
  },
)

for (const [name, text, shown] of [
  [
    PLUGIN,
    "antiburn: Run `/antiburn` to see your usage.",
    "Run `/antiburn` to see your usage.",
  ],
  [PLUGIN, "Usage updated.", "Usage updated."],
  ["other-plugin", "antiburn: other output", "antiburn: other output"],
]) {
  test(`command output removes only its own leading label: ${name} ${text}`, async ($, on) => {
    fakeEngine(on)
    await start($)
    const row = await $.ui.mount({
      plugin: PLUGIN,
      surface: "desktop",
      component: "CommandOutput",
      props: { command: name!, args: "help", text: text!, isErrored: false },
    })
    expect(await row.find({ text: shown! })).toBeDefined()
  })
}

for (const lostAccess of ["unsupported", "missing", "error"] as const) {
  test(`refresh never confirms retained account data after ${lostAccess} access`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.rateLimits = []
    await start($)
    await attachDesktop($)
    expect((await $.command.run(command(PLUGIN))).text).toContain("5-hour: 31%.")
    fake.now += 61_000
    if (lostAccess === "unsupported")
      fake.authorization = { kind: "api-key", handle: "test-api-handle" }
    if (lostAccess === "missing") fake.authorization = null
    if (lostAccess === "error") fake.authorizationFailure = true
    const reply = (await $.command.run(command(PLUGIN, "refresh"))).text
    expect(reply).not.toContain("Usage updated.")
    expect(reply).toContain("Usage unavailable · Last known.")
    expect(reply).toContain("5-hour: 31%.")
    expect(fake.requests).toHaveLength(1)
  })
}

for (const tier of ["prepend", "append"] as const) {
  for (const surface of ["terminal", "desktop"] as const) {
    for (const scenario of [
      "ready",
      "constrained",
      "spacious",
      "survey",
      "hidden",
      "failure",
    ] as const) {
      test(
        `unrelated ${tier} mod survives ${surface} quota ${scenario}`,
        {
          plugins: [
            {
              name: "unrelated-band",
              tier,
              register(on) {
                on("ui.render", { component: "AbovePrompt" }, async ($, e, next) => {
                  await $.ui.log("unrelated render")
                  const downstream = await next(e)
                  const { Box, Text } = $.ui.resolve(e)
                  return h(
                    Box,
                    { key: "unrelated-composition", flexDirection: "column" },
                    h(Text, { key: "unrelated-content" }, "Unrelated mod"),
                    downstream,
                  ) as RenderElement
                })
              },
            },
          ],
        },
        async ($, on) => {
          const fake = fakeEngine(on)
          await start($)
          fake.clockFailure = scenario === "failure"
          const rows =
            scenario === "hidden"
              ? 0
              : scenario === "constrained"
                ? 1
                : scenario === "spacious"
                  ? 8
                  : 4
          const band = await $.ui.mount({
            plugin: PLUGIN,
            surface,
            component: "AbovePrompt",
            props: props(80, rows, scenario === "survey"),
          })
          expect(await band.find({ text: "Unrelated mod" })).toBeDefined()
          expect(await band.find({ text: "host drawing" })).toBeDefined()
          expect(fake.renders).toBe(1)
          expect(fake.logs.filter((line) => line === "unrelated render").length).toBe(1)
          const visible = !["survey", "hidden", "failure"].includes(scenario)
          expect((await band.find({ key: "usage-band" })) !== undefined).toBe(visible)
          if (visible) {
            const boxes = await band.findAll({ type: "Box" })
            const own = boxes.findIndex((node) => node.key === "usage-spacing")
            const other = boxes.findIndex((node) => node.key === "unrelated-composition")
            expect(own >= 0 && other >= 0).toBe(true)
            expect(own < other).toBe(tier === "append")
            expect((await band.find({ key: "usage-spacing" }))?.props.marginTop).toBe(
              rows < 8 ? 0 : surface === "desktop" ? 0.25 : 1,
            )
          }
          await band.unmount()
        },
      )
    }
  }
}

for (const surface of ["terminal", "desktop"] as const) {
  test(
    `empty downstream UI retains ${surface} quota`,
    {
      plugins: [
        {
          name: "empty-band",
          tier: "append",
          register(on) {
            on("ui.render", { component: "AbovePrompt" }, async ($, e) => {
              await $.ui.log("empty render")
              const { Box } = $.ui.resolve(e)
              return h(Box, { key: "empty-downstream" }) as RenderElement
            })
          },
        },
      ],
    },
    async ($, on) => {
      const fake = fakeEngine(on)
      await start($)
      const band = await $.ui.mount({
        plugin: PLUGIN,
        surface,
        component: "AbovePrompt",
        props: props(),
      })
      expect(await band.find({ key: "usage-band" })).toBeDefined()
      expect(fake.renders).toBe(0)
      expect(fake.logs.filter((line) => line === "empty render").length).toBe(1)
    },
  )

  test(
    `failed downstream mod forwards once on ${surface}`,
    {
      plugins: [
        {
          name: "failed-band",
          tier: "append",
          register(on) {
            on("ui.render", { component: "AbovePrompt" }, async ($) => {
              await $.ui.log("failed render")
              throw new Error("unrelated render failed")
            })
          },
        },
      ],
    },
    async ($, on) => {
      const fake = fakeEngine(on)
      await start($)
      const band = await $.ui.mount({
        plugin: PLUGIN,
        surface,
        component: "AbovePrompt",
        props: props(),
      })
      expect(await band.find({ text: "host drawing" })).toBeDefined()
      expect(await band.find({ key: "usage-band" })).toBeDefined()
      expect(fake.renders).toBe(1)
      expect(fake.logs.filter((line) => line === "failed render").length).toBe(1)
    },
  )
}
