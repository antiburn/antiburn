import { expect, test } from 'claude-code/testing'
import type { RenderElement } from 'claude-code'
import { fadeUsageSvg, UsageEntrance } from './usage-entrance'

const svg = (text: string) =>
  `<svg xmlns="http://www.w3.org/2000/svg"><text>${text}</text></svg>`
const graphic = (text = '8%'): Extract<RenderElement, { type: 'Svg' }> => ({
  type: 'Svg',
  props: {
    source: svg(text),
    alt: '5-hour usage',
    width: 288,
    height: 18,
    isInteractive: true,
  },
})

test('usage fades only opacity with a shorter reduced-motion duration and a visible fallback', () => {
  const source = fadeUsageSvg(svg('8%'), 120)
  expect(source).toContain(
    'animation:usage-enter 240ms cubic-bezier(.23,1,.32,1) -120ms both',
  )
  expect(source).toContain(
    '@keyframes usage-enter{from{opacity:0}to{opacity:1}}',
  )
  expect(source).toContain(
    '@media(prefers-reduced-motion:reduce){.usage-entrance{animation-duration:80ms}}',
  )
  expect(source).not.toContain('transform')
  expect(source).not.toContain('.usage-entrance{opacity:0')
  expect(source).toContain('<text>8%</text></g></svg>')
  expect(fadeUsageSvg(svg('8%'), 240)).toBe(svg('8%'))
})

test('unchanged graphics keep their source while new readings continue the same fade', () => {
  const owner = new UsageEntrance()
  const first = owner.render(graphic(), 'desktop', 1000)
  expect(first.props.source).toContain(' 0ms both')
  expect(owner.render(graphic(), 'desktop', 1100).props.source).toBe(
    first.props.source,
  )
  const changed = owner.render(graphic('9%'), 'desktop', 1120)
  expect(changed.props.source).toContain(' -120ms both')
  expect(changed.props.alt).toBe('5-hour usage')
  expect(changed.props.isInteractive).toBe(true)
  expect(owner.render(graphic('10%'), 'desktop', 1240).props.source).toBe(
    svg('10%'),
  )
  expect(owner.render(graphic('11%'), 'desktop', 2000).props.source).toBe(
    svg('11%'),
  )
})

test('status replacement and resize share the first visible timestamp', () => {
  const owner = new UsageEntrance()
  owner.render(graphic('Checking usage...'), 'desktop', 1000)
  expect(owner.render(graphic('8%'), 'desktop', 1080).props.source).toContain(
    ' -80ms both',
  )
  const resized = { ...graphic(), props: { ...graphic().props, width: 144 } }
  expect(owner.render(resized, 'desktop', 1120).props.width).toBe(144)
  expect(
    owner.render(graphic('Unavailable'), 'desktop', 1400).props.source,
  ).toBe(svg('Unavailable'))
})

test('sessions and module reloads grant new entrances', () => {
  const owner = new UsageEntrance()
  owner.render(graphic(), 'desktop', 1000)
  owner.reset()
  expect(owner.render(graphic(), 'desktop', 3000).props.source).toContain(
    ' 0ms both',
  )
  const reloaded = new UsageEntrance()
  const fresh = reloaded.render(graphic(), 'desktop', 4000).props.source
  expect(fresh).toContain(' 0ms both')
  expect(fresh).not.toBe(owner.render(graphic(), 'desktop', 3000).props.source)
})

test('sites enter independently and bounded ownership does not evict an existing site', () => {
  const owner = new UsageEntrance()
  for (let site = 0; site < 8; site++) {
    expect(
      owner.render(graphic(), String(site), site * 1000).props.source,
    ).toContain(' 0ms both')
  }
  expect(owner.render(graphic(), 'overflow', 10000).props.source).toBe(
    svg('8%'),
  )
  expect(owner.render(graphic(), '0', 10000).props.source).toContain(
    ' 0ms both',
  )
})

test('the fade changes graphics without changing controls or geometry', () => {
  const owner = new UsageEntrance()
  const button = { type: 'Text' as const, props: {}, children: ['Refresh'] }
  const drawing = {
    type: 'Box' as const,
    props: { width: 80 },
    children: [graphic(), button],
  }
  const faded = owner.render(drawing, 'desktop', 1000)
  expect(faded.props).toBe(drawing.props)
  expect(faded.children[1]).toBe(button)
  expect(JSON.stringify(faded.children[0])).toContain('usage-enter')
})

test('a new entrance changes the animation name for retained SVG elements', () => {
  const owner = new UsageEntrance()
  const first = owner.render(graphic(), 'desktop', 1000).props.source
  const name = (source: string) => source.match(/animation:([^ ]+)/)?.[1]
  const changed = owner.render(graphic('9%'), 'desktop', 1100).props.source
  expect(name(changed)).toBe(name(first))
  owner.reset()
  const returned = owner.render(graphic(), 'desktop', 2000).props.source
  expect(name(returned)).not.toBe(name(first))
  expect(returned).toContain(`@keyframes ${name(returned)}{`)
})

test('completed graphics retain their source across host redraws', () => {
  const owner = new UsageEntrance()
  const first = owner.render(graphic(), 'desktop', 1000).props.source
  expect(owner.render(graphic(), 'desktop', 1300).props.source).toBe(first)
  expect(owner.render(graphic(), 'desktop', 9000).props.source).toBe(first)
  expect(owner.render(graphic('9%'), 'desktop', 9000).props.source).toBe(
    svg('9%'),
  )
})

test('desktop attachments share an appearance without resetting peers or the first handshake', () => {
  const owner = new UsageEntrance()
  owner.start()
  const first = owner.render(graphic(), 'desktop', 1000).props.source
  owner.attach('one')
  owner.attach('one')
  owner.attach('two')
  owner.detach('one')
  expect(owner.render(graphic(), 'desktop', 1400).props.source).toBe(first)
  owner.detach('two')
  owner.attach('one')
  expect(owner.render(graphic(), 'desktop', 2000).props.source).not.toBe(first)
})

test('an already attached host can detach after a module reload', () => {
  const owner = new UsageEntrance()
  const first = owner.render(graphic(), 'desktop', 1000).props.source
  owner.detach('preexisting-desktop')
  owner.attach('preexisting-desktop')
  expect(owner.render(graphic(), 'desktop', 2000).props.source).not.toBe(first)
})

test('the opacity wrapper closes after nested SVG roots', () => {
  const nested = '<svg><svg><text>8%</text></svg></svg>'
  expect(fadeUsageSvg(nested, 0)).toContain(
    '<svg><text>8%</text></svg></g></svg>',
  )
})

test('SVG-free drawings do not consume the first graphic entrance', () => {
  const owner = new UsageEntrance()
  const empty: Extract<RenderElement, { type: 'Box' }> = { type: 'Box', props: {}, children: [] }
  expect(owner.render(empty, 'desktop', 1000)).toBe(empty)
  const first = owner.render(graphic(), 'desktop', 3000)
  expect(first.props.source).toContain(' 0ms both')
})
