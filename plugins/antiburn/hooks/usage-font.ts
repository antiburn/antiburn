import { BITCOUNT_GLYPHS, BITCOUNT_PIXEL, BITCOUNT_TABULAR, type DotGlyph } from './usage-font-data'

export { BITCOUNT_PIXEL }
export type CaptionGlyph = { character: string; x: number; advance: number; dots: DotGlyph['dots'] }

export function captionGlyphs(text: string, size = 15, tabular = false, tracking = 0.035): CaptionGlyph[] {
  let x = 0
  return [...text.toUpperCase()].map((character) => {
    const glyph = (tabular ? BITCOUNT_TABULAR[character] : undefined) ?? BITCOUNT_GLYPHS[character] ?? BITCOUNT_GLYPHS['—']!
    // The source tnum feature keeps mixed advances. Fixed digit cells prevent rolls from shifting unchanged columns.
    const units = tabular && /[0-9]/.test(character) ? 700 : glyph.advance
    const advance = (units / 1000 + tracking) * size
    const result = { character, x, advance, dots: glyph.dots }
    x += advance
    return result
  })
}

export function captionWidth(text: string, size = 15, tabular = false, tracking = 0.035): number {
  const glyphs = captionGlyphs(text, size, tabular, tracking)
  const last = glyphs[glyphs.length - 1]
  return last === undefined ? 0 : last.x + last.advance
}

export function captionSvg(text: string, x: number, baseline: number, size = 15, tabular = false, tracking = 0.035): string {
  const scale = size / 1000
  return captionGlyphs(text, size, tabular, tracking)
    .map(
      (glyph) =>
        `<g transform="translate(${x + glyph.x} ${baseline}) scale(${scale} ${-scale})">${glyph.dots.map(([dx, dy]) => `<use href="#bitcount-pixel" x="${dx}" y="${dy}"/>`).join('')}</g>`,
    )
    .join('')
}
