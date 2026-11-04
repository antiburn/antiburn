import { PRESENTATION, usageLayout } from './usage-presentation'

type Layout = ReturnType<typeof usageLayout>

function innerSvg(source: string): string {
  const open = source.indexOf('>')
  const close = source.lastIndexOf('</svg>')
  if (open < 0 || close < open) throw new Error('Expected an SVG source')
  return source.slice(open + 1, close)
}

function scopeSource(
  source: string,
  scope: string,
  responsive: boolean,
  layout: Layout,
  columns: number,
): string {
  const ids = [...source.matchAll(/\bid="([^"]+)"/g)].map((match) => match[1]!)
  const idMap = new Map(ids.map((id) => [id, `${scope}-${id}`]))
  let namespaced = source.replace(
    /\bid="([^"]+)"/g,
    (_, id: string) => `id="${idMap.get(id)}"`,
  )
  for (const [id, namespacedId] of idMap) {
    namespaced = namespaced
      .replaceAll(`href="#${id}"`, `href="#${namespacedId}"`)
      .replaceAll(`url(#${id})`, `url(#${namespacedId})`)
      .replace(new RegExp(`#${id}(?![\\w-])`, 'g'), `#${namespacedId}`)
  }

  return namespaced.replace(/<style>([\s\S]*?)<\/style>/g, (_, originalCss: string) => {
    let css = originalCss
    const keyframes = [...css.matchAll(/@keyframes\s+([\w-]+)/g)].map(
      (match) => match[1]!,
    )
    for (const name of keyframes) {
      const uniqueName = `${scope}-${name}`
      css = css.replace(
        new RegExp(`(@keyframes\\s+)${name}(?![\\w-])`, 'g'),
        `$1${uniqueName}`,
      )
      css = css.replace(
        /(animation(?:-name)?\s*:\s*)([^;}]+)/g,
        (_declaration: string, prefix: string, value: string) =>
          prefix +
          value.replace(new RegExp(`(?<![\\w-])${name}(?![\\w-])`, 'g'), uniqueName),
      )
    }
    if (responsive) {
      const fixedCells =
        (PRESENTATION.brandInset * 2 +
          PRESENTATION.markSlotSize +
          PRESENTATION.brandGap +
          (layout.inline ? PRESENTATION.inlineGap : 0)) /
        8
      css = css
        .replaceAll('100vw', 'var(--meter-width)')
        .replaceAll(':root', ':scope')
        .replace(
          /@media\(min-width:([\d.]+)px\)/g,
          (_match: string, pixels: string) =>
            `@media(min-width:calc(${Number(pixels) * (layout.inline ? 2 : 1)}px + ${fixedCells} * 100vw / ${columns}))`,
        )
    }
    return `<style>@scope (.${scope}){${css}}</style>`
  })
}

function packGlyphs(svg: string): string {
  const glyphs = new Map<string, string>()
  const packed = svg.replace(
    /(<g transform="[^"]+">)((?:<use\b[^>]*\/>)+)<\/g>/g,
    (_match, open: string, dots: string) => {
      let id = glyphs.get(dots)
      if (id === undefined) {
        id = `strip-glyph-${glyphs.size}`
        glyphs.set(dots, id)
      }
      return `${open}<use href="#${id}"/></g>`
    },
  )
  if (glyphs.size === 0) return packed
  const definitions = [...glyphs]
    .map(([dots, id]) => `<g id="${id}">${dots}</g>`)
    .join('')
  return packed.replace('>', `><defs>${definitions}</defs>`)
}

function replayIdentity(source: string): string {
  const identity = source.match(/\bdata-entrance="\d+"/)?.[0]
  return identity ? ` ${identity}` : ''
}

function markTransform(mark: string): string {
  const viewBox = mark.match(/\bviewBox="([^"]+)"/)?.[1]
  const values = viewBox
    ?.trim()
    .split(/[\s,]+/)
    .map(Number)
  if (
    values === undefined ||
    values.length !== 4 ||
    values.some((value) => !Number.isFinite(value)) ||
    values[2]! <= 0 ||
    values[3]! <= 0
  )
    throw new Error('Expected a finite, positive mark viewBox')
  const [x, y, width, height] = values as [number, number, number, number]
  const scale = PRESENTATION.markSize / Math.max(width, height)
  const xOffset = (PRESENTATION.markSize - width * scale) / 2
  const yOffset = (PRESENTATION.markSize - height * scale) / 2
  return `translate(${xOffset} ${yOffset}) scale(${scale}) translate(${-x} ${-y})`
}

function contentViewBox(
  source: string,
  width: number,
  height: number,
): [number, number, number, number] {
  const value = source.match(/\bviewBox="([^"]+)"/)?.[1]
  const parts = value
    ?.trim()
    .split(/[\s,]+/)
    .map(Number) ?? [0, 0, width, height]
  if (
    parts.length !== 4 ||
    parts.some((part) => !Number.isFinite(part)) ||
    parts[2]! <= 0 ||
    parts[3]! <= 0
  )
    throw new Error('Expected a finite, positive status viewBox')
  return parts as [number, number, number, number]
}

export type UsageStripStatusGraphic = {
  source: string
  width: number
  height: number
}

export function usageStatusStripSvg(
  markSource: string,
  status: UsageStripStatusGraphic,
  columns: number,
  secondary?: UsageStripStatusGraphic,
  refresh = false,
): string {
  if (!Number.isFinite(columns) || columns <= 0)
    throw new Error('Expected a positive column count')
  const totalHeight = status.height + (secondary?.height ?? 0)
  const brand = scopeSource(markSource, 'strip-brand', false, usageLayout(1), columns)
  const mark = `<g class="strip-mark"><g class="strip-brand"${replayIdentity(markSource)} transform="${markTransform(markSource)}">${innerSvg(brand)}</g></g>`
  const cell = `calc(100vw / ${columns})`
  const offset =
    (PRESENTATION.brandInset + PRESENTATION.markSlotSize + PRESENTATION.brandGap) / 8
  const content = (graphic: UsageStripStatusGraphic, scope: string, y: number) => {
    if (
      !Number.isFinite(graphic.width) ||
      graphic.width <= 0 ||
      !Number.isFinite(graphic.height) ||
      graphic.height <= 0
    )
      throw new Error('Expected positive status dimensions')
    const [x, top, width, height] = contentViewBox(
      graphic.source,
      graphic.width,
      graphic.height,
    )
    const clipId = `${scope}-viewport`
    const inset = scope === 'strip-status' ? offset : 0
    const reserve = refresh && scope === 'strip-status' ? 4 : 0
    const clipWidth = `max(0px, calc(100vw - ${inset + reserve} * var(--cell)))`
    const scoped = scopeSource(graphic.source, scope, false, usageLayout(1), columns)
    return `<defs><clipPath id="${clipId}"><rect x="${x}" y="${top}" style="width:${clipWidth}" height="${height}"/></clipPath></defs><g class="${scope}" style="transform:translate(calc(${inset} * ${cell}),${y}px)" clip-path="url(#${clipId})">${innerSvg(scoped)}</g>`
  }
  const body =
    content(status, 'strip-status', 0) +
    (secondary === undefined
      ? ''
      : content(secondary, 'strip-status-secondary', status.height))
  const style = `<style>:root{--cell:${cell}}.strip-mark{transform:translate(calc(${PRESENTATION.brandInset / 8} * var(--cell) + (${PRESENTATION.markSlotSize / 8} * var(--cell) - ${PRESENTATION.markSize}px) / 2),${(status.height - PRESENTATION.markSize) / 2}px)}</style>`
  return packGlyphs(
    `<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="${totalHeight}">${style}${mark}${body}</svg>`,
  )
}

export function usageStripSvg(
  markSource: string,
  sources: readonly string[],
  layout: Layout,
  columns: number,
): string {
  if (sources.length !== 2) throw new Error('A usage strip needs two meter sources')
  if (!Number.isFinite(columns) || columns <= 0)
    throw new Error('Expected a positive column count')
  const fixedCells =
    (PRESENTATION.brandInset * 2 +
      PRESENTATION.markSlotSize +
      PRESENTATION.brandGap +
      (layout.inline ? PRESENTATION.inlineGap : 0)) /
    8
  const meterWidth = layout.inline
    ? `calc((100vw - ${fixedCells} * var(--cell)) / 2)`
    : `calc(100vw - ${fixedCells} * var(--cell))`
  const metersX =
    (PRESENTATION.brandInset + PRESENTATION.markSlotSize + PRESENTATION.brandGap) / 8
  const style = `<style>:root{--cell:calc(100vw / ${columns});--meter-width:${meterWidth};--meters-x:calc(${metersX} * var(--cell))}.strip-mark{transform:translate(calc(${PRESENTATION.brandInset / 8} * var(--cell) + (${PRESENTATION.markSlotSize / 8} * var(--cell) - ${PRESENTATION.markSize}px) / 2),${(layout.height - PRESENTATION.markSize) / 2}px)}</style>`
  const markScope = 'strip-brand'
  const brand = scopeSource(markSource, markScope, false, layout, columns)
  const markGroup = `<g class="strip-mark"><g class="strip-brand"${replayIdentity(markSource)} transform="${markTransform(markSource)}">${innerSvg(brand)}</g></g>`
  const meters = sources
    .map((source, index) => {
      const scope = `strip-meter-${index}`
      const x =
        index === 0 || !layout.inline
          ? 'var(--meters-x)'
          : `calc(var(--meters-x) + var(--meter-width) + ${PRESENTATION.inlineGap / 8} * var(--cell))`
      const y =
        index === 1 && !layout.inline ? layout.meterHeight + layout.stackedGap : 0
      const scoped = scopeSource(source, scope, true, layout, columns)
      return `<g class="strip-meter ${scope}"${replayIdentity(source)} style="transform:translate(${x},${y}px)">${innerSvg(scoped)}</g>`
    })
    .join('')
  return packGlyphs(
    `<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="${layout.height}">${style}${markGroup}${meters}</svg>`,
  )
}
