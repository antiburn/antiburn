import { expect, test } from 'claude-code/testing'
import type { Engine } from 'claude-code/testing'
import type { On, RenderElement, RenderSurface } from 'claude-code'
import { commandSummary, promptPreview } from './register'

const SMALL = { input_tokens: 10, output_tokens: 5, cache_read_input_tokens: 20_000, cache_creation_input_tokens: 0 }
const DEEP = { ...SMALL, cache_read_input_tokens: 450_000 }

type Fake = {
  submits: string[]
  fills: string[]
  compacts: (string | undefined)[]
  opens: string[]
  focusedOpens: boolean[]
  panes: string[]
  statuses: (string | undefined)[]
  writes: string[]
  model: string
  usage: typeof SMALL
}

function fakeEngine(on: On, surfaces: readonly RenderSurface[] = ['terminal']): Fake {
  const fake: Fake = {
    submits: [],
    fills: [],
    compacts: [],
    opens: [],
    focusedOpens: [],
    panes: [],
    statuses: [],
    writes: [],
    model: 'claude-opus-5',
    usage: SMALL,
  }
  on('session.start', () => ({ cwd: '/work' }))
  on('classic.UserPromptSubmit', () => ({}) as never)
  on('classic.Stop', () => ({}) as never)
  on('session.surfaces', () => ({ value: surfaces }))
  on('command.register', (_$, e) => ({ value: { command: e.name } }))
  on('ui.log', () => ({ value: undefined }))
  on('ui.status', (_$, e) => {
    fake.statuses.push(e.text)
    return { value: undefined }
  })
  on('ui.open', (_$, e) => {
    fake.opens.push(e.id)
    fake.focusedOpens.push(e.focus === true)
    if (!fake.panes.includes(e.id)) fake.panes.push(e.id)
    return { value: { isPlaced: true } }
  })
  on('ui.panes', () => ({
    value: fake.panes.map(id => ({ id, title: 'antiburn', isShown: true, isFocused: false, isPlaced: true })),
  }))
  on('ui.close', (_$, e) => {
    fake.panes = fake.panes.filter(id => id !== e.id)
    return { value: undefined }
  })
  on('fs.write', (_$, e) => {
    fake.writes.push(e.path)
    return { value: undefined }
  })
  on('prompt.submit', (_$, e) => {
    fake.submits.push(e.text)
    return { text: e.text }
  })
  on('prompt.fill', (_$, e) => {
    fake.fills.push(e.text)
    return { isFilled: true }
  })
  on('session.compact', (_$, e) => {
    fake.compacts.push(e.instructions)
    return { messages: [{ role: 'user' as const, text: 'Summary', toolUses: [] }] }
  })
  on('tool.call', () => ({ isError: true, result: 'exit 1' }))
  on('turn.complete', () => ({ text: '' }))
  // Stands for the engine's own drawing when a mod hook yields with next(e).
  on('ui.render', (t$, e) => {
    const { Box } = t$.ui.resolve(e)
    return h(Box, {}) as RenderElement
  })
  on('turn.step', async function* (_$, e) {
    return {
      turnId: e.turnId,
      index: e.index,
      answer: '',
      toolUses: [],
      stopReason: 'end_turn' as const,
      usage: { ...fake.usage, model: e.model },
    }
  })
  return fake
}

async function startSession($: Engine, mode = 'default') {
  await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
  await $.classic.UserPromptSubmit({ ...HOOK_BASE, hook_event_name: 'UserPromptSubmit', prompt: 'hi', permission_mode: mode } as never)
}

const HOOK_BASE = { session_id: 's', transcript_path: '/t', cwd: '/work' }

async function turn($: Engine, fake: Fake, options: { failures?: number } = {}) {
  // The kit runs a streaming event only while the test reads its generator.
  const stream = $.turn.step({ turnId: 't', index: 0, model: fake.model, effort: 'high', messageCount: 1 })
  let chunk = await stream.next()
  while (chunk.done !== true) chunk = await stream.next()
  for (let i = 0; i < (options.failures ?? 0); i += 1) {
    // The model labels each retry differently, as in a live session.
    await $.tool.call({ tool: 'Bash', command: 'npm test', description: `Run the tests (try ${i + 1})` } as never)
  }
  await $.turn.complete({ answer: '', durationMs: 1, isAborted: false, turnId: 't', reason: 'answer' })
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
  title: 'antiburn',
  isFocused: true,
  bodyColumns: 48,
  placement: 'inline' as const,
  scroll: { offset: 0, bodyRows: 24 },
  view: {},
}

for (const surface of ['terminal', 'desktop'] as const) {
  test(`a retry loop raises the strip and the chip, and the fix sends one prompt on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await turn($, fake, { failures: 3 })

    const band = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'AbovePrompt', props: stripProps() })
    expect((await band.drawn()) !== null).toBe(true)
    expect((await band.find({ text: /Retry loop · 3 tries · Bash/ })) !== undefined).toBe(true)
    // The desktop prompt sends a digit to Claude, so only the terminal strip takes hotkeys.
    expect((await band.find({ key: 'fix' }))?.props.hotkey).toBe(surface === 'terminal' ? '1' : undefined)
    expect((await band.find({ key: 'view' }))?.props.hotkey).toBe(surface === 'terminal' ? '2' : undefined)
    const chip = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'SessionMode', props: { modes: [] } })
    // The Code tab footer drops an Svg, so the state mark is tinted text.
    expect((await chip.find({ key: 'chip' }))?.props.label).toBe('antiburn · 1')
    expect((await chip.find({ text: /^●$/ }))?.props.color).toBe('warning')
    expect(await chip.find({ type: 'Svg' })).toBe(undefined)

    await band.press({ key: 'fix' })
    expect(fake.submits.length).toBe(1)
    expect(fake.submits[0]).toMatch(/^Stop retrying the call that keeps failing\./)
    expect((await chip.find({ key: 'chip' }))?.props.label).toBe('antiburn · 1')
    expect((await chip.find({ text: /^⧗$/ }))?.props.color).toBe('warning')

    await turn($, fake)
    expect((await chip.find({ key: 'chip' }))?.props.label).toBe('antiburn')
    expect((await chip.find({ text: /^✓$/ }))?.props.color).toBe('#108EAC')
    // The resolved strip names the check in the pass cyan, as text and not in a box.
    if (surface === 'desktop') {
      const svgs = await band.findAll({ type: 'Svg' })
      expect(svgs.map(node => node.props.alt)).toEqual(['antiburn', 'Resolved', 'Retry loop resolved'])
      expect(String(svgs[2]?.props.source)).toMatch(/class="p"/)
    } else {
      const outcome = await band.find({ text: /^✓ Retry loop resolved$/ })
      expect(outcome?.props.color).toBe('#2DA7C6')
      expect(outcome?.props.backgroundColor).toBe(undefined)
    }
    await turn($, fake)
    expect(await band.find({ key: 'fix' })).toBe(undefined)
    expect(fake.writes).toEqual([])
  })

  test(`the chip and View open the checks pane on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await turn($, fake, { failures: 3 })
    const band = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'AbovePrompt', props: stripProps() })
    await band.press({ key: 'view' })
    const chip = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'SessionMode', props: { modes: [] } })
    await chip.press({ key: 'chip' })
    // The new check opened the pane without the keys; each press opens it with them.
    expect(fake.opens).toEqual(['checks', 'checks', 'checks'])
    expect(fake.focusedOpens).toEqual([false, true, true])
  })

  test(`the pane lists a check and opens its detail on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await turn($, fake, { failures: 3 })
    const view = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'Pane', requestId: 'checks', props: PANE_PROPS })
    const isTerminal = surface === 'terminal'
    // The desktop puts the count in a badge and names the group, as the app does.
    expect((await view.find({ text: isTerminal ? /^● Checks 1$/ : /^Checks$/ })) !== undefined).toBe(true)
    // Both surfaces give the first sentence of the check as its reason.
    expect((await view.find({ text: /^The same tool call failed several times in one turn\.$/ })) !== undefined).toBe(true)
    // The footer gives no key hint: the chevron shows how a card opens.
    expect(await view.find({ text: /→ more/ })).toBe(undefined)
    // Both surfaces name the app in the link text.
    expect((await view.find({ text: /^antiburn app( ↗)?$/ })) !== undefined).toBe(true)
    if (isTerminal) expect((await view.find({ key: 'row-X1' }))?.props.label).toBe('▸')
    await view.press({ key: 'row-X1' })
    expect((await view.find({ text: /Stop retrying the call/ })) !== undefined).toBe(true)
    // Both surfaces open the card in place: the list stays, and no back row shows.
    expect(await view.find({ key: 'back' })).toBe(undefined)
    if (isTerminal) expect((await view.find({ key: 'row-X1' }))?.props.label).toBe('▾')
    await view.press({ key: 'hide-X1' })
    // A check showed this session, so the pane does not say "No checks yet".
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) !== undefined).toBe(true)
  })
}

for (const mode of ['auto', 'bypassPermissions']) {
  test(`${mode} mode needs a second press before the fix is sent`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($, mode)
    await turn($, fake, { failures: 3 })
    const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps() })
    await band.press({ key: 'fix' })
    expect(fake.submits).toEqual([])
    expect(fake.opens).toEqual(['checks', 'checks'])
    await band.press({ key: 'fix' })
    expect(fake.submits.length).toBe(1)
  })
}


test('an armed desktop card opens on its prompt, takes the primary style, and a watched metric goes neutral', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, 'auto')
  await turn($, fake, { failures: 3 })
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await view.find({ key: 'card-fix-X1' }))?.props.variant).toBe(undefined)
  expect((await view.findAll({ type: 'Svg' })).some(node => node.props.alt === '3 tries ·')).toBe(true)
  expect((await view.find({ text: /^Stop retrying the call/ })) === undefined).toBe(true)
  await view.press({ key: 'card-fix-X1' })
  expect(fake.submits).toEqual([])
  const armed = await view.find({ key: 'card-fix-X1' })
  expect(armed?.props.label).toBe('Confirm send')
  expect(armed?.props.variant).toBe('primary')
  expect((await view.find({ text: /Stop retrying the call/ })) !== undefined).toBe(true)
  await view.press({ key: 'card-fix-X1' })
  expect(fake.submits.length).toBe(1)
  // The watched card keeps its metric as neutral text, without the failure Svg.
  expect((await view.find({ text: /^3 tries · Bash$/ }))?.props.color).toBe('subtle')
  expect((await view.findAll({ type: 'Svg' })).some(node => node.props.alt === '3 tries ·')).toBe(false)
})

// The Svg of a chevron: the first child of the chevron Box. The press target lies on top of it.
async function chevronSvg(drawing: { find: (query: { key: string }) => Promise<unknown> }, key: string) {
  const box = (await drawing.find({ key: `chevron-${key}` })) as { children?: { props: Record<string, unknown> }[] } | undefined
  return box?.children?.[0]?.props
}

test('a switch to auto mode at turn end needs a second press', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  await $.classic.Stop({ ...HOOK_BASE, hook_event_name: 'Stop', stop_hook_active: false, permission_mode: 'auto' } as never)
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps() })
  await band.press({ key: 'fix' })
  expect(fake.submits).toEqual([])
})

test('Context heavy compacts on press and the compaction resolves it', async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await turn($, fake)
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps() })
  expect((await band.find({ key: 'fix' }))?.props.label).toBe('1 Compact now')
  await band.press({ key: 'fix' })
  expect(fake.compacts.length).toBe(1)
  expect(fake.compacts[0]).toMatch(/^Keep the current task/)
  expect((await band.find({ text: /✓ Context heavy resolved/ })) !== undefined).toBe(true)
})

test('an armed compact card opens on its instructions and takes the primary style', async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await turn($, fake)
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await view.find({ text: /^Keep the current task/ })) === undefined).toBe(true)
  await view.press({ key: 'card-fix-D' })
  expect(fake.compacts).toEqual([])
  expect((await view.find({ key: 'card-fix-D' }))?.props.variant).toBe('primary')
  expect((await view.find({ text: /Keep the current task/ })) !== undefined).toBe(true)
})

test('Obsolete model fills the /model command for the user to send', async ($, on) => {
  const fake = fakeEngine(on)
  fake.model = 'claude-opus-4-8'
  await startSession($)
  await turn($, fake)
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps() })
  await band.press({ key: 'fix' })
  expect(fake.fills).toEqual(['/model claude-opus-5'])
  expect(fake.submits).toEqual([])
})

test('the strip picks a shorter form at narrow widths', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const wide = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(120) })
  expect((await wide.find({ text: /^antiburn  1 to fix  Retry loop · 3 tries · Bash$/ })) !== undefined).toBe(true)
  const medium = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(90) })
  expect((await medium.find({ text: /3 tries/ })) === undefined).toBe(true)
  // The fix button acts on the top check, so the shorter forms keep its name.
  expect((await medium.find({ text: /antiburn/ })) !== undefined).toBe(true)
  expect((await medium.find({ text: / 1 to fix  Retry loop/ })) !== undefined).toBe(true)
  const narrow = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(70) })
  expect((await narrow.find({ text: /antiburn/ })) !== undefined).toBe(true)
  expect((await narrow.find({ text: /Retry loop/ })) !== undefined).toBe(true)
  expect((await narrow.find({ key: 'view' }))?.props.label).toBe('2 View')
  // A narrow desktop strip shows the mark and the check name, without the metric.
  const narrowDesktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'AbovePrompt', props: stripProps(54) })
  // The close glyph is an Svg too, because the desktop draws no native close control for the strip.
  expect((await narrowDesktop.findAll({ type: 'Svg' })).map(node => node.props.alt)).toEqual(['antiburn', '1 to fix', 'Dismiss'])
  expect((await narrowDesktop.find({ text: /Retry loop/ })) !== undefined).toBe(true)
  expect((await narrowDesktop.find({ text: /3 tries/ })) === undefined).toBe(true)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'AbovePrompt', props: stripProps(120) })
  expect((await desktop.find({ text: /Retry loop · 3 tries · Bash/ })) !== undefined).toBe(true)
  const head = (await desktop.find({ key: 'brand-head' })) as { children?: { type?: string }[] } | undefined
  // The wordmark and the count pill are SVG, so they follow the theme. The pill carries the state, so no dot.
  expect((head?.children ?? []).filter(child => child.type === 'Svg').length).toBe(2)
  expect((await desktop.find({ text: /●/ })) === undefined).toBe(true)
  // The terminal band boxes the count on a brand tint in the failure text of the check language and draws the name in the brand.
  const count = (await wide.find({ text: /^ 1 to fix $/ })) as { props?: { color?: string; backgroundColor?: string } } | undefined
  const name = (await wide.find({ text: /^antiburn$/ })) as { props?: { color?: string } } | undefined
  expect(count?.props?.backgroundColor).toBe('#4D2F23')
  expect(count?.props?.color).toBe('#FF8D5C')
  // The desktop pill is a tint like the terminal box. It reads the theme and changes the fill and the ink, in monospace.
  const pill = (await desktop.findAll({ type: 'Svg' })).find(node => node.props.alt === '1 to fix')
  const pillSource = String(pill?.props.source)
  expect(pillSource).toMatch(/\.r\{fill:#F2DCCD\}\.i\{fill:#9A3412\}/)
  expect(pillSource).toMatch(/prefers-color-scheme:dark\)\{\.r\{fill:#51372A\}\.i\{fill:#FF8D5C\}/)
  expect(pillSource).toMatch(/monospace/)
  const wordmark = (await desktop.findAll({ type: 'Svg' })).find(node => node.props.key === 'brand' || node.props.alt === 'antiburn')
  expect(String(wordmark?.props.source)).not.toMatch(/prefers-color-scheme/)
  expect(name?.props?.color).toBe('#FF5914')
})

test('the strip drops the metric below 100 columns and stacks the buttons below 56', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  for (const surface of ['terminal', 'desktop'] as const) {
    const medium = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'AbovePrompt', props: stripProps(90) })
    expect((await medium.find({ text: /3 tries/ })) === undefined).toBe(true)
    expect(await medium.find({ key: 'actions' })).toBe(undefined)
    // Below 56 columns the buttons take a second row, and the first row keeps the check name.
    const stacked = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'AbovePrompt', props: stripProps(44) })
    expect((await stacked.find({ key: 'actions' })) !== undefined).toBe(true)
    expect((await stacked.find({ text: /Retry loop/ })) !== undefined).toBe(true)
    expect((await stacked.find({ key: 'fix' })) !== undefined).toBe(true)
  }
  // The terminal name and count still fit on the first row at 44 columns.
  const stacked = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(44) })
  expect((await stacked.find({ text: /^antiburn  1 to fix  Retry loop$/ })) !== undefined).toBe(true)
})

test('the strip yields while a turn runs', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(120, true) })
  expect(await band.find({ key: 'fix' })).toBe(undefined)
})

test('dismiss hides the strip, and a new kind brings it back with a new count', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps() })
  // The dismiss control is a dim plain ×, as the host's own close control looks.
  expect((await band.find({ key: 'dismiss' }))?.props.plain).toBe(true)
  expect((await band.find({ text: '×' })) !== undefined).toBe(true)
  await band.press({ key: 'dismiss' })
  expect(await band.find({ key: 'fix' })).toBe(undefined)
  expect(await band.find({ key: 'view' })).toBe(undefined)
  expect(await band.find({ text: /to fix/ })).toBe(undefined)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'AbovePrompt', props: stripProps() })
  expect(await desktop.findAll({ type: 'Svg' })).toEqual([])
  // The same check in a later turn does not bring the strip back.
  await turn($, fake, { failures: 3 })
  expect(await band.find({ key: 'fix' })).toBe(undefined)
  fake.usage = DEEP
  await turn($, fake, { failures: 3 })
  expect((await band.find({ text: /\(1 new\)/ })) !== undefined).toBe(true)
  // "(1 new)" stays at 80 to 99 columns, where the metric has gone. The terminal keeps it while it fits.
  for (const surface of ['terminal', 'desktop'] as const) {
    const medium = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'AbovePrompt', props: stripProps(surface === 'terminal' ? 99 : 90) })
    const text = surface === 'terminal' ? await medium.find({ text: /2 to fix \(1 new\)/ }) : (await medium.findAll({ type: 'Svg' })).find(node => node.props.alt === '2 to fix (1 new)')
    expect(text !== undefined).toBe(true)
  }
})

test('/antiburn opens the pane and opens the pane', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const presentation = { isFullscreen: false, columns: 100 }
  const opened = await $.command.run({ command: 'antiburn', args: '', presentation } as never)
  expect(opened.text).toBe('Opened the antiburn checks pane.')
  expect(fake.opens).toEqual(['checks', 'checks'])
})

test('/antiburn demo shows all five checks and keeps them until demo off', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
  const pane = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  for (const label of ['Retry loop', 'Reading spiral', 'Context heavy', 'Overthinking']) {
    expect((await pane.find({ text: new RegExp(label) })) !== undefined).toBe(true)
  }
  // The resolved check is in the folded Resolved group.
  await pane.press({ key: 'group-resolved' })
  expect((await pane.find({ text: /Obsolete model/ })) !== undefined).toBe(true)
  await turn($, fake)
  expect((await pane.find({ text: /Reading spiral/ })) !== undefined).toBe(true)
  await $.command.run({ command: 'antiburn', args: 'demo off', presentation } as never)
  expect((await pane.find({ text: /^No checks yet$/ })) !== undefined).toBe(true)
})

test('VS Code gets the status-line text instead of a strip', async ($, on) => {
  const fake = fakeEngine(on, ['vscode'])
  await startSession($)
  await turn($, fake, { failures: 3 })
  expect(fake.statuses.at(-1)).toBe('antiburn · 1 ●')
})

test('the terminal gets no status-line text', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  expect(fake.statuses).toEqual([])
})

test('the desktop strip draws its own button chrome, so labels have no digit', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'AbovePrompt', props: stripProps() })
  expect((await band.find({ key: 'fix' }))?.props.label).toBe('Ask Claude to fix')
  expect((await band.find({ key: 'view' }))?.props.label).toBe('View checks')
})

test('resolved checks fold into a group that opens on press', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  await turn($, fake)
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await view.find({ text: /✓ Resolved 1 ▸/ })) !== undefined).toBe(true)
  expect(await view.find({ key: 'row-X1' })).toBe(undefined)
  await view.press({ key: 'group-resolved' })
  expect((await view.find({ key: 'row-X1' })) !== undefined).toBe(true)
  // A resolved outcome takes the cyan of the Resolved icon, not the theme green.
  const outcome = (await view.findAll({ type: 'Text' })).find(node => node.props.color === '#108EAC')
  expect(outcome !== undefined).toBe(true)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  // The group state is shared, so the press above also opened the desktop group.
  const metric = (await desktop.findAll({ type: 'Svg' })).find(node => String(node.props.source).includes('class="p"'))
  expect(String(metric?.props.source)).toMatch(/\.p\{fill:#108EAC\}.*#2DA7C6.*font-weight="400"/)
  // The card shows the Resolved heading's CircleCheck, and the outcome text has no ✓ glyph.
  const svgs = await desktop.findAll({ type: 'Svg' })
  expect(svgs.some(node => node.props.alt === 'Resolved' && String(node.props.source).includes('m9 12 2 2 4-4'))).toBe(true)
  expect(svgs.some(node => String(node.props.alt).startsWith('✓'))).toBe(false)
})

test('the desktop mark lies behind an empty pane and under the cards otherwise', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  await turn($, fake)
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  // Only a folded Resolved section shows, so the pane has room for the mark.
  expect((await view.find({ key: 'watermark' })) !== undefined).toBe(true)
  await view.press({ key: 'group-chevron-resolved' })
  expect((await view.find({ key: 'card-X1' })) !== undefined).toBe(true)
  expect(await view.find({ key: 'watermark' })).toBe(undefined)
  expect((await view.find({ key: 'watermark-space' })) !== undefined).toBe(true)
})

test('an open card shows the check as resolved and drops its actions', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  await view.press({ key: 'row-X1' })
  expect((await view.find({ key: 'card-fix-X1' })) !== undefined).toBe(true)
  await turn($, fake)
  // The resolved card moves into the folded Resolved section, as on the desktop.
  await view.press({ key: 'group-resolved' })
  // The metric line names the outcome, so no second line repeats it.
  expect((await view.find({ text: /^✓ (Fixed|Stopped by itself)$/ })) !== undefined).toBe(true)
  expect(await view.find({ text: /resolved this session|The fix stopped it|It stopped by itself\./ })).toBe(undefined)
  expect(await view.find({ key: 'card-fix-X1' })).toBe(undefined)
  expect(await view.find({ text: /Fix prompt/ })).toBe(undefined)
})

test('a watched check offers no second send in the strip or the detail', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps() })
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  await view.press({ key: 'row-X1' })
  await view.press({ key: 'card-fix-X1' })
  expect(fake.submits.length).toBe(1)
  expect(await view.find({ key: 'card-fix-X1' })).toBe(undefined)
  expect((await view.find({ text: /Awaiting verification/ })) !== undefined).toBe(true)
  // Nothing is left to fix, so the strip goes away and the chip shows the watched check.
  expect(await band.find({ key: 'fix' })).toBe(undefined)
  expect((await band.find({ text: /antiburn/ })) === undefined).toBe(true)
  const chip = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'SessionMode', props: { modes: [] } } as never)
  expect((await chip.find({ key: 'chip' }))?.props.label).toBe('antiburn · 1')
  expect((await chip.find({ text: /^⧗$/ })) !== undefined).toBe(true)
})

test('the terminal chevron opens one card in place and closes it again', async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  await view.press({ key: 'row-D' })
  expect((await view.find({ key: 'card-fix-D' })) !== undefined).toBe(true)
  // A second card opens in place of the first, so only one card holds the f and h keys.
  await view.press({ key: 'row-T' })
  expect(await view.find({ key: 'card-fix-D' })).toBe(undefined)
  expect((await view.find({ key: 'card-fix-T' }))?.props.hotkey).toBe('f')
  expect((await view.find({ key: 'row-D' })) !== undefined).toBe(true)
  await view.press({ key: 'row-T' })
  expect(await view.find({ key: 'card-fix-T' })).toBe(undefined)
  expect((await view.find({ key: 'row-T' }))?.props.label).toBe('▸')
})

for (const surface of ['terminal', 'desktop'] as const) {
  test(`a docked pane on ${surface} keeps the footer at the bottom edge`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    await $.command.run({ command: 'antiburn', args: 'demo', presentation: { isFullscreen: false, columns: 100 } } as never)
    const props = { ...PANE_PROPS, placement: 'dock' as const }
    const view = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'Pane', requestId: 'checks', props })
    const root = await view.find({ type: 'Box' })
    // The terminal takes the rows as a minimum, so Ink does not shrink an open card.
    // The desktop takes them as a fixed height, so the footer stays in view.
    if (surface === 'terminal') {
      expect(root?.props.minHeight).toBe(24)
      expect(root?.props.height).toBe(undefined)
    } else {
      expect(root?.props.height).toBe(24)
      expect(root?.props.minHeight).toBe(undefined)
    }
  })
}

test('the chip counts only the checks left to fix, as the strip does', async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({ command: 'antiburn', args: 'demo states', presentation } as never)
  const chip = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'SessionMode', props: { modes: [] } } as never)
  expect((await chip.find({ key: 'chip' }))?.props.label).toBe('antiburn · 5')
  // The terminal draws the state mark after the label, as the Code tab footer does.
  const text = (await chip.find({ type: 'Box' }))?.text ?? ''
  expect(text.indexOf('antiburn · 5')).toBeGreaterThan(-1)
  expect(text.indexOf('antiburn · 5')).toBeLessThan(text.indexOf('●'))
})

test('the first row takes the focus when the pane opens', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await view.find({ key: 'row-X1' }))?.props.autoFocus).toBe(true)
})

test('the desktop pane draws each check as a card with an icon tile; the terminal does not', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await desktop.find({ key: 'card-X1' })) !== undefined).toBe(true)
  // A narrow card drops the area line; a wide card keeps it.
  expect(await desktop.find({ text: /^Tool calls$/ })).toBe(undefined)
  // The pane shows this session only, so the heading names no period.
  expect(await desktop.find({ text: /^this session$/ })).toBe(undefined)
  // A card fill is translucent, so the mark does not draw behind a card. It takes the free rows under the cards.
  expect(await desktop.find({ key: 'watermark' })).toBe(undefined)
  expect((await desktop.find({ key: 'watermark-space' })) !== undefined).toBe(true)
  const terminal = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect(await terminal.find({ key: 'card-X1' })).toBe(undefined)
  expect((await terminal.find({ key: 'row-X1' })) !== undefined).toBe(true)
})

for (const surface of ['terminal', 'desktop'] as const) {
  test(`the pane footer links to the antiburn app on ${surface}`, async ($, on) => {
    const fake = fakeEngine(on)
    await startSession($)
    await turn($, fake, { failures: 3 })
    const view = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'Pane', requestId: 'checks', props: PANE_PROPS })
    // The terminal puts the arrow after the Link, so it follows the URL that a terminal without OSC 8 prints.
    const name = (await view.find({ text: surface === 'terminal' ? /^antiburn app$/ : /^antiburn app ↗$/ })) as { props?: { color?: string } } | undefined
    expect(name?.props?.color).toBe('#FF5914')
    if (surface === 'terminal') {
      // The arrow is dim, as the terminal draws the URL it follows.
      const arrow = (await view.find({ text: /^ ↗$/ })) as { props?: { color?: string; dimColor?: boolean } } | undefined
      expect(arrow?.props?.dimColor).toBe(true)
      expect(arrow?.props?.color).toBe(undefined)
    }
    expect(await view.find({ text: /Keep your session in check/ })).toBe(undefined)
    expect(await view.find({ text: /\+r\d+/ })).toBe(undefined)
    expect((await view.find({ text: /Get more from every session with / })) !== undefined).toBe(true)
    expect(await view.find({ text: /not findings/ })).toBe(undefined)
    expect(await view.find({ text: /Across sessions · / })).toBe(undefined)
  })
}

test('only the desktop pane draws the antiburn watermark', async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await desktop.find({ key: 'watermark' })) !== undefined).toBe(true)
  const terminal = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect(await terminal.find({ key: 'watermark' })).toBe(undefined)
  // Only the desktop draws the small brand mark beside the footer link.
  const linkRow = async (drawing: typeof desktop) =>
    ((await drawing.find({ key: 'app-link' })) as { children?: { type?: string }[] } | undefined)?.children ?? []
  expect((await linkRow(desktop)).some(child => child.type === 'Svg')).toBe(true)
  expect((await linkRow(terminal)).some(child => child.type === 'Svg')).toBe(false)
})

test('a card offers its fix and a Show more toggle, and compacts only on a second press', async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await turn($, fake)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await desktop.find({ key: 'card-fix-D' }))?.props.label).toBe('Compact now')
  expect((await desktop.find({ key: 'card-fix-D' }))?.props.hotkey).toBe(undefined)
  // A closed card gives the first sentence as its reason. A chevron with no label opens it.
  expect((await desktop.find({ text: /^Each request now carries a very large context\.$/ })) !== undefined).toBe(true)
  expect(await desktop.find({ text: /^Show (more|less)$/ })).toBe(undefined)
  expect((await desktop.find({ key: 'chevron-row-D' })) !== undefined).toBe(true)
  // The chevron's alt names the action, as the button has no label.
  expect((await chevronSvg(desktop, 'row-D'))?.alt).toBe('Show more')
  await desktop.press({ key: 'row-D' })
  // The open card gives the whole reason in one paragraph.
  expect((await desktop.find({ text: /^Each request now carries a very large context\. Every step pays for all of it again\.$/ })) !== undefined).toBe(true)
  // The open card gives the compact instructions but no Evidence block: the metric sits above.
  expect((await desktop.find({ text: /^Compact instructions$/ })) !== undefined).toBe(true)
  expect((await desktop.find({ text: /^Evidence$/ })) === undefined).toBe(true)
  expect((await desktop.find({ key: 'card-fix-D' })) !== undefined).toBe(true)
  await desktop.press({ key: 'row-D' })
  expect((await desktop.find({ text: /^Compact instructions$/ })) === undefined).toBe(true)
  await desktop.press({ key: 'card-fix-D' })
  expect(fake.compacts.length).toBe(0)
  // The armed button asks for the second press itself.
  expect((await desktop.find({ text: /^Confirm compact$/ })) !== undefined).toBe(true)
  expect((await desktop.find({ text: /Press again/ })) === undefined).toBe(true)
  // Another press cancels the armed fix.
  await desktop.press({ key: 'row-D' })
  expect((await desktop.find({ text: /^Confirm compact$/ })) === undefined).toBe(true)
  await desktop.press({ key: 'row-D' })
  await desktop.press({ key: 'card-fix-D' })
  expect(fake.compacts.length).toBe(0)
  await desktop.press({ key: 'card-fix-D' })
  expect(fake.compacts.length).toBe(1)
})

test('every pane width uses one card layout, with no area line', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  for (const bodyColumns of [80, 40, 30]) {
    const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: { ...PANE_PROPS, bodyColumns } })
    expect((await view.find({ key: 'card-fix-X1' })) !== undefined).toBe(true)
    expect((await view.find({ text: /^Tool calls$/ })) === undefined).toBe(true)
    await view.unmount()
  }
})

test('the list leaves out building checks and folds resolved checks into one section', async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  // The demo has active, watched and resolved checks. The pane never shows a building check.
  await $.command.run({ command: 'antiburn', args: 'demo states', presentation } as never)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await desktop.find({ text: /Obsolete model/ })) === undefined).toBe(true)
  expect((await desktop.find({ text: /Building evidence/ })) === undefined).toBe(true)
  expect((await desktop.find({ key: 'card-X1' })) !== undefined).toBe(true)
  expect((await desktop.find({ key: 'card-T' })) !== undefined).toBe(true)
  expect((await desktop.find({ key: 'card-O' })) === undefined).toBe(true)
  // The chevron opens the section too.
  await desktop.press({ key: 'group-chevron-resolved' })
  expect((await desktop.find({ key: 'card-O' })) !== undefined).toBe(true)
  expect(String((await chevronSvg(desktop, 'group-chevron-resolved'))?.source)).toMatch(/animation:turn/)
  // The desktop title is Text, not a button, so it lines up with the Checks title.
  expect(await desktop.find({ type: 'Button', key: 'group-resolved' })).toBe(undefined)
  await desktop.press({ key: 'group-chevron-resolved' })
  expect((await desktop.find({ key: 'card-O' })) === undefined).toBe(true)
})

test('a watched check moves to its own section, and the section icon draws in once', async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await turn($, fake, { failures: 3 })
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await desktop.find({ text: /^Awaiting verification$/ })) === undefined).toBe(true)
  await desktop.press({ key: 'card-fix-X1' })
  expect(fake.submits.length).toBe(1)
  // The watched card leaves the Checks section and shows no action.
  expect((await desktop.find({ text: /^Awaiting verification$/ })) !== undefined).toBe(true)
  expect((await desktop.find({ text: /^Checking the next turns\./ })) !== undefined).toBe(true)
  expect(await desktop.find({ key: 'card-fix-X1' })).toBe(undefined)
  const icon = (await desktop.findAll({ type: 'Svg' })).find(node => node.props.alt === 'Awaiting verification')
  expect(String(icon?.props.source)).toMatch(/animation:enter/)
  const terminal = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await terminal.find({ text: /⧗ Awaiting verification 1/ })) !== undefined).toBe(true)
  // A press in the pane draws the pane again, so the icon stops its draw-in and does not flicker.
  await desktop.press({ key: 'row-D' })
  const settled = (await desktop.findAll({ type: 'Svg' })).find(node => node.props.alt === 'Awaiting verification')
  expect(String(settled?.props.source)).not.toMatch(/animation/)
})

test('an open card puts its actions last, with Hide beside the fix', async ($, on) => {
  const fake = fakeEngine(on)
  fake.usage = DEEP
  await startSession($)
  await turn($, fake)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect(await desktop.find({ key: 'hide-D' })).toBe(undefined)
  await desktop.press({ key: 'row-D' })
  // The card's text runs in drawing order: the details, then the action row.
  const text = (await desktop.find({ key: 'card-D' }))?.text ?? ''
  expect(text.indexOf('Compact instructions')).toBeGreaterThan(-1)
  expect(text.indexOf('Compact instructions')).toBeLessThan(text.indexOf('Compact now'))
  expect(text.indexOf('Compact now')).toBeLessThan(text.indexOf('Ignore this session'))
  expect((await desktop.find({ key: 'hide-D' })) !== undefined).toBe(true)
})

for (const surface of ['desktop', 'terminal'] as const) {
  test(`an empty pane says what it watches on ${surface}`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    const view = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'Pane', requestId: 'checks', props: PANE_PROPS })
    expect((await view.find({ text: /^No checks yet$/ })) !== undefined).toBe(true)
    // Each word is its own Text, so a wrapped line stays centred.
    const words = (await view.findAll({ type: 'Text' })).map(node => node.text.trim())
    expect(words.join(' ')).toMatch(/antiburn is watching this session\./)
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) === undefined).toBe(true)
  })

  test(`a pane with no active check says there is nothing to fix on ${surface}`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    const presentation = { isFullscreen: false, columns: 100 }
    await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
    const view = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'Pane', requestId: 'checks', props: PANE_PROPS })
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) === undefined).toBe(true)
    // A demo press acts as a real fix does and moves the card to Awaiting verification.
    for (const id of ['D', 'D', 'T', 'E1', 'K1', 'K2']) {
      if (surface === 'desktop') {
        await view.press({ key: `card-fix-${id}` })
      } else {
        if ((await view.find({ key: `row-${id}` })) === undefined) continue
        if ((await view.find({ key: `card-fix-${id}` })) === undefined) await view.press({ key: `row-${id}` })
        // Compact needs a second press. After the first press of any other fix, the button is gone.
        while ((await view.find({ key: `card-fix-${id}` })) !== undefined) await view.press({ key: `card-fix-${id}` })
      }
    }
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) !== undefined).toBe(true)
  })

  test(`a pane with every check ignored says there is nothing to fix, with no active mark, on ${surface}`, async ($, on) => {
    fakeEngine(on)
    await startSession($)
    const presentation = { isFullscreen: false, columns: 100 }
    await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
    const view = await $.ui.mount({ plugin: 'antiburn-live', surface, component: 'Pane', requestId: 'checks', props: PANE_PROPS })
    for (const id of ['X1', 'D', 'T', 'O', 'E1', 'K1', 'C1', 'K2']) {
      if ((await view.find({ key: `row-${id}` })) === undefined) continue
      // A card shows Ignore only while it is open, on both surfaces.
      if ((await view.find({ key: `hide-${id}` })) === undefined) await view.press({ key: `row-${id}` })
      if ((await view.find({ key: `hide-${id}` })) !== undefined) await view.press({ key: `hide-${id}` })
    }
    expect(await view.find({ text: /^No checks yet$/ })).toBe(undefined)
    expect((await view.find({ text: /^Nothing to fix right now\.$/ })) !== undefined).toBe(true)
    expect(await view.find({ text: /^● $/ })).toBe(undefined)
    expect(await view.find({ key: 'checks-icon' })).toBe(undefined)
  })
}

test('the strip leads with an active check over a watched one', async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 150 }
  await $.command.run({ command: 'antiburn', args: 'demo states', presentation } as never)
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(150) })
  // Retry loop comes first in catalog order, but it is watched, so Context heavy leads.
  expect((await band.find({ key: 'fix' }))?.props.label).toBe('1 Compact now')
  expect((await band.find({ text: / to fix  Context heavy · / })) !== undefined).toBe(true)
})

test('the open Context heavy card names file reads as the cause', async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect(await desktop.find({ text: /File reads are about/ })).toBe(undefined)
  await desktop.press({ key: 'row-D' })
  expect((await desktop.find({ text: /File reads are about 61% of it/ })) !== undefined).toBe(true)
})

test('the open terminal card follows the desktop card: Cause under the reason, no Evidence, keys on the buttons', async ($, on) => {
  fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
  const terminal = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  // A skill name in a metric draws in bold, in place of the desktop pill.
  const skill = (await terminal.findAll({ type: 'Text' })).find(node => node.props.bold === true && /^\/\S+$/.test(node.text ?? ''))
  expect(skill !== undefined).toBe(true)
  await terminal.press({ key: 'row-D' })
  expect(await terminal.find({ text: /^Evidence$/ })).toBe(undefined)
  expect(await terminal.find({ text: /this session so far/ })).toBe(undefined)
  expect((await terminal.find({ text: /^Cause · File reads are about 61% of it/ })) !== undefined).toBe(true)
  expect((await terminal.find({ key: 'card-fix-D' }))?.props.label).toBe('f Compact now')
  expect((await terminal.find({ key: 'hide-D' }))?.props.label).toBe('h Ignore this session')
})

test('a demo press acts as a real fix does, and the card moves to Awaiting verification', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, 'default')
  const presentation = { isFullscreen: false, columns: 100 }
  await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  await desktop.press({ key: 'card-fix-E1' })
  await desktop.press({ key: 'card-fix-T' })
  await desktop.press({ key: 'card-fix-D' })
  await desktop.press({ key: 'card-fix-D' })
  expect(fake.submits.length).toBe(1)
  expect(fake.fills).toEqual(['/effort high'])
  expect(fake.compacts.length).toBe(1)
  expect(await desktop.find({ key: 'card-fix-E1' })).toBe(undefined)
  expect(await desktop.find({ text: /Demo: nothing was sent/ })).toBe(undefined)
})

test('a terminal card compacts only on a second press, and a group press disarms it', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, 'default')
  const presentation = { isFullscreen: false, columns: 150 }
  await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
  const view = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  await view.press({ key: 'row-D' })
  await view.press({ key: 'card-fix-D' })
  expect(fake.compacts).toEqual([])
  expect((await view.find({ key: 'card-fix-D' }))?.props.label).toBe('f Confirm compact')
  // A press on the Resolved heading is a press in the pane, so it disarms the fix, as on the desktop.
  await view.press({ key: 'group-resolved' })
  expect((await view.find({ key: 'card-fix-D' }))?.props.label).toBe('f Compact now')
  await view.press({ key: 'card-fix-D' })
  await view.press({ key: 'card-fix-D' })
  expect(fake.compacts.length).toBe(1)
})

test('the terminal strip in the demo compacts as a real fix does', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, 'default')
  const presentation = { isFullscreen: false, columns: 150 }
  await $.command.run({ command: 'antiburn', args: 'demo', presentation } as never)
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(150) })
  await band.press({ key: 'fix' })
  await band.press({ key: 'fix' })
  expect(fake.compacts.length).toBe(1)
})

test('K1 reads the installed skills after a long turn and advises the skill for next time', async ($, on) => {
  const fake = fakeEngine(on)
  let lists = 0
  on('command.list', () => {
    lists += 1
    return { value: [
      { name: 'review-and-draft-pr', description: 'Review the change and open a draft pull request.', source: 'plugin' },
      { name: 'antiburn', description: 'antiburn checks', source: 'plugin' },
    ] }
  })
  await startSession($)
  await $.classic.UserPromptSubmit({ ...HOOK_BASE, hook_event_name: 'UserPromptSubmit', prompt: 'review and draft pr for this', permission_mode: 'default', source: 'user' } as never)
  for (let index = 0; index < 12; index += 1) {
    const stream = $.turn.step({ turnId: 't', index, model: fake.model, effort: 'high', messageCount: 1 })
    let chunk = await stream.next()
    while (chunk.done !== true) chunk = await stream.next()
  }
  await $.turn.complete({ answer: '', durationMs: 1, isAborted: false, turnId: 't', reason: 'answer' })
  expect(lists).toBe(1)
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  // The skill name draws as a quiet monospace badge: a rounded rectangle on a light tint, beside the metric.
  const row = (await desktop.find({ key: 'skill-metric-K1' })) as { children?: { props: Record<string, unknown> }[] } | undefined
  expect(String(row?.children?.[0]?.props.source)).toMatch(/rx="4".*fill-opacity.*monospace.*font-weight="500"/)
  expect(row?.children?.[0]?.props.alt).toBe('/review-and-draft-pr')
  // The rest of the metric draws in the failure text, one SVG per part.
  const parts = (row?.children ?? []).slice(1).map(child => child.props.alt)
  expect(parts).toEqual(['fits ·', '12 steps'])
  // The turn is over, so the open card gives advice for the next request in one line.
  await desktop.press({ key: 'row-K1' })
  const fix = (await desktop.find({ key: 'fix-K1' })) as { children?: { props: Record<string, unknown> }[] } | undefined
  expect(fix?.children?.[1]?.props.alt).toBe('/review-and-draft-pr')
  expect((await desktop.find({ text: /^Next time, start with$/ })) !== undefined).toBe(true)
  expect(await desktop.find({ text: /review and draft pr for this/ })).toBe(undefined)
  expect((await desktop.find({ key: 'card-fix-K1' }))?.props.label).toBe('Got it')
  // Got it sends nothing and closes the card.
  await desktop.press({ key: 'card-fix-K1' })
  expect(fake.fills).toEqual([])
  expect(fake.submits).toEqual([])
  expect(await desktop.find({ key: 'card-fix-K1' })).toBe(undefined)
  // A short turn does not read the list again.
  await turn($, fake)
  expect(lists).toBe(1)
})

test('a fix with a command takes one line, and a long prompt shows about three lines', () => {
  // A plugin skill shows its short name; the fix still sends the full name.
  expect(commandSummary({ id: 'K1', prompt: '/anthropic-skills:docs' })).toEqual({ name: '/docs' })
  expect(commandSummary({ id: 'O', prompt: '/model claude-sonnet-5-5' })).toEqual({ name: '/model claude-sonnet-5-5' })
  expect(commandSummary({ id: 'C1', prompt: 'List each change you made outside the plan.' })).toBe(null)
  const long = 'word '.repeat(60).trim()
  const preview = promptPreview(long)
  expect(preview.isCut).toBe(true)
  expect(preview.text.length).toBeLessThanOrEqual(181)
  expect(preview.text.endsWith('word…')).toBe(true)
  expect(promptPreview('short')).toEqual({ text: 'short', isCut: false })
})

test('the demo tour adds checks step by step and keeps the live checks apart', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  const presentation = { isFullscreen: false, columns: 150 }
  const run = (args: string) => $.command.run({ command: 'antiburn', args, presentation } as never)
  expect((await run('demo next')).text).toMatch(/^Start the tour first/)
  await run('demo tour')
  const pane = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  const band = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'AbovePrompt', props: stripProps(150) })
  expect((await pane.find({ text: /^No checks yet$/ })) !== undefined).toBe(true)
  // A real turn feeds only the live tracker, so the tour does not show its retry loop.
  await turn($, fake, { failures: 3 })
  expect(await pane.find({ text: /Retry loop/ })).toBe(undefined)
  expect((await run('demo next')).text).toBe('Tour step 1 of 4: A retry loop is the first warning.')
  expect((await band.find({ text: /Retry loop/ })) !== undefined).toBe(true)
  await pane.press({ key: 'tour-next' })
  expect((await band.find({ text: /4 to fix \(3 new\)/ })) !== undefined).toBe(true)
  await run('demo next')
  await run('demo next')
  expect((await pane.find({ text: /Demo tour · step 4 of 4/ })) !== undefined).toBe(true)
  expect((await run('demo next')).text).toMatch(/^The tour has no more steps/)
  await pane.press({ key: 'tour-end' })
  expect(await pane.find({ key: 'tour' })).toBe(undefined)
  // The live checks come back with the real retry loop.
  expect((await pane.find({ text: /Retry loop/ })) !== undefined).toBe(true)
})

test('the chip does not count a building check, so it agrees with the pane', async ($, on) => {
  fakeEngine(on, ['desktop'])
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  const run = (args: string) => $.command.run({ command: 'antiburn', args, presentation } as never)
  await run('demo tour')
  await run('demo next')
  const pane = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  const chip = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'SessionMode', props: { modes: [] } } as never)
  expect((await chip.find({ key: 'chip' }))?.props.label).toBe('antiburn · 1')
  // Step 1 also holds T at building. The pane leaves it out, and so does the chip.
  await pane.press({ key: 'row-X1' })
  await pane.press({ key: 'hide-X1' })
  expect((await pane.find({ text: /^Nothing to fix right now\.$/ })) !== undefined).toBe(true)
  expect((await chip.find({ key: 'chip' }))?.props.label).toBe('antiburn')
  expect((await chip.find({ text: /^✓$/ })) !== undefined).toBe(true)
})

test('a fix pressed in the demo tour acts for real and resolves on the next step', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($, 'default')
  const presentation = { isFullscreen: false, columns: 100 }
  const run = (args: string) => $.command.run({ command: 'antiburn', args, presentation } as never)
  await run('demo tour')
  for (let step = 0; step < 2; step += 1) await run('demo next')
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  await desktop.press({ key: 'card-fix-X1' })
  await desktop.press({ key: 'card-fix-T' })
  expect(fake.submits.length).toBe(1)
  expect(fake.fills).toEqual(['/effort high'])
  expect(await desktop.find({ key: 'card-fix-X1' })).toBe(undefined)
  expect(await desktop.find({ key: 'card-fix-T' })).toBe(undefined)
  await run('demo next')
  const terminal = await $.ui.mount({ plugin: 'antiburn-live', surface: 'terminal', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  await terminal.press({ key: 'group-resolved' })
  expect((await terminal.find({ text: /✓ Fixed/ })) !== undefined).toBe(true)
})

test('the desktop watermark stays above the tour row, so Next step takes clicks', async ($, on) => {
  fakeEngine(on, ['desktop'])
  await startSession($)
  const presentation = { isFullscreen: false, columns: 100 }
  const desktop = await $.ui.mount({ plugin: 'antiburn-live', surface: 'desktop', component: 'Pane', requestId: 'checks', props: PANE_PROPS })
  expect((await desktop.find({ key: 'watermark' }))?.props.bottom).toBe(1)
  await $.command.run({ command: 'antiburn', args: 'demo tour', presentation } as never)
  expect((await desktop.find({ key: 'watermark' }))?.props.bottom).toBe(2)
  await desktop.press({ key: 'tour-next' })
  expect((await desktop.find({ text: /step 1 of 4/ })) !== undefined).toBe(true)
})

test('a new check opens the pane once, without the keys', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  await turn($, fake, { failures: 3 })
  expect(fake.opens).toEqual(['checks'])
  expect(fake.focusedOpens).toEqual([false])
})

test('a new check leaves an open pane as it is', async ($, on) => {
  const fake = fakeEngine(on)
  await startSession($)
  fake.panes = ['checks']
  await turn($, fake, { failures: 3 })
  expect(fake.opens).toEqual([])
})

