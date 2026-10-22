import { expect, test } from "claude-code/testing"
import type { Engine } from "claude-code/testing"
import type { On, RenderElement, RenderSurface, TurnStepChunk } from "claude-code"
import { commandSummary, promptPreview } from "./register"
import { BURN_ANIMATION_ENABLED } from "./burn"
import type { RateLimitReading } from "./usage"

const SMALL = {
  input_tokens: 10,
  output_tokens: 5,
  cache_read_input_tokens: 20_000,
  cache_creation_input_tokens: 0,
}
const DEEP = { ...SMALL, cache_read_input_tokens: 450_000 }

type Fake = {
  streamChunks?: TurnStepChunk[]
  streamFailure?: boolean
  streamStarts: number
  liveMode: string | undefined
  rateLimits: RateLimitReading[]
  usageReads: number
  commands: { name: string; argumentHint?: string }[]
  now: number
  pulseTimers: { ms: number; fire: () => void }[]
  flameTimers: { ms: number; fire: () => void }[]
  presentationTimers: { ms: number; fire: () => void }[]
  readUsage?: () => Promise<RateLimitReading[]>
  clockRead?: () => Promise<number>
  stateWrites: string[]
  submits: string[]
  fills: string[]
  compacts: (string | undefined)[]
  opens: string[]
  focusedOpens: boolean[]
  panes: string[]
  statuses: (string | undefined)[]
  logs: string[]
  writes: string[]
  model: string
  usage: typeof SMALL
}

function fakeEngine(on: On, surfaces: readonly RenderSurface[] = ["terminal"]): Fake {
  const fake: Fake = {
    streamStarts: 0,
    liveMode: "checks",
    rateLimits: [],
    usageReads: 0,
    commands: [],
    now: 1_800_000_000_000,
    pulseTimers: [],
    flameTimers: [],
    presentationTimers: [],
    stateWrites: [],
    submits: [],
    fills: [],
    compacts: [],
    opens: [],
    focusedOpens: [],
    panes: [],
    statuses: [],
    logs: [],
    writes: [],
    model: "claude-opus-5",
    usage: SMALL,
  }
  on("env.get", (_$, e) => ({
    value: e.name === "ANTIBURN_LIVE_MODE" ? fake.liveMode : undefined,
  }))
  on("session.usage", async () => {
    fake.usageReads += 1
    return {
      value: {
        startedAt: 0,
        context: { window: 200000 },
        rateLimits: fake.readUsage === undefined ? fake.rateLimits : await fake.readUsage(),
      },
    }
  })
  on("state.set", async (_$, e, next) => {
    fake.stateWrites.push(e.key)
    return next(e)
  })
  on("clock.now", async () => ({
    value: fake.clockRead ? await fake.clockRead() : fake.now,
  }))
  on("session.id", () => ({ value: "test-session" }))
  on(
    "clock.after",
    (_$, e) =>
      new Promise<{ value: undefined }>((resolve) => {
        ;(e.ms === 800
          ? fake.pulseTimers
          : e.ms === 100 || e.ms === 15000
            ? fake.flameTimers
            : fake.presentationTimers
        ).push({
          ms: e.ms,
          fire: () => resolve({ value: undefined }),
        })
      }),
  )
  on("classic.SessionStart", () => ({}) as never)
  on("session.measure", (_$, e) => ({ changed: e.changed }))
  on("session.end", (_$, e) => ({ sessionId: e.sessionId }))
  on("ui.focus", () => ({}))
  on("session.start", () => ({ cwd: "/work" }))
  on("classic.UserPromptSubmit", () => ({}) as never)
  on("classic.Stop", () => ({}) as never)
  on("session.surfaces", () => ({ value: surfaces }))
  on("command.register", (_$, e) => {
    fake.commands.push(e)
    return { value: { command: e.name } }
  })
  on("ui.log", (_$, e) => {
    fake.logs.push(e.text)
    return { value: undefined }
  })
  on("ui.status", (_$, e) => {
    fake.statuses.push(e.text)
    return { value: undefined }
  })
  on("ui.open", (_$, e) => {
    fake.opens.push(e.id)
    fake.focusedOpens.push(e.focus === true)
    if (!fake.panes.includes(e.id)) fake.panes.push(e.id)
    return { value: { isPlaced: true } }
  })
  on("ui.panes", () => ({
    value: fake.panes.map((id) => ({
      id,
      title: "antiburn",
      isShown: true,
      isFocused: false,
      isPlaced: true,
    })),
  }))
  on("ui.close", (_$, e) => {
    fake.panes = fake.panes.filter((id) => id !== e.id)
    return { value: undefined }
  })
  on("fs.write", (_$, e) => {
    fake.writes.push(e.path)
    return { value: undefined }
  })
  on("prompt.submit", (_$, e) => {
    fake.submits.push(e.text)
    return { text: e.text }
  })
  on("prompt.fill", (_$, e) => {
    fake.fills.push(e.text)
    return { isFilled: true }
  })
  on("session.compact", (_$, e) => {
    fake.compacts.push(e.instructions)
    return {
      messages: [{ role: "user" as const, text: "Summary", toolUses: [] }],
    }
  })
  on("tool.call", () => ({ isError: true, result: "exit 1" }))
  on("turn.complete", () => ({ text: "" }))
  // Stands for the engine's own drawing when a mod hook yields with next(e).
  on("ui.render", (t$, e) => {
    const { Box } = t$.ui.resolve(e)
    return h(Box, {}) as RenderElement
  })
  on("turn.step", async function* (_$, e) {
    fake.streamStarts += 1
    for (const chunk of fake.streamChunks ?? []) yield chunk
    if (fake.streamFailure) throw new Error("transport failed")
    return {
      turnId: e.turnId,
      index: e.index,
      answer: "",
      toolUses: [],
      stopReason: "end_turn" as const,
      usage: { ...fake.usage, model: e.model },
    }
  })
  return fake
}

async function startSession($: Engine, mode = "default") {
  await $.session.start({
    cwd: "/work",
    surface: "terminal",
    isInteractive: true,
  })
  await $.classic.UserPromptSubmit({
    ...HOOK_BASE,
    hook_event_name: "UserPromptSubmit",
    prompt: "hi",
    permission_mode: mode,
  } as never)
}

const HOOK_BASE = { session_id: "s", transcript_path: "/t", cwd: "/work" }

async function turn($: Engine, fake: Fake, options: { failures?: number } = {}) {
  // The kit runs a streaming event only while the test reads its generator.
  const stream = $.turn.step({
    turnId: "t",
    index: 0,
    model: fake.model,
    effort: "high",
    messageCount: 1,
  })
  let chunk = await stream.next()
  while (chunk.done !== true) chunk = await stream.next()
  for (let i = 0; i < (options.failures ?? 0); i += 1) {
    // The model labels each retry differently, as in a live session.
    await $.tool.call({
      tool: "Bash",
      command: "npm test",
      description: `Run the tests (try ${i + 1})`,
    } as never)
  }
  await $.turn.complete({
    answer: "",
    durationMs: 1,
    isAborted: false,
    turnId: "t",
    reason: "answer",
  })
}

async function showCheckSamples($: Engine, variant: "demo" | "demo states" = "demo states") {
  await $.command.run({
    command: "antiburn-dev",
    args: variant,
    presentation: { isFullscreen: false, columns: 150 },
  } as never)
}

function stripProps(bodyColumns = 120, isWorking = false) {
  return {
    hasSurvey: false,
    isWorking,
    maxRows: 4,
    bodyColumns,
    scroll: { offset: 0, bodyRows: 4 },
    view: {},
  }
}

const PANE_PROPS = {
  title: "antiburn",
  isFocused: true,
  bodyColumns: 48,
  placement: "inline" as const,
  scroll: { offset: 0, bodyRows: 24 },
  view: {},
}

for (const surface of ["terminal", "desktop"] as const) {
  test(`real session events do not run the mod's former live detectors on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await turn($, fake, { failures: 3 })

    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect((await band.find({ text: /to fix/ })) === undefined).toBe(true)
    expect(await band.find({ key: "fix" })).toBeUndefined()
    const chip = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "SessionMode",
      props: { modes: [] },
    })
    expect((await chip.find({ key: "chip" }))?.props.label).toBe("antiburn")
    expect(fake.submits).toEqual([])
    expect(fake.compacts).toEqual([])
    expect(fake.writes).toEqual([])
  })

  test(`the chip opens an empty checks pane on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await turn($, fake, { failures: 3 })
    const chip = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "SessionMode",
      props: { modes: [] },
    })
    await chip.press({ key: "chip" })
    expect(fake.opens).toEqual(["checks"])
    expect(fake.focusedOpens).toEqual([true])
    const pane = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props: PANE_PROPS,
    })
    expect(await pane.find({ text: /^No checks yet$/ })).toBeDefined()
  })

  test(`the pane lists a check and opens its detail on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await showCheckSamples($)
    const view = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props: PANE_PROPS,
    })
    const isTerminal = surface === "terminal"
    // The desktop puts the count in a badge and names the group, as the app does.
    expect(
      (await view.find({ text: isTerminal ? /^● Checks 5$/ : /^Checks$/ })) !== undefined,
    ).toBe(true)
    // Both surfaces give the first sentence of the check as its reason.
    expect(
      (await view.find({
        text: /^Claude read many files in one turn and changed none\./,
      })) !== undefined,
    ).toBe(true)
    // The footer gives no key hint: the chevron shows how a card opens.
    expect(await view.find({ text: /→ more/ })).toBe(undefined)
    // Both surfaces name the app in the link text.
    expect((await view.find({ text: /^antiburn app( ↗)?$/ })) !== undefined).toBe(true)
    if (isTerminal) expect((await view.find({ key: "row-E1" }))?.props.label).toBe("▸")
    await view.press({ key: "row-E1" })
    expect((await view.find({ text: /Pause the search/ })) !== undefined).toBe(true)
    // Both surfaces open the card in place: the list stays, and no back row shows.
    expect(await view.find({ key: "back" })).toBe(undefined)
    if (isTerminal) expect((await view.find({ key: "row-E1" }))?.props.label).toBe("▾")
    await view.press({ key: "hide-E1" })
    expect(await view.find({ key: "row-E1" })).toBeUndefined()
  })
}

for (const mode of ["auto", "bypassPermissions"]) {
  test(`${mode} mode needs a second press before the fix is sent`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($, mode)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect(fake.submits).toEqual([])
    expect(await band.find({ key: "fix" })).toBeUndefined()
    await $.tool.call({ tool: "Bash", command: "npm test" } as never)
    await turn($, fake, { failures: 3 })
    expect(await band.find({ key: "fix" })).toBeUndefined()
    expect(fake.submits).toEqual([])
  })
}

test("an armed desktop card opens on its prompt, takes the primary style, and a watched metric goes neutral", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, "auto")
  await showCheckSamples($)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect(await view.find({ key: "card-fix-E1" })).toBeDefined()
  expect((await view.find({ text: /^Pause the search/ })) === undefined).toBe(true)
  await view.press({ key: "card-fix-E1" })
  expect(fake.submits).toEqual([])
  const armed = await view.find({ key: "card-fix-E1" })
  expect(armed?.props.label).toBe("Confirm send")
  expect(armed?.props.variant).toBe("primary")
  expect((await view.find({ text: /Pause the search/ })) !== undefined).toBe(true)
  await view.press({ key: "card-fix-E1" })
  expect(fake.submits.length).toBe(1)
  // The watched card keeps its metric as neutral text, without the failure Svg.
  expect(await view.find({ text: /24 reads/ })).toBeDefined()
})

// The Svg of a chevron: the first child of the chevron Box. The press target lies on top of it.
async function chevronSvg(
  drawing: { find: (query: { key: string }) => Promise<unknown> },
  key: string,
) {
  const box = (await drawing.find({ key: `chevron-${key}` })) as
    { children?: { props: Record<string, unknown> }[] } | undefined
  return box?.children?.[0]?.props
}

test("a switch to auto mode at turn end needs a second press", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await $.classic.Stop({
    ...HOOK_BASE,
    hook_event_name: "Stop",
    stop_hook_active: false,
    permission_mode: "auto",
  } as never)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  await $.tool.call({ tool: "Bash", command: "npm test" } as never)
  expect(fake.submits).toEqual([])
  expect(await band.find({ key: "fix" })).toBeUndefined()
})

test("Context heavy compacts on press and the compaction resolves it", async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await showCheckSamples($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect((await band.find({ key: "fix" }))?.props.label).toBe("1 Compact now")
  await band.press({ key: "fix" })
  expect(fake.compacts.length).toBe(1)
  expect(fake.compacts[0]).toMatch(/^Keep the current task/)
  expect(await band.drawn()).not.toBeNull()
})

test("an armed compact card opens on its instructions and takes the primary style", async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await showCheckSamples($)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await view.find({ text: /^Keep the current task/ })) === undefined).toBe(true)
  await view.press({ key: "card-fix-D" })
  expect(fake.compacts).toEqual([])
  expect((await view.find({ key: "card-fix-D" }))?.props.variant).toBe("primary")
  expect((await view.find({ text: /Keep the current task/ })) !== undefined).toBe(true)
})

test("Obsolete model fills the /model command for the user to send", async ($, on) => {
  const fake = fakeEngine(on)
  fake.model = "claude-opus-4-8"
  await startSession($)
  await turn($, fake)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await band.find({ key: "fix" })).toBeUndefined()
  expect(fake.fills).toEqual([])
  expect(fake.submits).toEqual([])
})

test("the strip picks a shorter form at narrow widths", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const wide = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  expect(
    (await wide.find({
      text: /^\s*\d+ to fix {2}Context heavy · 182k tokens per request$/,
    })) !== undefined,
  ).toBe(true)
  const medium = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(90),
  })
  expect((await medium.find({ text: /182k tokens per request/ })) === undefined).toBe(true)
  // The fix button acts on the top check, so the shorter forms keep its name.
  expect(await medium.findAll({ type: "Raster" })).toHaveLength(1)
  expect((await medium.find({ text: / \d+ to fix {2}Context heavy/ })) !== undefined).toBe(true)
  const narrow = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(70),
  })
  expect(await narrow.findAll({ type: "Raster" })).toHaveLength(1)
  expect((await narrow.find({ text: /Context heavy/ })) !== undefined).toBe(true)
  expect((await narrow.find({ key: "view" }))?.props.label).toBe("2 View")
  // A narrow desktop strip shows the mark and the check name, without the metric.
  const narrowDesktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(54),
  })
  // The close glyph is an Svg too, because the desktop draws no native close control for the strip.
  expect((await narrowDesktop.findAll({ type: "Svg" })).map((node) => node.props.alt)).toEqual([
    "5 to fix",
    "Dismiss",
    "Checks and usage separator",
    "antiburn",
    "Usage not available yet. Try send a message first",
  ])
  expect((await narrowDesktop.find({ text: /Context heavy/ })) !== undefined).toBe(true)
  expect((await narrowDesktop.find({ text: /182k tokens/ })) === undefined).toBe(true)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  expect((await desktop.find({ text: /Context heavy · 182k tokens per request/ })) !== undefined).toBe(true)
  const head = (await desktop.find({ key: "brand-head" })) as
    { children?: { type?: string }[] } | undefined
  // The wordmark and the count pill are SVG, so they follow the theme. The pill carries the state, so no dot.
  expect((head?.children ?? []).filter((child) => child.type === "Svg").length).toBe(1)
  expect((await desktop.find({ text: /●/ })) === undefined).toBe(true)
  // The terminal band boxes the count on a brand tint in the failure text of the check language and draws the name in the brand.
  const count = (await wide.find({ text: /^ 5 to fix $/ })) as
    { props?: { color?: string; backgroundColor?: string } } | undefined
  const mark = (await wide.findAll({ type: "Box" })).find(
    (node) => node.props.width === 11 && node.props.height === 2,
  )
  expect(count?.props?.backgroundColor).toBe("#4D2F23")
  expect(count?.props?.color).toBe("#FF8D5C")
  // The desktop pill is a tint like the terminal box. It reads the theme and changes the fill and the ink, in monospace.
  const pill = (await desktop.findAll({ type: "Svg" })).find(
    (node) => node.props.alt === "5 to fix",
  )
  const pillSource = String(pill?.props.source)
  expect(pillSource).toMatch(/\.r\{fill:#F2DCCD\}\.i\{fill:#9A3412\}/)
  expect(pillSource).toMatch(
    /prefers-color-scheme:dark\)\{\.r\{fill:#51372A\}\.i\{fill:#FF8D5C\}/,
  )
  expect(pillSource).toMatch(/monospace/)
  const wordmark = (await desktop.findAll({ type: "Svg" })).find(
    (node) => node.props.key === "brand" || node.props.alt === "antiburn",
  )
  expect(String(wordmark?.props.source)).not.toMatch(/prefers-color-scheme/)
  expect(mark?.props.width).toBe(11)
})

test("the strip drops the metric below 100 columns and stacks the buttons below 56", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  for (const surface of ["terminal", "desktop"] as const) {
    const medium = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(90),
    })
  expect((await medium.find({ text: /182k tokens/ })) === undefined).toBe(true)
    expect(await medium.find({ key: "actions" })).toBe(undefined)
    // Below 56 columns the buttons take a second row, and the first row keeps the check name.
    const stacked = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(44),
    })
    expect((await stacked.find({ key: "actions" })) !== undefined).toBe(true)
    expect((await stacked.find({ text: /Context heavy/ })) !== undefined).toBe(true)
    expect((await stacked.find({ key: "fix" })) !== undefined).toBe(true)
  }
  // The terminal name and count still fit on the first row at 44 columns.
  const stacked = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(44),
  })
  expect((await stacked.find({ text: /^\s*5 to fix {2}Context heavy$/ })) !== undefined).toBe(true)
})

test("the strip yields while a turn runs", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120, true),
  })
  expect(await band.find({ key: "fix" })).toBe(undefined)
})

test("dismiss hides the strip, and a new kind brings it back with a new count", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  // The dismiss control is a dim plain ×, as the host's own close control looks.
  expect((await band.find({ key: "dismiss" }))?.props.plain).toBe(true)
  expect((await band.find({ text: "×" })) !== undefined).toBe(true)
  await band.press({ key: "dismiss" })
  expect(await band.find({ key: "fix" })).toBe(undefined)
  expect(await band.find({ key: "view" })).toBe(undefined)
  expect(await band.find({ text: /to fix/ })).toBe(undefined)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await desktop.find({ key: "fix" })).toBeUndefined()
  expect(
    (await desktop.findAll({ type: "Svg" })).some(
      (node) => node.props.alt === "Checks and usage separator",
    ),
  ).toBe(false)
  expect(
    (await desktop.findAll({ type: "Svg" })).some((node) => node.props.alt === "antiburn"),
  ).toBe(true)
  expect(
    (await desktop.findAll({ type: "Svg" })).some(
      (node) => node.props.alt === "Usage not available yet. Try send a message first",
    ),
  ).toBe(true)
  // The same check in a later turn does not bring the strip back.
  await turn($, fake, { failures: 3 })
  expect(await band.find({ key: "fix" })).toBe(undefined)
})

test("/antiburn-dev opens checks and the explicit checks argument remains supported", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  const opened = await $.command.run({
    command: "antiburn-dev",
    args: "",
    presentation,
  } as never)
  expect(opened.text).toBe("Opened the antiburn checks pane.")
  await $.command.run({
    command: "antiburn-dev",
    args: "checks",
    presentation,
  } as never)
  expect(fake.opens).toEqual(["checks", "checks"])
})

test("/antiburn-dev demo shows all five checks and keeps them until demo off", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo",
    presentation,
  } as never)
  const pane = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  for (const label of ["Retry loop", "Reading spiral", "Context heavy", "Overthinking"]) {
    expect((await pane.find({ text: new RegExp(label) })) !== undefined).toBe(true)
  }
  // The resolved check is in the folded Resolved group.
  await pane.press({ key: "group-resolved" })
  expect((await pane.find({ text: /Obsolete model/ })) !== undefined).toBe(true)
  await turn($, fake)
  expect((await pane.find({ text: /Reading spiral/ })) !== undefined).toBe(true)
  await $.command.run({
    command: "antiburn-dev",
    args: "demo off",
    presentation,
  } as never)
  expect((await pane.find({ text: /^No checks yet$/ })) !== undefined).toBe(true)
})

test("VS Code gets the status-line text instead of a strip", async ($, on) => {
  const fake = fakeEngine(on, ["vscode"])
  await startSession($)
  await showCheckSamples($)
  expect(fake.statuses.at(-1)).toBe("antiburn · 5 ●")
})

test("the terminal gets no status-line text", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  expect(fake.statuses).toEqual([])
})

test("the desktop strip draws its own button chrome, so labels have no digit", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect((await band.find({ key: "fix" }))?.props.label).toBe("Compact now")
  expect((await band.find({ key: "view" }))?.props.label).toBe("View checks")
})

test("resolved checks fold into a group that opens on press", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await view.find({ text: /✓ Resolved 2 ▸/ })) !== undefined).toBe(true)
  expect(await view.find({ key: "row-O" })).toBe(undefined)
  await view.press({ key: "group-resolved" })
  expect((await view.find({ key: "row-O" })) !== undefined).toBe(true)
  // A resolved outcome takes the cyan of the Resolved icon, not the theme green.
  const outcome = (await view.findAll({ type: "Text" })).find(
    (node) => node.props.color === "#108EAC",
  )
  expect(outcome !== undefined).toBe(true)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  // The group state is shared, so the press above also opened the desktop group.
  const metric = (await desktop.findAll({ type: "Svg" })).find((node) =>
    String(node.props.source).includes('class="p"'),
  )
  expect(String(metric?.props.source)).toMatch(
    /\.p\{fill:#108EAC\}.*#2DA7C6.*font-weight="400"/,
  )
  // The card shows the Resolved heading's CircleCheck, and the outcome text has no ✓ glyph.
  const svgs = await desktop.findAll({ type: "Svg" })
  expect(
    svgs.some(
      (node) =>
        node.props.alt === "Resolved" && String(node.props.source).includes("m9 12 2 2 4-4"),
    ),
  ).toBe(true)
  expect(svgs.some((node) => String(node.props.alt).startsWith("✓"))).toBe(false)
})

test("the desktop mark lies behind an empty pane and under the cards otherwise", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  // The empty app-provided view leaves room for the mark.
  expect((await view.find({ key: "watermark" })) !== undefined).toBe(true)
  await showCheckSamples($)
  await view.press({ key: "group-chevron-resolved" })
  expect((await view.find({ key: "card-O" })) !== undefined).toBe(true)
  expect(await view.find({ key: "watermark" })).toBe(undefined)
  expect((await view.find({ key: "watermark-space" })) !== undefined).toBe(true)
})

test("an open card shows the check as resolved and drops its actions", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  await view.press({ key: "group-resolved" })
  await view.press({ key: "row-O" })
  expect((await view.find({ text: /^✓ Fixed$/ })) !== undefined).toBe(true)
  expect(await view.find({ key: "card-fix-O" })).toBe(undefined)
})

test("a watched check offers no second send in its detail", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  await view.press({ key: "row-X1" })
  expect(await view.find({ key: "card-fix-X1" })).toBe(undefined)
  expect(fake.submits).toEqual([])
  expect((await view.find({ key: "row-X1" })) !== undefined).toBe(true)
  // Nothing is left to fix, so the strip goes away and the chip shows the watched check.
  expect(await band.find({ key: "fix" })).toBeDefined()
  expect(await band.find({ key: "view" })).toBeDefined()
  expect(await band.findAll({ type: "Raster" })).toHaveLength(1)
  const chip = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "SessionMode",
    props: { modes: [] },
  } as never)
  expect((await chip.find({ key: "chip" }))?.props.label).toMatch(/^antiburn/)
})

test("the terminal chevron opens one card in place and closes it again", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo",
    presentation,
  } as never)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  await view.press({ key: "row-D" })
  expect((await view.find({ key: "card-fix-D" })) !== undefined).toBe(true)
  // A second card opens in place of the first, so only one card holds the f and h keys.
  await view.press({ key: "row-T" })
  expect(await view.find({ key: "card-fix-D" })).toBe(undefined)
  expect((await view.find({ key: "card-fix-T" }))?.props.hotkey).toBe("f")
  expect((await view.find({ key: "row-D" })) !== undefined).toBe(true)
  await view.press({ key: "row-T" })
  expect(await view.find({ key: "card-fix-T" })).toBe(undefined)
  expect((await view.find({ key: "row-T" }))?.props.label).toBe("▸")
})

for (const surface of ["terminal", "desktop"] as const) {
  test(`a docked pane on ${surface} keeps the footer at the bottom edge`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    await $.command.run({
      command: "antiburn-dev",
      args: "demo",
      presentation: { isFullscreen: false, columns: 100 },
    } as never)
    const props = { ...PANE_PROPS, placement: "dock" as const }
    const view = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props,
    })
    const root = await view.find({ type: "Box" })
    // The terminal takes the rows as a minimum, so Ink does not shrink an open card.
    // The desktop takes them as a fixed height, so the footer stays in view.
    if (surface === "terminal") {
      expect(root?.props.minHeight).toBe(24)
      expect(root?.props.height).toBe(undefined)
    } else {
      expect(root?.props.height).toBe(24)
      expect(root?.props.minHeight).toBe(undefined)
    }
  })
}

test("the chip counts only the checks left to fix, as the strip does", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo states",
    presentation,
  } as never)
  const chip = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "SessionMode",
    props: { modes: [] },
  } as never)
  expect((await chip.find({ key: "chip" }))?.props.label).toBe("antiburn · 5")
  // The terminal draws the state mark after the label, as the Code tab footer does.
  const text = (await chip.find({ type: "Box" }))?.text ?? ""
  expect(text.indexOf("antiburn · 5")).toBeGreaterThan(-1)
  expect(text.indexOf("antiburn · 5")).toBeLessThan(text.indexOf("●"))
})

test("the first row takes the focus when the pane opens", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await view.find({ key: "row-D" }))?.props.autoFocus).toBe(true)
})

test("the desktop pane draws each check as a card with an icon tile; the terminal does not", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await desktop.find({ key: "card-D" })) !== undefined).toBe(true)
  // A narrow card drops the area line; a wide card keeps it.
  expect(await desktop.find({ text: /^Tool calls$/ })).toBe(undefined)
  // The pane shows this session only, so the heading names no period.
  expect(await desktop.find({ text: /^this session$/ })).toBe(undefined)
  // A card fill is translucent, so the mark does not draw behind a card. It takes the free rows under the cards.
  expect(await desktop.find({ key: "watermark" })).toBe(undefined)
  expect((await desktop.find({ key: "watermark-space" })) !== undefined).toBe(true)
  const terminal = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect(await terminal.find({ key: "card-D" })).toBeUndefined()
  expect((await terminal.find({ key: "row-D" })) !== undefined).toBe(true)
})

for (const surface of ["terminal", "desktop"] as const) {
  test(`the pane footer links to the antiburn app on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await turn($, fake, { failures: 3 })
    const view = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props: PANE_PROPS,
    })
    // The terminal puts the arrow after the Link, so it follows the URL that a terminal without OSC 8 prints.
    const name = (await view.find({
      text: surface === "terminal" ? /^antiburn app$/ : /^antiburn app ↗$/,
    })) as { props?: { color?: string } } | undefined
    expect(name?.props?.color).toBe("#FF5914")
    if (surface === "terminal") {
      // The arrow is dim, as the terminal draws the URL it follows.
      const arrow = (await view.find({ text: /^ ↗$/ })) as
        { props?: { color?: string; dimColor?: boolean } } | undefined
      expect(arrow?.props?.dimColor).toBe(true)
      expect(arrow?.props?.color).toBe(undefined)
    }
    expect(await view.find({ text: /Keep your session in check/ })).toBe(undefined)
    expect(await view.find({ text: /\+r\d+/ })).toBe(undefined)
    expect((await view.find({ text: /Get more from every session with / })) !== undefined).toBe(
      true,
    )
    expect(await view.find({ text: /not findings/ })).toBe(undefined)
    expect(await view.find({ text: /Across sessions · / })).toBe(undefined)
  })
}

test("only the desktop pane draws the antiburn watermark", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await desktop.find({ key: "watermark" })) !== undefined).toBe(true)
  const terminal = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect(await terminal.find({ key: "watermark" })).toBe(undefined)
  // Only the desktop draws the small brand mark beside the footer link.
  const linkRow = async (drawing: typeof desktop | typeof terminal) =>
    (
      (await drawing.find({ key: "app-link" })) as
        { children?: { type?: string }[] } | undefined
    )?.children ?? []
  expect((await linkRow(desktop)).some((child) => child.type === "Svg")).toBe(true)
  expect((await linkRow(terminal)).some((child) => child.type === "Svg")).toBe(false)
})

test("a card offers its fix and a Show more toggle, and compacts only on a second press", async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await showCheckSamples($)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await desktop.find({ key: "card-fix-D" }))?.props.label).toBe("Compact now")
  expect((await desktop.find({ key: "card-fix-D" }))?.props.hotkey).toBe(undefined)
  // A closed card gives the first sentence as its reason. A chevron with no label opens it.
  expect(
    (await desktop.find({
      text: /^Each request now carries a very large context\.$/,
    })) !== undefined,
  ).toBe(true)
  expect(await desktop.find({ text: /^Show (more|less)$/ })).toBe(undefined)
  expect((await desktop.find({ key: "chevron-row-D" })) !== undefined).toBe(true)
  // The chevron's alt names the action, as the button has no label.
  expect((await chevronSvg(desktop, "row-D"))?.alt).toBe("Show more")
  await desktop.press({ key: "row-D" })
  // The open card gives the whole reason in one paragraph.
  expect(
    (await desktop.find({
      text: /^Each request now carries a very large context\. Every step pays for all of it again\.$/,
    })) !== undefined,
  ).toBe(true)
  // The open card gives the compact instructions but no Evidence block: the metric sits above.
  expect((await desktop.find({ text: /^Compact instructions$/ })) !== undefined).toBe(true)
  expect((await desktop.find({ text: /^Evidence$/ })) === undefined).toBe(true)
  expect((await desktop.find({ key: "card-fix-D" })) !== undefined).toBe(true)
  await desktop.press({ key: "row-D" })
  expect((await desktop.find({ text: /^Compact instructions$/ })) === undefined).toBe(true)
  await desktop.press({ key: "card-fix-D" })
  expect(fake.compacts.length).toBe(0)
  // The armed button asks for the second press itself.
  expect((await desktop.find({ text: /^Confirm compact$/ })) !== undefined).toBe(true)
  expect((await desktop.find({ text: /Press again/ })) === undefined).toBe(true)
  // Another press cancels the armed fix.
  await desktop.press({ key: "row-D" })
  expect((await desktop.find({ text: /^Confirm compact$/ })) === undefined).toBe(true)
  await desktop.press({ key: "row-D" })
  await desktop.press({ key: "card-fix-D" })
  expect(fake.compacts.length).toBe(0)
  await desktop.press({ key: "card-fix-D" })
  expect(fake.compacts.length).toBe(1)
})

test("every pane width uses one card layout, with no area line", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  for (const bodyColumns of [80, 40, 30]) {
    const view = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "checks",
      props: { ...PANE_PROPS, bodyColumns },
    })
    expect((await view.find({ key: "card-fix-D" })) !== undefined).toBe(true)
    expect((await view.find({ text: /^Tool calls$/ })) === undefined).toBe(true)
    await view.unmount()
  }
})

test("the list leaves out building checks and folds resolved checks into one section", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  // The demo has active, watched and resolved checks. The pane never shows a building check.
  await $.command.run({
    command: "antiburn-dev",
    args: "demo states",
    presentation,
  } as never)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await desktop.find({ text: /Obsolete model/ })) === undefined).toBe(true)
  expect((await desktop.find({ text: /Building evidence/ })) === undefined).toBe(true)
  expect((await desktop.find({ key: "card-X1" })) !== undefined).toBe(true)
  expect((await desktop.find({ key: "card-T" })) !== undefined).toBe(true)
  expect((await desktop.find({ key: "card-O" })) === undefined).toBe(true)
  // The chevron opens the section too.
  await desktop.press({ key: "group-chevron-resolved" })
  expect((await desktop.find({ key: "card-O" })) !== undefined).toBe(true)
  expect(String((await chevronSvg(desktop, "group-chevron-resolved"))?.source)).toMatch(
    /animation:turn/,
  )
  // The desktop title is Text, not a button, so it lines up with the Checks title.
  expect(await desktop.find({ type: "Button", key: "group-resolved" })).toBe(undefined)
  await desktop.press({ key: "group-chevron-resolved" })
  expect((await desktop.find({ key: "card-O" })) === undefined).toBe(true)
})

test("a watched check moves to its own section, and the section icon draws in once", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await desktop.find({ text: /^Awaiting verification$/ })) !== undefined).toBe(true)
  expect((await desktop.find({ text: /^Checking the next turns\./ })) !== undefined).toBe(true)
  expect(await desktop.find({ key: "card-fix-X1" })).toBe(undefined)
  const icon = (await desktop.findAll({ type: "Svg" })).find(
    (node) => node.props.alt === "Awaiting verification",
  )
  expect(String(icon?.props.source)).toMatch(/animation:enter/)
  const terminal = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await terminal.find({ text: /⧗ Awaiting verification 1/ })) !== undefined).toBe(true)
  // A press in the pane draws the pane again, so the icon stops its draw-in and does not flicker.
  await desktop.press({ key: "row-X1" })
  const settled = (await desktop.findAll({ type: "Svg" })).find(
    (node) => node.props.alt === "Awaiting verification",
  )
  expect(String(settled?.props.source)).not.toMatch(/animation/)
})

test("an open card puts its actions last, with Hide beside the fix", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  await showCheckSamples($)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect(await desktop.find({ key: "hide-D" })).toBe(undefined)
  await desktop.press({ key: "row-D" })
  // The card's text runs in drawing order: the details, then the action row.
  const text = (await desktop.find({ key: "card-D" }))?.text ?? ""
  expect(text.indexOf("Compact instructions")).toBeGreaterThan(-1)
  expect(text.indexOf("Compact instructions")).toBeLessThan(text.indexOf("Compact now"))
  expect(text.indexOf("Compact now")).toBeLessThan(text.indexOf("Ignore this session"))
  expect((await desktop.find({ key: "hide-D" })) !== undefined).toBe(true)
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`an empty pane says what it watches on ${surface}`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    const view = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props: PANE_PROPS,
    })
    expect((await view.find({ text: /^No checks yet$/ })) !== undefined).toBe(true)
    // Each word is its own Text, so a wrapped line stays centred.
    const words = (await view.findAll({ type: "Text" })).map((node) => node.text.trim())
    expect(words.join(" ")).toMatch(/antiburn is watching this session\./)
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) === undefined).toBe(true)
  })

  test(`a pane with no active check says there is nothing to fix on ${surface}`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    const presentation = { isFullscreen: false, columns: 100 }
    await $.command.run({
      command: "antiburn-dev",
      args: "demo",
      presentation,
    } as never)
    const view = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props: PANE_PROPS,
    })
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) === undefined).toBe(true)
    // A demo press acts as a real fix does and moves the card to Awaiting verification.
    for (const id of ["D", "D", "T", "E1", "K1", "K2"]) {
      if (surface === "desktop") {
        await view.press({ key: `card-fix-${id}` })
      } else {
        if ((await view.find({ key: `row-${id}` })) === undefined) continue
        if ((await view.find({ key: `card-fix-${id}` })) === undefined)
          await view.press({ key: `row-${id}` })
        // Compact needs a second press. After the first press of any other fix, the button is gone.
        while ((await view.find({ key: `card-fix-${id}` })) !== undefined)
          await view.press({ key: `card-fix-${id}` })
      }
    }
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) !== undefined).toBe(true)
  })

  test(`a pane with every check ignored says there is nothing to fix, with no active mark, on ${surface}`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    const presentation = { isFullscreen: false, columns: 100 }
    await $.command.run({
      command: "antiburn-dev",
      args: "demo",
      presentation,
    } as never)
    const view = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props: PANE_PROPS,
    })
    for (const id of ["X1", "D", "T", "O", "E1", "K1", "C1", "K2"]) {
      if ((await view.find({ key: `row-${id}` })) === undefined) continue
      // A card shows Ignore only while it is open, on both surfaces.
      if ((await view.find({ key: `hide-${id}` })) === undefined)
        await view.press({ key: `row-${id}` })
      if ((await view.find({ key: `hide-${id}` })) !== undefined)
        await view.press({ key: `hide-${id}` })
    }
    expect(await view.find({ text: /^No checks yet$/ })).toBe(undefined)
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) !== undefined).toBe(true)
    expect(await view.find({ text: /^● $/ })).toBe(undefined)
    expect(await view.find({ key: "checks-icon" })).toBe(undefined)
  })
}

test("the strip leads with an active check over a watched one", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 150 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo states",
    presentation,
  } as never)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(150),
  })
  // Retry loop comes first in catalog order, but it is watched, so Context heavy leads.
  expect((await band.find({ key: "fix" }))?.props.label).toBe("1 Compact now")
  expect((await band.find({ text: / to fix {2}Context heavy · / })) !== undefined).toBe(true)
})

test("the open Context heavy card names file reads as the cause", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo",
    presentation,
  } as never)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect(await desktop.find({ text: /File reads are about/ })).toBe(undefined)
  await desktop.press({ key: "row-D" })
  expect((await desktop.find({ text: /File reads are about 61% of it/ })) !== undefined).toBe(
    true,
  )
})

test("the open terminal card follows the desktop card: Cause under the reason, no Evidence, keys on the buttons", async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo",
    presentation,
  } as never)
  const terminal = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  // A skill name in a metric draws in bold, in place of the desktop pill.
  const skill = (await terminal.findAll({ type: "Text" })).find(
    (node) => node.props.bold === true && /^\/\S+$/.test(node.text ?? ""),
  )
  expect(skill !== undefined).toBe(true)
  await terminal.press({ key: "row-D" })
  expect(await terminal.find({ text: /^Evidence$/ })).toBe(undefined)
  expect(await terminal.find({ text: /this session so far/ })).toBe(undefined)
  expect(
    (await terminal.find({
      text: /^Cause · File reads are about 61% of it/,
    })) !== undefined,
  ).toBe(true)
  expect((await terminal.find({ key: "card-fix-D" }))?.props.label).toBe("f Compact now")
  expect((await terminal.find({ key: "hide-D" }))?.props.label).toBe("h Ignore this session")
})

test("a demo press acts as a real fix does, and the card moves to Awaiting verification", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, "default")
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo",
    presentation,
  } as never)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  await desktop.press({ key: "card-fix-E1" })
  await desktop.press({ key: "card-fix-T" })
  await desktop.press({ key: "card-fix-D" })
  await desktop.press({ key: "card-fix-D" })
  expect(fake.submits.length).toBe(1)
  expect(fake.fills).toEqual(["/effort high"])
  expect(fake.compacts.length).toBe(1)
  expect(await desktop.find({ key: "card-fix-E1" })).toBe(undefined)
  expect(await desktop.find({ text: /Demo: nothing was sent/ })).toBe(undefined)
})

test("a terminal card compacts only on a second press, and a group press disarms it", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, "default")
  const presentation = { isFullscreen: false, columns: 150 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo",
    presentation,
  } as never)
  const view = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  await view.press({ key: "row-D" })
  await view.press({ key: "card-fix-D" })
  expect(fake.compacts).toEqual([])
  expect((await view.find({ key: "card-fix-D" }))?.props.label).toBe("f Confirm compact")
  // A press on the Resolved heading is a press in the pane, so it disarms the fix, as on the desktop.
  await view.press({ key: "group-resolved" })
  expect((await view.find({ key: "card-fix-D" }))?.props.label).toBe("f Compact now")
  await view.press({ key: "card-fix-D" })
  await view.press({ key: "card-fix-D" })
  expect(fake.compacts.length).toBe(1)
})

test("the terminal strip in the demo compacts as a real fix does", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, "default")
  const presentation = { isFullscreen: false, columns: 150 }
  await $.command.run({
    command: "antiburn-dev",
    args: "demo",
    presentation,
  } as never)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(150),
  })
  await band.press({ key: "fix" })
  await band.press({ key: "fix" })
  expect(fake.compacts.length).toBe(1)
})

test("the mod does not read installed skills to create a local live check", async ($, on) => {
  const fake = fakeEngine(on)
  let lists = 0
  on("command.list", () => {
    lists += 1
    return {
      value: [
        {
          name: "review-and-draft-pr",
          description: "Review the change and open a draft pull request.",
          source: "plugin",
        },
        { name: "antiburn-dev", description: "antiburn checks", source: "plugin" },
      ],
    }
  })
  await startSession($)
  await startSession($)
  await turn($, fake)
  expect(lists).toBe(0)
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await desktop.find({ text: /^No checks yet$/ })) !== undefined).toBe(true)
  expect(fake.submits).toEqual([])
  expect(lists).toBe(0)
})

test("a fix with a command takes one line, and a long prompt shows about three lines", () => {
  // A plugin skill shows its short name; the fix still sends the full name.
  expect(commandSummary({ id: "K1", prompt: "/anthropic-skills:docs" })).toEqual({
    name: "/docs",
  })
  expect(commandSummary({ id: "O", prompt: "/model claude-sonnet-5-5" })).toEqual({
    name: "/model claude-sonnet-5-5",
  })
  expect(
    commandSummary({
      id: "C1",
      prompt: "List each change you made outside the plan.",
    }),
  ).toBe(null)
  const long = "word ".repeat(60).trim()
  const preview = promptPreview(long)
  expect(preview.isCut).toBe(true)
  expect(preview.text.length).toBeLessThanOrEqual(181)
  expect(preview.text.endsWith("word…")).toBe(true)
  expect(promptPreview("short")).toEqual({ text: "short", isCut: false })
})

test("the demo tour adds checks step by step and keeps the live checks apart", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 150 }
  const run = (args: string) =>
    $.command.run({ command: "antiburn-dev", args, presentation } as never)
  expect((await run("demo next")).text).toMatch(/^Start the tour first/)
  await run("demo tour")
  const pane = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(150),
  })
  expect((await pane.find({ text: /^No checks yet$/ })) !== undefined).toBe(true)
  // A real turn feeds only the live tracker, so the tour does not show its retry loop.
  await turn($, fake, { failures: 3 })
  expect(await pane.find({ text: /Retry loop/ })).toBe(undefined)
  expect((await run("demo next")).text).toBe(
    "Tour step 1 of 4: A retry loop is the first warning.",
  )
  expect((await band.find({ text: /Retry loop/ })) !== undefined).toBe(true)
  await pane.press({ key: "tour-next" })
  expect((await band.find({ text: /4 to fix \(3 new\)/ })) !== undefined).toBe(true)
  await run("demo next")
  await run("demo next")
  expect((await pane.find({ text: /Demo tour · step 4 of 4/ })) !== undefined).toBe(true)
  expect((await run("demo next")).text).toMatch(/^The tour has no more steps/)
  await pane.press({ key: "tour-end" })
  expect(await pane.find({ key: "tour" })).toBe(undefined)
  // Ending the tour returns to the empty app-provided checks view.
  expect((await pane.find({ text: /Retry loop/ })) === undefined).toBe(true)
})

test("the chip does not count a building check, so it agrees with the pane", async ($, on) => {
  fakeEngine(on, ["desktop"])
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  const run = (args: string) =>
    $.command.run({ command: "antiburn-dev", args, presentation } as never)
  await run("demo tour")
  await run("demo next")
  const pane = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  const chip = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "SessionMode",
    props: { modes: [] },
  } as never)
  expect((await chip.find({ key: "chip" }))?.props.label).toBe("antiburn · 1")
  // Step 1 also holds T at building. The pane leaves it out, and so does the chip.
  await pane.press({ key: "row-X1" })
  await pane.press({ key: "hide-X1" })
  expect((await pane.find({ text: /^Nothing to fix right now\.$/ })) !== undefined).toBe(true)
  expect((await chip.find({ key: "chip" }))?.props.label).toBe("antiburn")
  expect((await chip.find({ text: /^✓$/ })) !== undefined).toBe(true)
})

test("a fix pressed in the demo tour acts for real and resolves on the next step", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, "default")
  const presentation = { isFullscreen: false, columns: 100 }
  const run = (args: string) =>
    $.command.run({ command: "antiburn-dev", args, presentation } as never)
  await run("demo tour")
  for (let step = 0; step < 2; step += 1) await run("demo next")
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  await desktop.press({ key: "card-fix-X1" })
  await desktop.press({ key: "card-fix-T" })
  expect(fake.submits.length).toBe(1)
  expect(fake.fills).toEqual(["/effort high"])
  expect(await desktop.find({ key: "card-fix-X1" })).toBe(undefined)
  expect(await desktop.find({ key: "card-fix-T" })).toBe(undefined)
  await run("demo next")
  const terminal = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  await terminal.press({ key: "group-resolved" })
  expect((await terminal.find({ text: /✓ Fixed/ })) !== undefined).toBe(true)
})

test("the desktop watermark stays above the tour row, so Next step takes clicks", async ($, on) => {
  fakeEngine(on, ["desktop"])
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  const desktop = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "checks",
    props: PANE_PROPS,
  })
  expect((await desktop.find({ key: "watermark" }))?.props.bottom).toBe(1)
  await $.command.run({
    command: "antiburn-dev",
    args: "demo tour",
    presentation,
  } as never)
  expect((await desktop.find({ key: "watermark" }))?.props.bottom).toBe(2)
  await desktop.press({ key: "tour-next" })
  expect((await desktop.find({ text: /step 1 of 4/ })) !== undefined).toBe(true)
})

test("a new local event does not open the checks pane", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  expect(fake.opens).toEqual([])
  expect(fake.focusedOpens).toEqual([])
})

test("a new check leaves an open pane as it is", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  fake.panes = ["checks"]
  await turn($, fake, { failures: 3 })
  expect(fake.opens).toEqual([])
})

test("help, status, diagnostics, and usage routes coexist with live checks", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "checks"
  fake.rateLimits = [
    { kind: "five_hour", percentUsed: 8 },
    { kind: "seven_day", percentUsed: 1 },
  ]
  await startSession($)
  const run = (args: string) =>
    $.command.run({
      command: "antiburn-dev",
      args,
      origin: { kind: "composer" },
      presentation: { isFullscreen: false, columns: 80 },
    })
  expect((await run("help")).text).toContain("[help|status|diagnostics|usage")
  expect((await run("status")).text).toContain("App connection: Not checked")
  const diagnostics = (await run("diagnostics")).text
  expect(diagnostics).toContain("CLI: Check unavailable")
  expect(diagnostics).toContain("Checks pane: enabled; local detectors: disabled")
  expect((await run("usage")).text).toContain("5-hour")
  expect((await run("usage refresh")).text).toContain("5-hour")
  expect(fake.usageReads).toBe(3)
  expect((await run("")).text).toBe("Opened the antiburn checks pane.")
})

test("release ownership suppresses the dev quota before session start", async ($, on) => {
  const fake = fakeEngine(on)
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  let descriptorReads = 0
  on("engine.create", async (_$, e, next) => {
    const built = await next(e)
    return {
      ...built,
      antiburn: {
        describe: async () => {
          descriptorReads += 1
          return {
            id: "antiburn",
            version: "0.2.0",
            capabilities: { usage: true, usageUiEnabled: true },
          }
        },
      },
    }
  })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(descriptorReads).toBe(1)
  expect(fake.usageReads).toBe(0)
  expect(
    (await band.findAll({ type: "Svg" })).filter((node) =>
      /^5-hour:/.test(String(node.props.alt ?? "")),
    ),
  ).toHaveLength(0)
})

for (const [state, message] of [
  ["found", "CLI found"],
  ["not-found", "CLI not found"],
  ["unavailable", "CLI check unavailable"],
] as const) {
  test(`startup notice reports ${state} CLI without blocking live checks`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "checks"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    let probes = 0
    on("process.run", () => {
      probes += 1
      if (state === "unavailable") throw new Error("probe failed")
      return {
        value: {
          exitCode: 0,
          stdout: JSON.stringify({ state }),
          stderr: "",
          isStdoutTruncated: false,
          isStderrTruncated: false,
        },
      }
    })
    await startSession($)
    expect(fake.logs.some((line) => line.includes(message))).toBe(true)
    await turn($, fake, { failures: 3 })
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect(await band.find({ text: /to fix/ })).toBeUndefined()
    expect(fake.usageReads).toBe(1)
    expect(probes).toBe(1)
  })
}

test("startup notice probes once when the same session starts again", async ($, on) => {
  const fake = fakeEngine(on)
  let probes = 0
  on("process.run", () => {
    probes += 1
    return {
      value: {
        exitCode: 0,
        stdout: '{"state":"found"}',
        stderr: "",
        isStdoutTruncated: false,
        isStderrTruncated: false,
      },
    }
  })
  await startSession($)
  await $.session.start({
    cwd: "/work",
    surface: "terminal",
    isInteractive: true,
  })
  expect(probes).toBe(1)
  expect(fake.logs.filter((line) => line.includes("CLI found"))).toHaveLength(1)
})

test(
  "disabling the dev usage UI avoids automatic usage reads and local checks",
  { options: { usageUiEnabled: false, startupNoticeEnabled: false } },
  async ($, on) => {
    const fake = fakeEngine(on)
    const processes: string[][] = []
    on("process.run", (_$, e) => {
      processes.push([...e.argv])
      return {
        value: {
          exitCode: 0,
          stdout: '{"state":"found"}',
          stderr: "",
          isStdoutTruncated: false,
          isStderrTruncated: false,
        },
      }
    })
    fake.liveMode = "checks"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    expect(fake.usageReads).toBe(0)
    expect(processes).toEqual([])
    await turn($, fake, { failures: 3 })
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect(await band.find({ text: /to fix/ })).toBeUndefined()
    expect(await band.find({ text: /^8%$/ })).toBeUndefined()
    expect(fake.usageReads).toBe(0)
  },
)

for (const flag of [undefined, "usage", "invalid", "CHECKS"]) {
  test(`usage mode isolates checks for flag ${String(flag)}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = flag
    fake.rateLimits = [
      { kind: "five_hour", percentUsed: 8 },
      { kind: "seven_day", percentUsed: 1 },
    ]
    await startSession($)
    fake.model = "claude-opus-4-8"
    fake.usage = DEEP
    await turn($, fake, { failures: 4 })
    for (const args of ["checks", "demo", "demo tour", "demo next", "demo off"]) {
      const result = await $.command.run({
        command: "antiburn-dev",
        args,
        origin: { kind: "composer" },
        presentation: { isFullscreen: false, columns: 80 },
      })
      expect(result.text).toBe("Usage: /antiburn-dev")
    }
    expect(fake.stateWrites.filter((key) => key !== "usage" && key !== "burn")).toEqual([])
    expect(fake.statuses).toEqual([])
    expect(fake.opens).toEqual([])
    expect(fake.submits).toEqual([])
    expect(fake.fills).toEqual([])
    expect(fake.compacts).toEqual([])
    expect(fake.writes).toEqual([])
    for (const isWorking of [false, true]) {
      const band = await $.ui.mount({
        plugin: "antiburn-live",
        surface: "terminal",
        component: "AbovePrompt",
        props: stripProps(80, isWorking),
      })
      expect(await band.find({ text: /^8%$/ })).toBeDefined()
      expect(await band.find({ key: "fix" })).toBeUndefined()
    }
    await $.command.run({
      command: "antiburn-dev",
      args: "",
      origin: { kind: "composer" },
      presentation: { isFullscreen: false, columns: 80 },
    })
    expect(fake.opens).toEqual([])
    expect(fake.usageReads).toBe(1)
  })
}

test("a newer measurement wins over a delayed initial usage snapshot", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  let release!: (limits: RateLimitReading[]) => void
  let started!: () => void
  const reading = new Promise<void>((resolve) => {
    started = resolve
  })
  fake.readUsage = () => {
    started()
    return new Promise((resolve) => {
      release = resolve
    })
  }
  const starting = startSession($)
  await reading
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 94 }],
    changed: ["rateLimits"],
  })
  release([{ kind: "five_hour", percentUsed: 8 }])
  await starting
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await band.find({ text: /^94%$/ })).toBeDefined()
  expect(await band.find({ text: /^8%$/ })).toBeUndefined()
  expect(fake.usageReads).toBe(1)
})

test("failed refresh retains labeled values, and an empty measurement clears them", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  fake.readUsage = async () => {
    throw new Error("unavailable")
  }
  const debug = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "debug",
    props: PANE_PROPS,
  })
  await debug.press({ key: "usage-refresh" })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await band.find({ text: /Last known/ })).toBeDefined()
  expect(await band.find({ text: /^8%$/ })).toBeDefined()
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [],
    changed: ["rateLimits"],
  })
  expect(await band.find({ text: /^8%$/ })).toBeUndefined()
  expect(await band.find({ text: "Usage not available yet" })).toBeDefined()
  expect(fake.usageReads).toBe(2)
})

test(
  "explicit debug refresh still reads usage when the automatic UI is disabled",
  { options: { usageUiEnabled: false, startupNoticeEnabled: false } },
  async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "usage"
    await startSession($)
    expect(fake.usageReads).toBe(0)
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 42 }]
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    await debug.press({ key: "usage-refresh" })
    expect(fake.usageReads).toBe(1)
    expect(await debug.find({ text: /42%/ })).toBeDefined()
  },
)

test("clear reconciles through its command when classic hooks are unavailable and retains mode", async ($, on) => {
  const fake = fakeEngine(on)
  on("command.run", () => ({ text: "Cleared" }))
  fake.liveMode = "usage"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  fake.liveMode = "checks"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 9 }]
  await $.session.end({ reason: "clear", sessionId: "s", resume: {} } as never)
  await $.command.run({
    command: "clear",
    args: "",
    origin: { kind: "composer" },
    presentation: { isFullscreen: false, columns: 80 },
  })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await band.find({ text: /^9%$/ })).toBeDefined()
  expect(fake.usageReads).toBe(2)
  await turn($, fake, { failures: 3 })
  expect(await band.find({ key: "fix" })).toBeUndefined()
  expect(fake.usageReads).toBe(2)
})

test("a late usage failure cannot replace a newer measurement", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  let reject!: (error: Error) => void
  let started!: () => void
  const reading = new Promise<void>((resolve) => {
    started = resolve
  })
  fake.readUsage = () => {
    started()
    return new Promise((_resolve, fail) => {
      reject = fail
    })
  }
  const starting = startSession($)
  await reading
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "seven_day", percentUsed: 2.5 }],
    changed: ["rateLimits"],
  })
  reject(new Error("old read failed"))
  await starting
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await band.find({ text: /^2.5%$/ })).toBeDefined()
  expect(await band.find({ text: /Last known/ })).toBeUndefined()
})

test("reload into usage mode clears prior check UI and disables stale actions", async ($, on) => {
  const fake = fakeEngine(on, ["mobile"])
  await startSession($)
  await turn($, fake, { failures: 3 })
  expect(fake.statuses.length > 0).toBe(true)
  fake.liveMode = "usage"
  await $.session.start({
    cwd: "/work",
    surface: "terminal",
    isInteractive: true,
  })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await band.find({ key: "fix" })).toBeUndefined()
  expect(fake.statuses[fake.statuses.length - 1]).toBeUndefined()
  expect(fake.panes.includes("checks")).toBe(false)
  const before = fake.stateWrites.filter((key) => key !== "burn").length
  await turn($, fake, { failures: 3 })
  expect(fake.stateWrites.filter((key) => key !== "burn").length).toBe(before)
  expect(fake.submits).toEqual([])
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`usage preserves exact boundary values on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "usage"
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(80),
    })
    for (const percentUsed of [0, 1, 2.5, 8, 94, 99.5, 100, 105]) {
      await $.session.measure({
        context: { window: 200000 },
        rateLimits: [{ kind: "five_hour", percentUsed }],
        changed: ["rateLimits"],
      })
      const fiveHour =
        surface === "desktop"
          ? (await band.findAll({ type: "Svg" })).find((node) =>
              String(node.props.alt).startsWith("5-hour:"),
            )?.props.alt
          : (await band.find({ key: "usage-detail-five_hour" }))?.props.label
      const weekly =
        surface === "desktop"
          ? (await band.findAll({ type: "Svg" })).find((node) =>
              String(node.props.alt).startsWith("Weekly:"),
            )?.props.alt
          : (await band.find({ key: "usage-detail-seven_day" }))?.props.label
      expect(fiveHour).toMatch(
        new RegExp(
          `^5-hour: ${String(surface === "desktop" ? Math.floor(percentUsed) : percentUsed).replace(".", "\\.")}%\\.`,
        ),
      )
      expect(weekly).toMatch(/^Weekly: —\./)
    }
    expect(fake.usageReads).toBe(1)
    if (surface === "desktop") {
      const graphic = (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )
      expect(String(graphic?.props.source)).toContain("bitcount-pixel")
      expect(String(graphic?.props.source)).toContain("Reset unavailable")
      expect(await band.find({ key: "usage-expanded-five_hour" })).toBeUndefined()
      return
    }
    await band.press({ key: "usage-detail-five_hour" })
    expect(await band.find({ text: /^Reset unavailable$/ })).toBeDefined()
    await $.ui.focus({
      component: "AbovePrompt",
      requestId: "usage-test",
      plugin: "antiburn-live",
      element: "usage-detail-five_hour",
      origin: { kind: "person" },
    })
    await band.press({ key: "usage-detail-five_hour" })
    expect(await band.find({ key: "usage-expanded-five_hour" })).toBeUndefined()
  })
}

test("desktop entrance has one bounded owner and newer data cancels delayed fill", async ($, on) => {
  const fake = fakeEngine(on, ["desktop"])
  fake.liveMode = "usage"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(80),
  })
  expect(fake.presentationTimers).toEqual([])
  expect(
    String(
      (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )?.props.source,
    ),
  ).toContain("animation:meter-in")
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 9 }],
    changed: ["rateLimits"],
  })
  const settled = String(
    (await band.findAll({ type: "Svg" })).find((node) =>
      String(node.props.alt).startsWith("5-hour:"),
    )?.props.source,
  )
  expect(settled).not.toContain("animation:meter-in")
  expect(settled).not.toContain('class="roll-old"')
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 10 }],
    changed: ["rateLimits"],
  })
  expect(
    String(
      (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )?.props.source,
    ),
  ).toContain('class="roll-old"')
  expect(fake.presentationTimers).toEqual([])
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [],
    changed: ["rateLimits"],
  })
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 11 }],
    changed: ["rateLimits"],
  })
  expect(
    String(
      (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )?.props.source,
    ),
  ).not.toContain('class="roll-old"')
})

test("terminal fallback schedules no presentation timers", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(80),
  })
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 9 }],
    changed: ["rateLimits"],
  })
  expect(fake.presentationTimers).toEqual([])
})

test("completed desktop entrance permits the next numeric roll without scheduled cleanup", async ($, on) => {
  const fake = fakeEngine(on, ["desktop"])
  fake.liveMode = "usage"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 9 }]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(80),
  })
  const source = async () =>
    String(
      (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )?.props.source,
    )
  expect(await source()).toContain("animation:meter-in")
  fake.now += 960
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 10 }],
    changed: ["rateLimits"],
  })
  expect(await source()).toContain('class="roll-old"')
  expect(await source()).not.toContain("animation:meter-in")
  const completedRollSource = await source()
  fake.now += 284
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 10 }],
    changed: ["rateLimits"],
  })
  expect(await source()).toBe(completedRollSource)
  expect(fake.presentationTimers).toEqual([])
})

for (const surface of ["desktop", "terminal"] as const) {
  for (const flag of [undefined, "checks"] as const) {
    test(`debug is unadvertised and alone shows the effective mode on ${surface} with ${String(flag)}`, async ($, on) => {
      const fake = fakeEngine(on, [surface])
      fake.liveMode = flag
      await startSession($)
      expect(fake.commands).toHaveLength(1)
      expect(fake.commands[0]?.argumentHint ?? "").not.toMatch(/debug/)
      const run = (args: string) =>
        $.command.run({
          command: "antiburn-dev",
          args,
          origin: { kind: "composer" },
          presentation: { isFullscreen: false, columns: 80 },
        })
      expect((await run("debug")).text).toBe("Opened the antiburn debug pane.")
      expect(fake.opens).toEqual(["debug"])
      const debug = await $.ui.mount({
        plugin: "antiburn-live",
        surface,
        component: "Pane",
        requestId: "debug",
        props: PANE_PROPS,
      })
      expect(
        await debug.find({
          text:
            flag === "checks" ? "Mode 1 · Usage limits + live checks" : "Mode 0 · Usage limits",
        }),
      ).toBeDefined()
      expect(await debug.find({ key: "usage-preview-mid" })).toBeDefined()
      expect((await run("")).text).toBe(
        flag === "checks"
          ? "Opened the antiburn checks pane."
          : "Usage limits are shown above the prompt.",
      )
      const retired = await $.ui.mount({
        plugin: "antiburn-live",
        surface,
        component: "Pane",
        requestId: "usage",
        props: PANE_PROPS,
      })
      expect(await retired.find({ text: "Usage" })).toBeUndefined()
      expect(await debug.find({ key: "debug-usage-card" })).toBeDefined()
      expect(await debug.find({ key: "debug-checks-card" })).toBeDefined()
      for (const key of ["debug-checks-card", "debug-burn-card", "debug-usage-card"]) {
        const card = await debug.find({ key })
        if (surface === "terminal") {
          expect(card!.props.backgroundColor).toBeUndefined()
          expect(card!.props.borderColor).toBe("subtle")
        } else expect(card!.props.backgroundColor).toBeDefined()
      }
      expect((await run("unknown")).text).not.toMatch(/debug/)
      await run("checks")
      const checks = await $.ui.mount({
        plugin: "antiburn-live",
        surface,
        component: "Pane",
        requestId: "checks",
        props: PANE_PROPS,
      })
      expect(await checks.find({ text: /^Mode [01]/ })).toBeUndefined()
      expect(fake.opens).toEqual(flag === "checks" ? ["debug", "checks", "checks"] : ["debug"])
      expect(fake.submits).toEqual([])
      expect(fake.fills).toEqual([])
    })
  }
}

for (const surface of ["desktop", "terminal"] as const) {
  test(`usage previews are explicit, preserve native readings, and return to live on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    fake.liveMode = "usage"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const usagePane = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    const pane = usagePane
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect(await usagePane.find({ key: "usage-preview-mid" })).toBeDefined()
    expect(await usagePane.find({ key: "usage-debug-menu" })).toBeUndefined()
    const visible = async (text: string) =>
      surface === "desktop"
        ? (await band.findAll({ type: "Svg" })).some((node) =>
            String(node.props.alt).includes(text),
          )
        : (await band.findAll({ type: "Text" })).some((node) => node.text.includes(text))
    const initialReads = fake.usageReads
    await pane.press({ key: "usage-state-idle" })
    expect(await visible("Usage unavailable")).toBe(true)
    await pane.press({ key: "usage-state-checking" })
    if (surface === "terminal") {
      const phases = async () =>
        (await band.findAll({ type: "Text", in: "usage-checking" }))
          .filter((node) => node.text === ".")
          .map((node) => node.props.dimColor)
      expect(await band.find({ key: "usage-checking" })).toBeDefined()
      expect(await phases()).toEqual([false, true, true])
      await band.advance(300)
      expect(await phases()).toEqual([true, false, true])
    } else expect(await visible("Checking usage...")).toBe(true)
    expect(fake.usageReads).toBe(initialReads)
    await pane.press({ key: "usage-state-retry" })
    expect(await visible("Account retry at")).toBe(true)
    expect(await visible("Refresh checks session.")).toBe(true)
    if (surface === "terminal") {
      expect((await band.find({ key: "usage-retry-copy" }))?.props.height).toBeUndefined()
      expect((await band.find({ key: "usage-state-refresh" }))?.props.label).toBe("[↻]")
    }
    await band.press({ key: "usage-state-refresh" })
    if (surface === "terminal") expect(await band.find({ key: "usage-checking" })).toBeDefined()
    else expect(await visible("Checking usage...")).toBe(true)
    expect(fake.usageReads).toBe(initialReads)
    await pane.press({ key: "usage-state-success" })
    expect(await visible("31%")).toBe(true)
    await pane.press({ key: "usage-state-stale" })
    expect(await visible("Usage unavailable · Last known")).toBe(true)
    expect(await visible("31%")).toBe(true)
    expect(fake.usageReads).toBe(initialReads)
    expect(fake.submits).toEqual([])
    await pane.press({ key: "usage-preview-live" })
    expect(await visible("8%")).toBe(true)
    expect(fake.usageReads).toBe(initialReads)

    await pane.press({ key: "usage-preview-mid" })
    expect(await band.find({ text: /Synthetic usage · mid/ })).toBeUndefined()
    expect(await usagePane.find({ text: "5-hour: 48%" })).toBeDefined()
    await $.session.measure({
      context: { window: 200000 },
      rateLimits: [{ kind: "five_hour", percentUsed: 12 }],
      changed: ["rateLimits"],
    })
    expect(await usagePane.find({ text: "5-hour: 48%" })).toBeDefined()
    await pane.press({ key: "usage-preview-near-limit" })
    expect(await usagePane.find({ text: "5-hour: 98%" })).toBeDefined()
    if (surface === "desktop") {
      expect(
        String(
          (await band.findAll({ type: "Svg" })).find((node) =>
            String(node.props.alt).startsWith("5-hour:"),
          )?.props.source,
        ),
      ).toMatch(/debug-pulse/)
      await pane.press({ key: "usage-preview-reveal" })
      expect(
        String(
          (await band.findAll({ type: "Svg" })).find((node) =>
            String(node.props.alt).startsWith("5-hour:"),
          )?.props.source,
        ),
      ).toMatch(/debug-reset 280ms/)
      await pane.press({ key: "usage-preview-hide" })
      expect(
        String(
          (await band.findAll({ type: "Svg" })).find((node) =>
            String(node.props.alt).startsWith("5-hour:"),
          )?.props.source,
        ),
      ).toMatch(/debug-reset 180ms/)
    }
    expect(await pane.find({ key: "usage-preview-live" })).toBeDefined()
    await usagePane.press({ key: "usage-preview-live" })
    expect(await band.find({ text: /Synthetic usage/ })).toBeUndefined()
    expect(await usagePane.find({ text: "5-hour: 12%" })).toBeDefined()
    if (surface === "desktop")
      expect(
        String(
          (await band.findAll({ type: "Svg" })).find((node) =>
            String(node.props.alt).startsWith("5-hour:"),
          )?.props.source,
        ),
      ).not.toMatch(/debug-pulse/)
    expect(fake.submits).toEqual([])
    expect(fake.presentationTimers).toEqual([])
    expect(fake.usageReads).toBe(1)
  })
}

test("a delayed animation inspector cannot restore a preview after returning to live", async ($, on) => {
  const fake = fakeEngine(on, ["desktop"])
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  const usagePane = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "debug",
    props: PANE_PROPS,
  })
  const pane = usagePane
  await pane.press({ key: "usage-preview-mid" })
  let release!: (value: number) => void
  let started!: () => void
  const waiting = new Promise<void>((resolve) => {
    started = resolve
  })
  let first = true
  fake.clockRead = () => {
    if (!first) return Promise.resolve(fake.now)
    first = false
    started()
    return new Promise<number>((resolve) => {
      release = resolve
    })
  }
  const delayed = pane.press({ key: "usage-preview-entrance" })
  await waiting
  await pane.press({ key: "usage-preview-live" })
  release(fake.now)
  await delayed
  expect(await pane.find({ text: /Preview: mid/ })).toBeUndefined()
  expect(await usagePane.find({ text: "5-hour: 8%" })).toBeDefined()
})

test("desktop meter data stays outside the interactive frame and duplicate measurements preserve its source", async ($, on) => {
  const fake = fakeEngine(on, ["desktop"])
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(),
  })
  const source = async () =>
    (await band.findAll({ type: "Svg" })).find((node) =>
      String(node.props.alt).startsWith("5-hour:"),
    )!
  const initial = await source()
  expect(initial.props.isInteractive).not.toBe(true)
  fake.now += 1000
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: fake.rateLimits,
    changed: ["rateLimits"],
  })
  expect((await source()).props.source).toBe(initial.props.source)
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`debug switches live checks on and off without changing usage on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    fake.liveMode = "usage"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    const usagePane = debug
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect(await debug.find({ key: "usage-preview-exceeded" })).toBeUndefined()
    expect(await debug.find({ key: "usage-preview-decimal" })).toBeUndefined()
    await debug.press({ key: "usage-preview-mid" })
    await debug.press({ key: "usage-debug-mode" })
    expect(await debug.find({ text: "Mode 1 · Usage limits + live checks" })).toBeDefined()
    expect(await debug.find({ key: "debug-checks" })).toBeDefined()
    expect(await usagePane.find({ key: "debug-checks" })).toBeDefined()
    expect(await usagePane.find({ text: "5-hour: 48%" })).toBeDefined()
    expect(fake.commands.at(-1)?.argumentHint).toMatch(/checks/)
    await turn($, fake, { failures: 3 })
    expect(await band.find({ text: /to fix/ })).toBeUndefined()
    await debug.press({ key: "debug-checks" })
    expect(fake.panes).toContain("checks")
    await debug.press({ key: "usage-debug-mode" })
    expect(await debug.find({ text: "Mode 0 · Usage limits" })).toBeDefined()
    expect(await debug.find({ key: "debug-checks" })).toBeUndefined()
    expect(await usagePane.find({ key: "debug-checks" })).toBeUndefined()
    expect(fake.panes).not.toContain("checks")
    expect(fake.statuses.at(-1)).toBeUndefined()
    expect(fake.commands.at(-1)?.argumentHint).toBeUndefined()
    expect(await band.find({ text: /Retry loop/ })).toBeUndefined()
    await turn($, fake, { failures: 4 })
    await debug.press({ key: "usage-debug-mode" })
    expect(await band.find({ text: /Retry loop/ })).toBeUndefined()
    await usagePane.press({ key: "usage-preview-live" })
    expect(await usagePane.find({ text: "5-hour: 8%" })).toBeDefined()
    expect(fake.submits).toEqual([])
    expect(fake.fills).toEqual([])
    await $.session.start({ cwd: "/work", surface, isInteractive: true })
    expect(await debug.find({ text: "Mode 0 · Usage limits" })).toBeDefined()
  })
}

for (const id of ["exceeded", "decimal"]) {
  test(`reload clears the removed ${id} preview and resumes live usage`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "usage"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    let seeded = false
    on("state.get", (_$, e, next) => {
      if (e.plugin !== "antiburn-live" || e.key !== "usage" || seeded) return next(e)
      seeded = true
      return {
        value: {
          value: {
            preview: { id, capturedAt: fake.now },
            previewReveal: "show" as const,
            snapshot: {
              status: "ready" as const,
              windows: [],
              capturedAt: fake.now,
            },
            revealed: null,
            activated: null,
          },
          version: 0,
        },
      }
    })
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    expect(await band.find({ text: /Synthetic usage/ })).toBeUndefined()
    expect(await debug.find({ text: "Live usage data" })).toBeDefined()
    await $.session.measure({
      context: { window: 200000 },
      rateLimits: [{ kind: "five_hour", percentUsed: 12 }],
      changed: ["rateLimits"],
    })
    const usagePane = debug
    expect(await usagePane.find({ text: "5-hour: 12%" })).toBeDefined()
  })
}

test("a pending measurement cannot populate checks after switching away and back", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  const debug = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "debug",
    props: PANE_PROPS,
  })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(),
  })
  let release!: (value: number) => void
  let entered!: () => void
  const waiting = new Promise<void>((resolve) => {
    entered = resolve
  })
  let first = true
  fake.clockRead = () => {
    if (!first) return Promise.resolve(fake.now)
    first = false
    entered()
    return new Promise<number>((resolve) => {
      release = resolve
    })
  }
  const pending = $.session.measure({
    context: { window: 200000, percent: 96 },
    rateLimits: [{ kind: "five_hour", percentUsed: 8 }],
    changed: ["context", "rateLimits"],
  })
  await waiting
  await debug.press({ key: "usage-debug-mode" })
  await debug.press({ key: "usage-debug-mode" })
  release(fake.now)
  await pending
  expect(await band.find({ text: /Context heavy/ })).toBeUndefined()
  await $.session.measure({
    context: { window: 200000, percent: 96 },
    rateLimits: [],
    changed: ["context"],
  })
  expect(await band.find({ text: /Context heavy/ })).toBeUndefined()
  await $.session.measure({
    context: { window: 200000, percent: 96 },
    rateLimits: [],
    changed: ["context"],
  })
    expect(await band.find({ text: /Context heavy/ })).toBeUndefined()
})

test("reload closes the retired usage pane", async ($, on) => {
  const fake = fakeEngine(on)
  fake.panes = ["usage", "debug"]
  await startSession($)
  expect(fake.panes).toEqual(["debug"])
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`debug Checks card uses the existing gated demos on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    fake.liveMode = "usage"
    await startSession($)
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    expect(await debug.find({ key: "debug-checks-demo" })).toBeUndefined()
    await debug.press({ key: "usage-debug-mode" })
    await debug.press({ key: "debug-checks-demo" })
    const checks = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "checks",
      props: PANE_PROPS,
    })
    expect(await checks.find({ text: /Retry loop/ })).toBeDefined()
    await debug.press({ key: "debug-checks-live" })
    expect(await checks.find({ text: /Retry loop/ })).toBeUndefined()
    await debug.press({ key: "debug-checks-tour" })
    expect(await checks.find({ key: "tour-next" })).toBeDefined()
    await debug.press({ key: "usage-debug-mode" })
    expect(fake.panes).not.toContain("checks")
    expect(await debug.find({ key: "debug-checks-demo" })).toBeUndefined()
    expect(fake.submits).toEqual([])
  })
}

for (const surface of ["desktop", "terminal"] as const) {
  test(`layout comparison shares live and synthetic readings on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    fake.liveMode = "usage"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(80),
    })
    expect(await debug.find({ text: "Layout: Label reveal · inline (default)" })).toBeDefined()
    await debug.press({ key: "usage-preview-mid" })
    await debug.press({ key: "usage-layout-current" })
    expect(await debug.find({ text: "Layout: Current · captions above meters" })).toBeDefined()
    expect(await debug.find({ text: "5-hour: 48%" })).toBeDefined()
    if (surface === "desktop") {
      const svg = (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )!
      expect(svg.props.height).toBe(33.75)
    }
    await $.session.measure({
      context: { window: 200000 },
      rateLimits: [{ kind: "five_hour", percentUsed: 12 }],
      changed: ["rateLimits"],
    })
    await debug.press({ key: "usage-layout-inline" })
    expect(await debug.find({ text: "5-hour: 48%" })).toBeDefined()
    if (surface === "desktop") {
      const svg = (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )!
      expect(svg.props.height).toBe(18)
      expect(String(svg.props.source)).not.toContain("animation:meter-in")
      expect(String(svg.props.source)).not.toContain("animation:preview-fill")
    }
    await debug.press({ key: "usage-preview-live" })
    expect(await debug.find({ text: "5-hour: 12%" })).toBeDefined()
    await debug.press({ key: "usage-layout-current" })
    await $.session.start({ cwd: "/work", surface, isInteractive: true })
    expect(await debug.find({ text: "Layout: Label reveal · inline (default)" })).toBeDefined()
    expect(fake.presentationTimers).toEqual([])
    expect(fake.submits).toEqual([])
  })
}

test("terminal Label reveal exposes reset and pace separately without changing values", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [
    {
      kind: "five_hour",
      percentUsed: 48,
      resetsAt: new Date(fake.now + 3 * 60 * 60 * 1000).toISOString(),
    },
    {
      kind: "seven_day",
      percentUsed: 5,
      resetsAt: new Date(fake.now + (6 * 24 + 15 + 36 / 60) * 60 * 60 * 1000).toISOString(),
    },
  ]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  const paceControl = await band.find({ key: "usage-pace-five_hour" })
  expect(JSON.stringify(paceControl)).not.toContain("●")
  expect(JSON.stringify(paceControl)).toContain("│")
  await band.press({ key: "usage-detail-five_hour" })
  expect(await band.find({ text: /^RESETS IN / })).toBeDefined()
  await band.press({ key: "usage-pace-five_hour" })
  expect(await band.find({ text: /^Even pace:/ })).toBeDefined()
  expect(await band.find({ text: /^48%$/ })).toBeDefined()
  await band.press({ key: "usage-detail-seven_day" })
  expect(await band.find({ text: /^RESETS IN / })).toBeDefined()
  const resetTrack = await band.find({ key: "usage-reset-track" })
  const resetCopyTrack = await band.find({ key: "usage-reset-copy-track" })
  const resetCopy = await band.find({ text: /^RESETS IN / })
  const markerSlot = await band.find({ key: "usage-pace-marker-slot" })
  expect(resetTrack?.props.width).toBeGreaterThan(10)
  expect(resetCopyTrack?.props.width).toBe(resetTrack?.props.width)
  expect(resetCopyTrack?.props.left).toBe(0)
  expect(markerSlot?.props.left).toBe(1)
  expect(resetCopy?.text).toBe("RESETS IN 6D 15H")
  expect(resetCopy?.text.length).toBeLessThan(resetCopyTrack?.props.width as number)
  const resetTree = JSON.stringify(resetTrack)
  expect(resetTree.indexOf("usage-pace-marker-slot")).toBeLessThan(
    resetTree.indexOf("usage-reset-copy-track"),
  )
  await band.press({ key: "usage-pace-seven_day" })
  const weeklyPace = await band.find({ text: /^Even pace:/ })
  expect(weeklyPace).toBeDefined()
  expect(JSON.stringify(await band.find({ key: "usage-pace-right-seven_day" }))).toContain(
    "Even pace:",
  )
  expect(await band.find({ key: "usage-pace-seven_day" })).toBeDefined()
  await $.ui.focus({
    component: "AbovePrompt",
    requestId: "usage-test",
    plugin: "antiburn-live",
    element: "usage-pace-seven_day",
    origin: { kind: "person" },
  })
  expect(await band.find({ text: /^Even pace:/ })).toBeDefined()
  expect(fake.presentationTimers).toEqual([])
})

for (const elapsed of [0.2, 0.8]) {
  test(`terminal pace reveal preserves dots outside the copy at ${elapsed * 100}% elapsed`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "usage"
    fake.rateLimits = [
      {
        kind: "seven_day",
        percentUsed: 1,
        resetsAt: new Date(fake.now + (1 - elapsed) * 7 * 24 * 60 * 60 * 1000).toISOString(),
      },
    ]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(180),
    })
    const side = elapsed < 0.5 ? "right" : "left"
    const before = await band.find({ key: `usage-pace-${side}-seven_day` })
    await band.press({ key: "usage-pace-seven_day" })
    const after = await band.find({ key: `usage-pace-${side}-seven_day` })
    const tree = JSON.stringify(after)
    const copyIndex = tree.indexOf("Even pace:")
    expect(after?.props.width).toBe(before?.props.width)
    expect(copyIndex).toBeGreaterThan(-1)
    expect(tree).toContain("●")
    if (side === "right") {
      expect(tree.lastIndexOf("●")).toBeGreaterThan(copyIndex)
    } else {
      expect(tree.indexOf("●")).toBeLessThan(copyIndex)
      expect(tree.indexOf("●")).toBeGreaterThan(-1)
    }
    expect(await band.find({ text: /^1%$/ })).toBeDefined()
    expect(await band.find({ key: "usage-pace-seven_day" })).toBeDefined()
  })
}

if (BURN_ANIMATION_ENABLED) {
  test("rooted flame preview stays independent of usage examples and preserves meter sources", async ($, on) => {
    const fake = fakeEngine(on, ["desktop"])
    fake.liveMode = "usage"
    fake.rateLimits = [
      { kind: "five_hour", percentUsed: 8 },
      { kind: "seven_day", percentUsed: 1 },
    ]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "AbovePrompt",
      props: stripProps(),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    const before = (await band.findAll({ type: "Svg" })).find((node) =>
      String(node.props.alt).startsWith("5-hour:"),
    )!.props.source
    await debug.press({ key: "burn-preview" })
    const mark = (await band.findAll({ type: "Svg" })).find((node) =>
      String(node.props.alt).includes("output activity preview"),
    )!
    expect(mark.props.width).toBe(13)
    expect(mark.props.height).toBe((30 * 13) / 14)
    const slot = (await band.findAll({ type: "Svg" })).find(
      (node) => node.props.alt === "antiburn",
    )!
    expect(slot.props.height).toBe(13)
    expect(slot.props.isInteractive).toBe(false)
    const activitySlot = (await band.findAll({ type: "Box" })).find(
      (node) => node.props.bottom === 0 && node.props.left === 0 && node.props.width === 14 / 8,
    )
    expect(activitySlot?.props.alignItems).toBe("center")
    expect(String(slot.props.source)).not.toContain('<svg opacity="0"')
    expect(
      (await band.findAll({ type: "Svg" })).find((node) =>
        String(node.props.alt).startsWith("5-hour:"),
      )!.props.source,
    ).toBe(before)
    expect(await debug.find({ text: /Live usage data/ })).toBeDefined()
    const controlsAlt = "Usage reset and pace controls"
    const controls = (await band.findAll({ type: "Svg" })).find(
      (node) => node.props.alt === controlsAlt,
    )!
    expect(String(controls.props.source)).not.toContain("High output activity")
    expect(controls.props.width).toBe(stripProps().bodyColumns * 8 - 32)
    fake.now += 1000
    await debug.press({ key: "burn-stats" })
    expect(
      (await band.findAll({ type: "Svg" })).find((node) => node.props.alt === controlsAlt)!
        .props.source,
    ).toBe(controls.props.source)
    await debug.press({ key: "burn-preview-reduced" })
    const reduced = (await band.findAll({ type: "Svg" })).find((node) =>
      String(node.props.alt).includes("output activity preview"),
    )!
    expect(String(reduced.props.source)).toContain(".motion{display:none!important}")
    expect(String(reduced.props.source)).toContain('dur="15s"')
    await debug.press({ key: "burn-preview-stop" })
    expect(
      (await band.findAll({ type: "Svg" })).some((node) =>
        String(node.props.alt).includes("output activity preview"),
      ),
    ).toBe(false)
  })
}

if (BURN_ANIMATION_ENABLED) {
  test("clearing a session drops flame preview ownership", async ($, on) => {
    const fake = fakeEngine(on, ["desktop"])
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "AbovePrompt",
      props: stripProps(),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    await debug.press({ key: "burn-preview" })
    await $.classic.SessionStart({
      ...HOOK_BASE,
      hook_event_name: "SessionStart",
      source: "clear",
    } as never)
    expect(
      (await band.findAll({ type: "Svg" })).some((node) =>
        String(node.props.alt).includes("output activity preview"),
      ),
    ).toBe(false)
  })
}

test("output observer forwards every chunk and result once even when its clock fails", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  await startSession($)
  fake.streamChunks = [
    { kind: "text", index: 0, text: "hello", ref: 1 },
    { kind: "thinking", index: 1, text: "reasoning", ref: 2 },
    { kind: "input", index: 2, json: '{"path":"a"}', ref: 3 },
  ]
  fake.clockRead = async () => {
    throw new Error("clock unavailable")
  }
  const stream = $.turn.step({
    turnId: "observed",
    index: 0,
    model: fake.model,
    messageCount: 1,
  })
  const received: TurnStepChunk[] = []
  let item = await stream.next()
  while (!item.done) {
    received.push(item.value)
    item = await stream.next()
  }
  expect(received).toEqual(fake.streamChunks)
  expect(item.value.usage?.output_tokens).toBe(SMALL.output_tokens)
  expect(fake.streamStarts).toBe(1)
})

test("output observer preserves an upstream stream failure without restarting the request", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  fake.streamChunks = [{ kind: "text", index: 0, text: "partial", ref: 1 }]
  fake.streamFailure = true
  const stream = $.turn.step({
    turnId: "aborted",
    index: 0,
    model: fake.model,
    messageCount: 1,
  })
  const first = await stream.next()
  expect(first.value).toEqual(fake.streamChunks[0])
  let failed = false
  try {
    await stream.next()
  } catch {
    failed = true
  }
  expect(failed).toBe(true)
  expect(fake.streamStarts).toBe(1)
})

if (BURN_ANIMATION_ENABLED) {
  test("stream activity lights the mark, expires without output, and settles on completion", async ($, on) => {
    const fake = fakeEngine(on, ["desktop"])
    fake.liveMode = "usage"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "AbovePrompt",
      props: stripProps(),
    })
    fake.streamChunks = Array.from({ length: 5 }, (_, index) => ({
      kind: "text" as const,
      index,
      text: "a".repeat(800),
      ref: index + 1,
    }))
    const stream = $.turn.step({
      turnId: "fast",
      index: 0,
      model: fake.model,
      messageCount: 1,
    })
    for (let index = 0; index < 5; index += 1) {
      fake.now += 700
      expect((await stream.next()).done).toBe(false)
    }
    const hot = (await band.findAll({ type: "Svg" })).find((node) =>
      String(node.props.alt).includes("high recent output activity"),
    )
    expect(hot).toBeDefined()
    expect(String(hot!.props.source)).toContain('dur="8s"')
    fake.now += 9000
    await $.session.measure({
      context: { window: 200000 },
      rateLimits: fake.rateLimits,
      changed: ["rateLimits"],
    })
    expect(
      (await band.findAll({ type: "Svg" })).some((node) =>
        String(node.props.alt).includes("high recent output activity"),
      ),
    ).toBe(false)
    expect((await stream.next()).done).toBe(true)
    expect(fake.streamStarts).toBe(1)
    expect(fake.presentationTimers).toEqual([])
  })
}

if (BURN_ANIMATION_ENABLED) {
  test("a flame preview expires on the next render without changing usage", async ($, on) => {
    const fake = fakeEngine(on, ["desktop"])
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "AbovePrompt",
      props: stripProps(),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    await debug.press({ key: "burn-preview" })
    fake.now += 16000
    await $.session.measure({
      context: { window: 200000 },
      rateLimits: fake.rateLimits,
      changed: ["rateLimits"],
    })
    expect(
      (await band.findAll({ type: "Svg" })).some((node) =>
        String(node.props.alt).includes("output activity preview"),
      ),
    ).toBe(false)
  })
}

test("pending activity measurements do not delay streamed chunks", async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  fake.streamChunks = [{ kind: "text", index: 0, text: "a".repeat(800), ref: 1 }]
  let reads = 0
  let release!: () => void
  const measurement = new Promise<void>((resolve) => {
    release = resolve
  })
  fake.clockRead = async () => {
    reads += 1
    if (reads === 2) await measurement
    return fake.now
  }
  const stream = $.turn.step({
    turnId: "unblocked",
    index: 0,
    model: fake.model,
    messageCount: 1,
  })
  const first = await stream.next()
  expect(first.value).toEqual(fake.streamChunks[0])
  release()
  expect((await stream.next()).done).toBe(true)
  expect(fake.streamStarts).toBe(1)
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`finite usage sequence preserves quota animations and respects burn availability on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    await startSession($)
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect(fake.presentationTimers).toHaveLength(0)
    await debug.press({ key: "usage-sequence-start" })
    const advance = async () => {
      const timer = fake.presentationTimers.at(-1)!
      fake.now += timer.ms
      timer.fire()
      for (let count = 0; count < 100; count++) await Promise.resolve()
    }
    const reelIds = [
      "inactive",
      "low",
      "mid",
      "beyond-pace",
      "high",
      "near-limit",
      "empty",
      "error",
    ]
    for (let index = 0; index < reelIds.length; index++) {
      expect(await debug.find({ text: `Preview: ${reelIds[index]}` })).toBeDefined()
      expect(fake.presentationTimers).toHaveLength(index + 1)
      if (surface === "desktop" && index === 0) {
        const sources = await band.findAll({ type: "Svg" })
        expect(
          sources.some((node) => String(node.props.source).includes("animation:meter-in")),
        ).toBe(true)
      }
      await advance()
    }
    for (const [label, selector, duration] of [
      ["Reset reveal", ".reset", 280],
      ["Reset hide", ".reset", 180],
      ["Pace reveal", ".pace-detail", 280],
      ["Pace hide", ".pace-detail", 180],
    ] as const) {
      expect(await debug.find({ text: new RegExp(`Playing: .*${label}`) })).toBeDefined()
      if (surface === "desktop") {
        expect(
          (await band.findAll({ type: "Svg" })).some((node) =>
            String(node.props.source).includes(
              `${selector}{animation:debug-reset ${duration}ms`,
            ),
          ),
        ).toBe(true)
      }
      await advance()
    }
    if (BURN_ANIMATION_ENABLED) {
      expect(await debug.find({ text: /Playing: .*Ember catch/ })).toBeDefined()
      expect(await debug.find({ text: "Preview: high" })).toBeDefined()
      if (surface === "desktop")
        expect(
          (await band.findAll({ type: "Svg" })).some((node) =>
            String(node.props.alt).includes("output activity preview"),
          ),
        ).toBe(true)
      await advance()
    } else {
      expect(await debug.find({ text: /Playing: .*Ember catch/ })).toBeUndefined()
      expect(fake.flameTimers).toHaveLength(0)
      expect(fake.stateWrites.filter((key) => key === "burn")).toHaveLength(1)
      expect(await band.find({ key: "usage-mark" })).toBeUndefined()
    }
    expect(await debug.find({ key: "usage-sequence-stop" })).toBeUndefined()
    expect(fake.presentationTimers).toHaveLength(
      reelIds.length + (BURN_ANIMATION_ENABLED ? 5 : 4),
    )
    expect(fake.submits).toEqual([])
    expect(fake.usageReads).toBe(1)
  })
}

for (const action of ["stop", "manual", "live", "layout", "restart", "clear", "end"]) {
  test(`usage sequence cancels stale callbacks on ${action}`, async ($, on) => {
    const fake = fakeEngine(on, ["desktop"])
    await startSession($)
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    await debug.press({ key: "usage-sequence-start" })
    const stale = fake.presentationTimers[0]!
    if (action === "clear")
      await $.classic.SessionStart({ source: "clear", model: "claude-opus-5" })
    else if (action === "end")
      await $.session.end({
        reason: "clear",
        sessionId: "test-session",
        resume: { id: "test-session" },
      })
    else
      await debug.press({
        key: {
          stop: "usage-sequence-stop",
          manual: "usage-preview-mid",
          live: "usage-preview-live",
          layout: "usage-layout-current",
          restart: "usage-sequence-start",
        }[action]!,
      })
    const timerCount = fake.presentationTimers.length
    stale.fire()
    for (let count = 0; count < 100; count++) await Promise.resolve()
    expect(fake.presentationTimers).toHaveLength(timerCount)
    if (action !== "restart")
      expect(await debug.find({ key: "usage-sequence-stop" })).toBeUndefined()
    if (action === "manual") expect(await debug.find({ text: "Preview: mid" })).toBeDefined()
    if (action === "live") expect(await debug.find({ text: /Preview:/ })).toBeUndefined()
  })
}

test("usage sequence setup failure clears playback without scheduling work", async ($, on) => {
  const fake = fakeEngine(on, ["desktop"])
  await startSession($)
  const debug = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "debug",
    props: PANE_PROPS,
  })
  let fail = true
  fake.clockRead = async () => {
    if (fail) {
      fail = false
      throw new Error("clock unavailable")
    }
    return fake.now
  }
  await debug.press({ key: "usage-sequence-start" })
  expect(await debug.find({ key: "usage-sequence-stop" })).toBeUndefined()
  expect(fake.presentationTimers).toHaveLength(0)
})

if (BURN_ANIMATION_ENABLED) {
  test("desktop keeps its base mark through lease expiry without restarting the fire", async ($, on) => {
    const fake = fakeEngine(on, ["desktop"])
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "AbovePrompt",
      props: stripProps(),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    fake.now += 1000
    await debug.press({ key: "burn-preview" })
    const base = async () =>
      (await band.findAll({ type: "Svg" })).find((node) => node.props.alt === "antiburn")!
    const visible = String((await base()).props.source)
    expect(visible).not.toContain('<svg opacity="0"')
    fake.now += 15000
    await debug.press({ key: "burn-preview-stop" })
    expect(String((await base()).props.source)).toBe(visible)
    expect(String((await base()).props.source)).not.toContain('<svg opacity="0"')
    expect(
      (await band.findAll({ type: "Svg" })).some(
        (node) => node.props.height === (30 * 13) / 14 && node.props.width === 13,
      ),
    ).toBe(false)
    fake.now += 300
    await debug.press({ key: "burn-stats" })
    expect(String((await base()).props.source)).toBe(visible)
    expect(String((await base()).props.source)).not.toContain('<svg opacity="0"')
    expect(
      (await band.findAll({ type: "Svg" })).some(
        (node) => node.props.height === (30 * 13) / 14 && node.props.width === 13,
      ),
    ).toBe(false)
  })
}

for (const columns of [160, 60, 24]) {
  test(`desktop empty and error states retain usage geometry at ${columns} columns`, async ($, on) => {
    fakeEngine(on, ["desktop"])
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "AbovePrompt",
      props: stripProps(columns),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    await debug.press({ key: "usage-preview-empty" })
    const empty = await band.find({ key: "usage-band" })
    expect(empty!.props.height).toBe(18 / 16)
    const status = (await band.findAll({ type: "Svg" })).find(
      (node) => node.props.alt === "Usage not available yet. Try send a message first",
    )
    expect(status!.props.alt).toBe("Usage not available yet. Try send a message first")
    expect(String(status!.props.source)).toContain('class="hint"')
    if (columns === 24) {
      expect(String(status!.props.source)).not.toContain('<g class="ink">')
      expect(Number(status!.props.width)).toBeGreaterThan(150)
    }
    expect(String(status!.props.source)).toContain("bitcount-pixel")
    expect(String(status!.props.source)).not.toContain("<text")
    await debug.press({ key: "usage-preview-error" })
    expect(await band.find({ text: "Usage unavailable · Last known" })).toBeUndefined()
    expect(await debug.find({ text: "Last known" })).toBeDefined()
    expect(
      (await band.findAll({ type: "Svg" })).some((node) =>
        String(node.props.alt).includes("Usage unavailable · Last known"),
      ),
    ).toBe(true)
  })
}

if (BURN_ANIMATION_ENABLED) {
  test("an early stopped flame drops its cooling overlay on the next render after cooldown", async ($, on) => {
    const fake = fakeEngine(on, ["desktop"])
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "AbovePrompt",
      props: stripProps(),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "desktop",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    await debug.press({ key: "usage-preview-mid" })
    await debug.press({ key: "burn-preview" })
    fake.now += 1000
    await debug.press({ key: "burn-preview-stop" })
    const overlays = async () =>
      (await band.findAll({ type: "Svg" })).filter(
        (node) => node.props.height === (30 * 13) / 14 && node.props.width === 13,
      )
    expect((await overlays()).length).toBe(1)
    const meter = (await band.findAll({ type: "Svg" })).find((node) =>
      String(node.props.alt).startsWith("5-hour:"),
    )!
    expect(String(meter.props.source)).not.toContain("animation:preview-fill")
    expect(String(meter.props.source)).not.toContain('class="roll-old"')
    fake.now += 801
    await debug.press({ key: "burn-stats" })
    expect((await overlays()).length).toBe(0)
    const base = (await band.findAll({ type: "Svg" })).find(
      (node) => node.props.alt === "antiburn",
    )!
    expect(base.props.isInteractive).toBe(true)
  })
}

test("a reading that interrupts entrance settles the mark as well as the meters", async ($, on) => {
  const fake = fakeEngine(on, ["desktop"])
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(),
  })
  const base = async () =>
    (await band.findAll({ type: "Svg" })).find((node) => node.props.alt === "antiburn")!
  expect(String((await base()).props.source)).toContain("animation:mark-in")
  fake.now += 120
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 9 }],
    changed: ["rateLimits"],
  })
  expect(String((await base()).props.source)).not.toContain("animation:mark-in")
})

test("entrance replay is available on live data and repeated presses produce fresh animations", async ($, on) => {
  const fake = fakeEngine(on, ["desktop"])
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  const debug = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "debug",
    props: PANE_PROPS,
  })
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "AbovePrompt",
    props: stripProps(),
  })
  expect(await debug.find({ key: "usage-preview-entrance" })).toBeDefined()
  await debug.press({ key: "usage-preview-entrance" })
  const sources = async () =>
    (await band.findAll({ type: "Svg" }))
      .filter(
        (node) => node.props.alt === "antiburn" || String(node.props.alt).startsWith("5-hour:"),
      )
      .map((node) => String(node.props.source))
  const first = await sources()
  expect(first.some((source) => source.includes("animation:mark-in"))).toBe(true)
  expect(first.some((source) => source.includes("animation:meter-in"))).toBe(true)
  await debug.press({ key: "usage-preview-entrance" })
  const second = await sources()
  expect(second).not.toEqual(first)
  expect(second[0]).not.toBe(first[0])
  expect(second[1]).not.toBe(first[1])
  expect(await debug.find({ text: "Live usage data" })).toBeDefined()
  expect(await debug.find({ text: /Preview:/ })).toBeUndefined()
  expect(fake.presentationTimers).toEqual([])
})

test("entrance replay uses low usage when an empty example cannot enter", async ($, on) => {
  fakeEngine(on, ["desktop"])
  await startSession($)
  const debug = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "desktop",
    component: "Pane",
    requestId: "debug",
    props: PANE_PROPS,
  })
  await debug.press({ key: "usage-preview-entrance" })
  expect(await debug.find({ text: "Preview: low" })).toBeDefined()
})

for (const surface of ["desktop", "terminal"] as const) {
  test(`synthetic strip labels are optional and hidden by default on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    await startSession($)
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(),
    })
    await debug.press({ key: "usage-preview-mid" })
    expect(await band.find({ text: /Synthetic usage/ })).toBeUndefined()
    expect(await debug.find({ text: "Preview: mid" })).toBeDefined()
    expect(await debug.find({ text: /Synthetic data/ })).toBeDefined()
    await debug.press({ key: "usage-synthetic-labels" })
    expect(await band.find({ text: "Synthetic usage · mid" })).toBeDefined()
    expect(await debug.find({ key: "usage-synthetic-labels" })).toBeDefined()
    await debug.press({ key: "usage-synthetic-labels" })
    await debug.press({ key: "usage-sequence-start" })
    expect(await band.find({ text: /Synthetic usage/ })).toBeUndefined()
    expect(await debug.find({ key: "usage-sequence-stop" })).toBeDefined()
    expect(fake.presentationTimers).toHaveLength(1)
    await debug.press({ key: "usage-synthetic-labels" })
    expect(await band.find({ text: "Synthetic usage · inactive" })).toBeDefined()
    expect(await debug.find({ key: "usage-sequence-stop" })).toBeDefined()
    expect(fake.presentationTimers).toHaveLength(1)
    await debug.press({ key: "usage-sequence-stop" })
  })
}

for (const surface of ["desktop", "terminal"] as const) {
  test(`empty usage after clear recovers on the next native measurement on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    fake.liveMode = "usage"
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    fake.rateLimits = []
    await $.classic.SessionStart({
      ...HOOK_BASE,
      hook_event_name: "SessionStart",
      source: "clear",
    } as never)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    expect(await debug.find({ text: /Claude has not reported usage/ })).toBeDefined()
    if (surface === "desktop") {
      expect(
        (await band.findAll({ type: "Svg" })).some(
          (node) => node.props.alt === "Usage not available yet. Try send a message first",
        ),
      ).toBe(true)
    } else expect(await band.find({ text: "Usage not available yet" })).toBeDefined()
    await $.session.measure({
      context: { window: 200000 },
      rateLimits: [{ kind: "five_hour", percentUsed: 9 }],
      changed: ["rateLimits"],
    })
    if (surface === "desktop") {
      expect(
        (await band.findAll({ type: "Svg" })).some((node) =>
          String(node.props.alt).startsWith("5-hour: 9%."),
        ),
      ).toBe(true)
    } else expect(await band.find({ text: /^9%$/ })).toBeDefined()
    expect(await debug.find({ text: /Claude has not reported usage/ })).toBeUndefined()
    expect(fake.usageReads).toBe(2)
  })
}

test("terminal meters share one dot glyph and stacked limits have no blank row", async ($, on) => {
  const fake = fakeEngine(on, ["terminal"])
  fake.liveMode = "usage"
  fake.rateLimits = [
    { kind: "five_hour", percentUsed: 48 },
    { kind: "seven_day", percentUsed: 36 },
  ]
  await startSession($)
  for (const columns of [60, 79, 80, 81, 100]) {
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(columns),
    })
    expect(await band.find({ text: "○" })).toBeUndefined()
    const dots = await band.findAll({ text: /^●$/ })
    expect(dots.some((node) => node.props.color === "#FF5914")).toBe(true)
    expect(dots.some((node) => node.props.color === "subtle")).toBe(true)
    const meters = await band.find({ key: "usage-meters" })
    expect(meters!.props.gap).toBe(columns < 80 ? 0 : 3)
    expect(await band.findAll({ type: "Raster" })).toHaveLength(1)
    expect(await band.findAll({ type: "Image" })).toHaveLength(0)
    const raster = (await band.findAll({ type: "Raster" }))[0]!
    const link = (await band.findAll({ type: "Link" })).find(
      (node) => node.props.href === "https://antiburn.com",
    )
    expect(link).toBeUndefined()
    expect(raster.props.rows).toBe(1)
    expect(raster.props.columns).toBe(10)
    const slot = (await band.findAll({ type: "Box" })).find(
      (node) => node.props.width === 11 && node.props.height === 2,
    )
    expect(slot!.props.width).toBe(11)
    expect(slot!.props.paddingLeft).toBe(1)
    expect(slot!.props.height).toBe(2)
    expect(slot!.props.alignItems).toBe("center")
    expect(slot!.props.justifyContent).toBe("center")
    const windowColumns = columns < 80 ? columns - 12 : Math.floor((columns - 15) / 2)
    const expected = Math.floor((windowColumns - 6 - 3 - 2 + 1) / 2)
    const first = await band.find({ key: "usage-five_hour" })
    expect(first!.props.width).toBe(windowColumns)
    expect(dots).toHaveLength(expected * 2)
  }
})

if (BURN_ANIMATION_ENABLED) {
  for (const outcome of ["accepted", "denied"] as const) {
    test(`terminal block flame paints and stops after ${outcome} blits`, async ($, on) => {
      const fake = fakeEngine(on)
      fake.liveMode = "usage"
      fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
      const frames: string[] = []
      on("ui.blit", (_$, e) => {
        if ("cells" in e) frames.push(e.cells)
        return { value: outcome === "denied" ? { deny: "not mounted" } : {} }
      })
      await startSession($)
      const band = await $.ui.mount({
        plugin: "antiburn-live",
        surface: "terminal",
        component: "AbovePrompt",
        props: stripProps(100),
      })
      const debug = await $.ui.mount({
        plugin: "antiburn-live",
        surface: "terminal",
        component: "Pane",
        requestId: "debug",
        props: PANE_PROPS,
      })
      await band.drawn()
      expect(fake.flameTimers).toHaveLength(0)
      await debug.press({ key: "burn-preview" })
      await band.drawn()
      expect({ phase: "scheduled", length: fake.flameTimers.length }).toEqual({
        phase: "scheduled",
        length: 1,
      })
      const fire = async (advance: number) => {
        fake.now += advance
        fake.flameTimers.at(-1)!.fire()
        for (let i = 0; i < 200; i += 1) await Promise.resolve()
      }
      await fire(1000)
      expect({ phase: "painted", length: frames.length }).toEqual({
        phase: "painted",
        length: 1,
      })
      expect(fake.flameTimers).toHaveLength(outcome === "accepted" ? 2 : 1)
      if (outcome === "accepted") {
        await fire(100)
        expect(frames).toHaveLength(2)
        expect(frames[1]).not.toBe(frames[0])
        await fire(16000)
        const count = frames.length
        const timers = fake.flameTimers.length
        await fire(100)
        expect(frames).toHaveLength(count)
        expect(fake.flameTimers).toHaveLength(timers)
      }
      expect(await band.findAll({ type: "Raster" })).toHaveLength(1)
      expect(fake.submits).toEqual([])
    })
  }
}

if (BURN_ANIMATION_ENABLED) {
  for (const action of ["hidden", "clear", "reduced"] as const) {
    test(`terminal block flame handles ${action} without stale animation`, async ($, on) => {
      const fake = fakeEngine(on)
      fake.liveMode = "usage"
      fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
      let frames = 0
      on("ui.blit", () => {
        frames += 1
        return { value: {} }
      })
      await startSession($)
      const band = await $.ui.mount({
        plugin: "antiburn-live",
        surface: "terminal",
        component: "AbovePrompt",
        requestId: "flame-band",
        props: stripProps(100),
      })
      await band.drawn()
      const debug = await $.ui.mount({
        plugin: "antiburn-live",
        surface: "terminal",
        component: "Pane",
        requestId: "debug",
        props: PANE_PROPS,
      })
      await debug.press({
        key: action === "reduced" ? "burn-preview-reduced" : "burn-preview",
      })
      await band.drawn()
      const timer = fake.flameTimers.at(-1)!
      expect(timer.ms).toBe(action === "reduced" ? 15000 : 100)
      if (action === "hidden") {
        await band.unmount()
        const hidden = await $.ui.mount({
          plugin: "antiburn-live",
          surface: "terminal",
          component: "AbovePrompt",
          requestId: "flame-band",
          props: { ...stripProps(100), maxRows: 0 },
        })
        await hidden.drawn()
      } else if (action === "clear")
        await $.classic.SessionStart({
          ...HOOK_BASE,
          hook_event_name: "SessionStart",
          source: "clear",
        } as never)
      fake.now += timer.ms
      timer.fire()
      for (let i = 0; i < 200; i += 1) await Promise.resolve()
      expect(frames).toBe(action === "reduced" ? 1 : 0)
      expect(fake.flameTimers).toHaveLength(1)
    })
  }
}

for (const fixture of ["high", "near-limit"] as const) {
  test(`terminal ${fixture} pulses only red dots and settles without restarting`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "usage"
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(120),
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    await debug.press({ key: `usage-preview-${fixture}` })
    await band.drawn()
    expect(fake.pulseTimers).toHaveLength(1)
    const marker = (await band.findAll({ type: "Text" })).find((node) => node.text === "│")
    expect(marker?.props.color).toBe("#FF5914")
    expect(marker?.props.dimColor).toBe(false)
    expect(await band.findAll({ text: "•" })).toHaveLength(0)
    const fire = async (advance: number) => {
      fake.now += advance
      fake.pulseTimers.at(-1)!.fire()
      for (let i = 0; i < 200; i += 1) await Promise.resolve()
      await band.drawn()
    }
    await fire(800)
    const rings = (await band.findAll({ type: "Text" })).filter(
      (node) => node.text === "•" && node.props.color !== undefined,
    )
    expect(rings.length).toBeGreaterThan(0)
    expect([...new Set(rings.map((node) => node.props.color))]).toEqual(["#ed4545"])
    expect((await band.findAll({ text: "●" })).length).toBeGreaterThan(0)
    await fire(800)
    expect(await band.findAll({ text: "•" })).toHaveLength(0)
    await fire(8000)
    const timers = fake.pulseTimers.length
    expect(await band.findAll({ text: "•" })).toHaveLength(0)
    await band.drawn()
    expect(fake.pulseTimers).toHaveLength(timers)
    await debug.press({ key: "usage-preview-mid" })
    await band.drawn()
    expect(await band.findAll({ text: "•" })).toHaveLength(0)
    await debug.press({ key: `usage-preview-${fixture}` })
    await band.drawn()
    const pending = fake.pulseTimers.at(-1)!
    await debug.press({ key: "usage-preview-empty" })
    await band.drawn()
    const stopped = fake.pulseTimers.length
    fake.now += 800
    pending.fire()
    for (let i = 0; i < 200; i += 1) await Promise.resolve()
    expect(fake.pulseTimers).toHaveLength(stopped)
    await debug.press({ key: `usage-preview-${fixture}` })
    await band.drawn()
    if (BURN_ANIMATION_ENABLED) {
      const motion = fake.pulseTimers.at(-1)!
      await debug.press({ key: "burn-preview-reduced" })
      await band.drawn()
      const reduced = fake.pulseTimers.length
      fake.now += 800
      motion.fire()
      for (let i = 0; i < 200; i += 1) await Promise.resolve()
      expect(fake.pulseTimers).toHaveLength(reduced)
      expect(await band.findAll({ text: "•" })).toHaveLength(0)
    } else {
      expect(await debug.find({ key: "burn-preview-reduced" })).toBeUndefined()
      expect(fake.flameTimers).toHaveLength(0)
    }
  })
}

for (const surface of ["desktop", "terminal"] as const) {
  test(`the idle usage brand links to the website on ${surface} with and without readings`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "usage"
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(120),
    })
    for (const readings of [[], [{ kind: "five_hour", percentUsed: 8 }]]) {
      await $.session.measure({
        context: { window: 200000 },
        rateLimits: readings,
        changed: ["rateLimits"],
      })
      const links = (await band.findAll({ type: "Link" })).filter(
        (node) => node.props.href === "https://antiburn.com",
      )
      expect(links).toHaveLength(surface === "desktop" ? 1 : 0)
      expect(JSON.stringify(links[0])).not.toContain('"type":"Svg"')
      expect(JSON.stringify(links[0])).not.toContain('"type":"Raster"')
      if (surface === "terminal")
        expect(await band.find({ key: "usage-brand-open" })).toBeDefined()
      else expect(links[0]?.props.label).toBe("Open antiburn website")
    }
  })
}

if (BURN_ANIMATION_ENABLED) {
  for (const surface of ["desktop", "terminal"] as const) {
    test(`brand links preserve burn rendering on ${surface}`, async ($, on) => {
      const fake = fakeEngine(on)
      fake.liveMode = "usage"
      fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
      await startSession($)
      const band = await $.ui.mount({
        plugin: "antiburn-live",
        surface,
        component: "AbovePrompt",
        props: stripProps(120),
      })
      const debug = await $.ui.mount({
        plugin: "antiburn-live",
        surface,
        component: "Pane",
        requestId: "debug",
        props: PANE_PROPS,
      })
      await band.drawn()
      await debug.press({ key: "burn-preview" })
      await band.drawn()
      const links = (await band.findAll({ type: "Link" })).filter(
        (node) => node.props.href === "https://antiburn.com",
      )
      expect(links).toHaveLength(surface === "desktop" ? 1 : 0)
      if (surface === "desktop") {
        const graphics = await band.findAll({ type: "Svg" })
        expect(
          graphics.some((node) => node.props.alt === "Usage reset and pace controls"),
        ).toBe(true)
        expect(
          graphics.some((node) => String(node.props.alt).includes("output activity preview")),
        ).toBe(true)
      } else expect((await band.findAll({ type: "Raster" }))[0]?.props.rows).toBe(1)
      await debug.press({ key: "burn-preview-stop" })
      fake.now += 1000
      await band.drawn()
      expect(
        (await band.findAll({ type: "Link" })).filter(
          (node) => node.props.href === "https://antiburn.com",
        ),
      ).toHaveLength(surface === "desktop" ? 1 : 0)
    })
  }
}

test("terminal logo has no hint and keeps its two-row flame slot", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  const debug = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "Pane",
    requestId: "debug",
    props: PANE_PROPS,
  })
  for (const burning of BURN_ANIMATION_ENABLED ? [false, true] : [false]) {
    if (burning) await debug.press({ key: "burn-preview" })
    const tree = JSON.stringify(await band.drawn())
    expect(tree).not.toContain("Click/Enter")
    expect(tree).not.toContain("usage-brand-link")
    expect((await band.find({ key: "usage-brand-text-slot" }))?.props.height).toBe(2)
    expect(await band.find({ key: "usage-brand-open" })).toBeDefined()
  }
})

test("terminal brand hover and reset labels have separate bounded regions", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [
    { kind: "five_hour", percentUsed: 8 },
    { kind: "seven_day", percentUsed: 1 },
  ]
  await startSession($)
  for (const columns of [60, 120]) {
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(columns),
    })
    const boxes = await band.findAll({ type: "Box" })
    const target = boxes.find(
      (node) =>
        node.props.width === 10 &&
        node.props.height === 1 &&
        node.props.overflow === "hidden" &&
        node.text.includes("antiburn"),
    )
    expect(target?.props.width).toBe(10)
    expect(target?.props.height).toBe(1)
    expect(target?.props.overflow).toBe("hidden")
    expect(target?.text).toContain("antiburn")
    expect(target?.text).not.toContain("5-hour")
    for (const label of ["5-hour", "Weekly"]) {
      const bounds = boxes.find(
        (node) =>
          node.props.width === label.length &&
          node.props.overflow === "hidden" &&
          node.text === label,
      )
      expect(bounds?.props.height).toBe(1)
      expect(bounds?.props.flexShrink).toBe(0)
    }
    await band.press({ key: "usage-detail-five_hour" })
    expect(await band.find({ type: "Text", text: "Reset unavailable" })).toBeDefined()
    const link = (await band.findAll({ type: "Link" })).find(
      (node) => node.props.href === "https://antiburn.com",
    )
    expect(link).toBeUndefined()
    await band.press({ key: "usage-detail-five_hour" })
  }
})

test("entering terminal band focus does not reveal or dismiss usage details", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [
    {
      kind: "five_hour",
      percentUsed: 12,
      resetsAt: new Date(fake.now + 3 * 60 * 60 * 1000).toISOString(),
    },
  ]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  await $.ui.focus({
    component: "AbovePrompt",
    requestId: "focus-test",
    plugin: "antiburn-live",
    element: "usage-detail-five_hour",
    origin: { kind: "person" },
  })
  expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
  await band.press({ key: "usage-detail-five_hour" })
  expect(await band.find({ text: /^RESETS IN / })).toBeDefined()
  await $.ui.focus({
    component: "AbovePrompt",
    requestId: "focus-test",
    plugin: "antiburn-live",
    element: "usage-pace-seven_day",
    origin: { kind: "person" },
  })
  expect(await band.find({ text: /^RESETS IN / })).toBeDefined()
  await band.press({ key: "usage-detail-five_hour" })
  expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
})

test("terminal brand activation opens only the fixed website and leaves reset details alone", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [{ kind: "five_hour", percentUsed: 12 }]
  const commands: string[][] = []
  on("process.run", (_$, e) => {
    commands.push([...e.argv])
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
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  expect(commands.filter((argv) => argv[0] !== "node")).toEqual([])
  await band.press({ key: "usage-brand-open" })
  expect(commands.filter((argv) => argv[0] !== "node")).toEqual([
    ["open", "https://antiburn.com"],
  ])
  expect(await band.find({ text: /^Reset unavailable$/ })).toBeUndefined()
  expect(fake.submits).toEqual([])
})

test("terminal website activation tries the Linux opener after a nonzero open result", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  const commands: string[][] = []
  on("process.run", (_$, e) => {
    commands.push([...e.argv])
    return {
      value: {
        exitCode: e.argv[0] === "open" ? 1 : 0,
        stdout: "",
        stderr: "",
        isStdoutTruncated: false,
        isStderrTruncated: false,
      },
    }
  })
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  await band.press({ key: "usage-brand-open" })
  expect(commands.filter((argv) => argv[0] !== "node")).toEqual([
    ["open", "https://antiburn.com"],
    ["xdg-open", "https://antiburn.com"],
  ])
  expect(fake.statuses).not.toContain("Open https://antiburn.com in your browser")
})

for (const columns of [60, 120]) {
  test(`terminal hover reveals are bounded and transient at ${columns} columns`, async ($, on) => {
    const fake = fakeEngine(on)
    fake.liveMode = "usage"
    fake.rateLimits = [
      {
        kind: "five_hour",
        percentUsed: 48,
        resetsAt: new Date(fake.now + 3 * 3600000).toISOString(),
      },
      {
        kind: "seven_day",
        percentUsed: 82,
        resetsAt: new Date(fake.now + 3 * 86400000).toISOString(),
      },
    ]
    await startSession($)
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface: "terminal",
      component: "AbovePrompt",
      props: stripProps(columns),
    })
    const reset = "usage-hover-reset-seven_day"
    const marker = "usage-hover-pace-seven_day"
    expect((await band.find({ key: reset }))?.props.width).toBe(6)
    expect((await band.find({ key: marker }))?.props.width).toBe(1)
    expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
    await band.pointer({ in: reset, type: "move", x: 2, y: 0 })
    expect(await band.find({ text: /^RESETS IN 3D/ })).toBeDefined()
    expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
    await band.pointer({ in: reset, type: "leave", x: 2, y: 0 })
    expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
    await band.press({ key: "usage-detail-seven_day" })
    await band.pointer({ in: marker, type: "move", x: 0, y: 0 })
    expect(await band.find({ text: /^Even pace:/ })).toBeDefined()
    expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
    await band.pointer({ in: marker, type: "move", x: 1, y: 0 })
    expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
    expect(await band.find({ text: /^RESETS IN 3D/ })).toBeDefined()
    await band.pointer({ in: marker, type: "move", x: -1, y: 0 })
    expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
    await band.pointer({ in: reset, type: "move", x: 0, y: 1 })
    expect(await band.find({ text: /^RESETS IN 3D/ })).toBeDefined()
    await band.press({ key: "usage-detail-seven_day" })
    expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
    expect(fake.submits).toEqual([])
  })
}

test("terminal hover target keeps click and Enter activation as fallbacks", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  fake.rateLimits = [
    {
      kind: "five_hour",
      percentUsed: 12,
      resetsAt: new Date(fake.now + 3 * 3600000).toISOString(),
    },
  ]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  const target = "usage-hover-reset-five_hour"
  await band.pointer({ in: target, type: "down", x: 0, y: 0, button: "left" })
  await band.pointer({ in: target, type: "up", x: 0, y: 0, button: "left" })
  await band.pointer({ in: target, type: "leave", x: 0, y: 0 })
  expect(await band.find({ text: /^RESETS IN / })).toBeDefined()
  await band.key({ in: target, key: "return" })
  expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
  await band.pointer({ in: target, type: "down", x: 0, y: 0, button: "left" })
  await band.pointer({ in: target, type: "up", x: 6, y: 0, button: "left" })
  expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
})

test("terminal hover is cleared when a pace target or the usage strip disappears", async ($, on) => {
  const fake = fakeEngine(on)
  fake.liveMode = "usage"
  const reading = {
    kind: "five_hour",
    percentUsed: 12,
    resetsAt: new Date(fake.now + 3 * 3600000).toISOString(),
  }
  fake.rateLimits = [reading]
  await startSession($)
  const band = await $.ui.mount({
    plugin: "antiburn-live",
    surface: "terminal",
    component: "AbovePrompt",
    props: stripProps(120),
  })
  const beforeExpiry = fake.now
  await band.pointer({
    in: "usage-hover-pace-five_hour",
    type: "move",
    x: 0,
    y: 0,
  })
  fake.now += 3 * 3600000 + 1
  await band.redraw()
  expect(await band.find({ key: "usage-hover-pace-five_hour" })).toBeUndefined()
  fake.now = beforeExpiry
  await band.redraw()
  expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
  const oldProps = (await band.find({ key: "usage-hover-pace-five_hour" }))!.props.props as {
    binding: string
  }
  await band.pointer({
    in: "usage-hover-pace-five_hour",
    type: "move",
    x: 0,
    y: 0,
  })
  expect(await band.find({ text: /^Even pace:/ })).toBeDefined()
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [{ kind: "five_hour", percentUsed: 12 }],
    changed: ["rateLimits"],
  })
  expect(await band.find({ text: /^Pace unavailable$/ })).toBeUndefined()
  expect(await band.find({ key: "usage-hover-pace-five_hour" })).toBeUndefined()
  await $.session.measure({
    context: { window: 200000 },
    rateLimits: [reading],
    changed: ["rateLimits"],
  })
  expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
  await band.post(
    { binding: oldProps.binding, hovered: true, activate: false },
    { in: "usage-hover-pace-five_hour" },
  )
  expect(await band.find({ text: /^Even pace:/ })).toBeUndefined()
  await band.pointer({
    in: "usage-hover-reset-five_hour",
    type: "move",
    x: 0,
    y: 0,
  })
  expect(await band.find({ text: /^RESETS IN / })).toBeDefined()
  await band.redraw({ ...stripProps(120), hasSurvey: true })
  await band.redraw(stripProps(120))
  expect(await band.find({ text: /^RESETS IN / })).toBeUndefined()
})

for (const surface of ["desktop", "terminal"] as const) {
  for (const mode of ["usage", "checks"] as const) {
    test(`disabled burn forwards streaming without activity work in ${mode} on ${surface}`, async ($, on) => {
      const fake = fakeEngine(on, [surface])
      fake.liveMode = mode
      fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
      fake.usage = DEEP
      await startSession($)
      if (mode === "checks") await showCheckSamples($)
      const band = await $.ui.mount({
        plugin: "antiburn-live",
        surface,
        component: "AbovePrompt",
        props: stripProps(),
      })
      await band.drawn()
      let clockReads = 0
      fake.clockRead = async () => {
        clockReads++
        return fake.now
      }
      const burnWrites = fake.stateWrites.filter((key) => key === "burn").length
      fake.streamChunks = [
        { kind: "text", index: 0, text: "x".repeat(4000), ref: 1 },
        { kind: "thinking", index: 1, text: "y".repeat(4000), ref: 2 },
        { kind: "input", index: 2, json: '{"path":"a"}', ref: 3 },
      ]
      const stream = $.turn.step({
        turnId: "t",
        index: 0,
        model: fake.model,
        effort: "high",
        messageCount: 1,
      })
      const chunks: TurnStepChunk[] = []
      let item = await stream.next()
      while (!item.done) {
        chunks.push(item.value)
        fake.now += 1000
        item = await stream.next()
      }
      expect(chunks).toEqual(fake.streamChunks)
      expect(item.value.usage).toEqual({ ...DEEP, model: fake.model })
      expect(fake.streamStarts).toBe(1)
      expect(clockReads).toBe(0)
      expect(fake.stateWrites.filter((key) => key === "burn")).toHaveLength(burnWrites)
      await $.turn.complete({
        answer: "",
        durationMs: 1,
        isAborted: false,
        turnId: "t",
        reason: "answer",
      })
      expect(fake.flameTimers).toHaveLength(0)
      expect(await band.find({ key: "usage-mark" })).toBeUndefined()
      expect(await band.find({ key: "burn-controls" })).toBeUndefined()
      expect(await band.find({ key: "usage-band" })).toBeDefined()
      if (mode === "checks") {
        expect((await band.find({ key: "fix" }))?.props.label).toBe(
          surface === "desktop" ? "Compact now" : "1 Compact now",
        )
        await band.press({ key: "fix" })
        expect(fake.compacts).toHaveLength(1)
      } else expect(await band.find({ key: "fix" })).toBeUndefined()
    })
  }

  test(`disabled burn previews stay unavailable through debug and check demos on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on, [surface])
    fake.rateLimits = [{ kind: "five_hour", percentUsed: 8 }]
    await startSession($)
    await $.command.run({
      command: "antiburn-dev",
      args: "debug",
      origin: { kind: "composer" },
      presentation: { isFullscreen: false, columns: 120 },
    })
    const debug = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "Pane",
      requestId: "debug",
      props: PANE_PROPS,
    })
    const band = await $.ui.mount({
      plugin: "antiburn-live",
      surface,
      component: "AbovePrompt",
      props: stripProps(),
    })
    expect(
      await debug.find({ text: "Burn previews and activity tracking are unavailable." }),
    ).toBeDefined()
    for (const key of [
      "burn-preview",
      "burn-preview-reduced",
      "burn-preview-stop",
      "burn-stats",
    ])
      expect(await debug.find({ key })).toBeUndefined()
    const burnWrites = fake.stateWrites.filter((key) => key === "burn").length
    await debug.press({ key: "debug-checks-demo" })
    expect(await band.find({ key: "fix" })).toBeDefined()
    await debug.press({ key: "debug-checks-live" })
    await $.command.run({
      command: "antiburn-dev",
      args: "demo",
      origin: { kind: "composer" },
      presentation: { isFullscreen: false, columns: 120 },
    })
    expect(await band.find({ key: "fix" })).toBeDefined()
    await $.command.run({
      command: "antiburn-dev",
      args: "demo off",
      origin: { kind: "composer" },
      presentation: { isFullscreen: false, columns: 120 },
    })
    await band.drawn()
    expect(fake.stateWrites.filter((key) => key === "burn")).toHaveLength(burnWrites)
    expect(fake.flameTimers).toHaveLength(0)
    expect(await band.find({ key: "usage-mark" })).toBeUndefined()
    expect(await band.find({ key: "burn-controls" })).toBeUndefined()
    expect(await band.find({ key: "usage-band" })).toBeDefined()
  })
}
