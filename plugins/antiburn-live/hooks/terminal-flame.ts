import { BURN_IGNITION } from './burn'

export const TERMINAL_FLAME = { columns: 10, rows: 2, frameMs: 100, fadeMs: 220 } as const
// Use the same heat field for the upper flames and the fill behind each letter.
export function terminalBrandCells(elapsed: number, strength = 1, reduced = false): string {
  const level = Math.max(0, Math.min(1, strength))
  const ignition = Math.max(0, Math.min(1, elapsed / BURN_IGNITION.duration))
  const word = ' antiburn '
  const heatAt = (x: number) =>
    level * (reduced ? 0.4 : (ignition * (Math.sin(elapsed / 190 + x * 1.7) + Math.sin(elapsed / 310 - x * 0.8) + 2)) / 4)
  let bytes = ''
  const cell = (glyph: number, foreground: number, background: number) => {
    for (const value of [glyph, foreground, background]) {
      for (let shift = 0; shift < 32; shift += 8) bytes += String.fromCharCode((value >>> shift) & 255)
    }
  }
  for (let x = 0; x < word.length; x += 1) {
    const heat = heatAt(x)
    const glyph = reduced || heat < 0.45 ? 32 : heat < 0.68 ? '▄'.charCodeAt(0) : heat < 0.86 ? '░'.charCodeAt(0) : '▒'.charCodeAt(0)
    const color = ((105 + Math.round(heat * 28)) << 16) | ((50 + Math.round(heat * 17)) << 8) | 29
    cell(glyph, color, 0x01000000)
  }
  for (let x = 0; x < word.length; x += 1) {
    const heat = heatAt(x)
    const edge = x === 0 || x === word.length - 1
    const glyph = !edge
      ? word.charCodeAt(x)
      : level <= 0
        ? 32
        : heat < 0.25
          ? '░'.charCodeAt(0)
          : heat < 0.65
            ? '▒'.charCodeAt(0)
            : '▓'.charCodeAt(0)
    const background = ((77 + Math.round(heat * 38)) << 16) | ((47 + Math.round(heat * 12)) << 8) | (35 - Math.round(heat * 6))
    cell(glyph, edge ? 0x9b502a : 0xff5914, background)
  }
  return btoa(bytes)
}

export function terminalWordmark(cells: string) {
  const bytes = atob(cells)
  const value = (offset: number) =>
    (bytes.charCodeAt(offset) | (bytes.charCodeAt(offset + 1) << 8) | (bytes.charCodeAt(offset + 2) << 16)) >>> 0
  const color = (offset: number) => `#${value(offset).toString(16).padStart(6, '0')}`
  return Array.from({ length: TERMINAL_FLAME.columns }, (_, index) => {
    const offset = (TERMINAL_FLAME.columns + index) * 12
    return { glyph: String.fromCharCode(value(offset)), color: color(offset + 4), backgroundColor: color(offset + 8) }
  })
}
