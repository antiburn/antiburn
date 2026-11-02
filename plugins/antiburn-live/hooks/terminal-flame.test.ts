import { terminalWordmark } from './terminal-flame'
import { expect, test } from 'claude-code/testing'
import { terminalBrandCells } from './terminal-flame'

test('background flames preserve the readable brand letters and muted fill', () => {
  const a = atob(terminalBrandCells(900))
  const b = atob(terminalBrandCells(1100))
  expect(a.length).toBe(240)
  expect(b).not.toBe(a)
  for (let x = 1; x <= 8; x += 1) {
    expect(a.charCodeAt(120 + x * 12)).toBe(' antiburn '.charCodeAt(x))
    expect(a.slice(120 + x * 12 + 4, 120 + x * 12 + 8)).toBe(String.fromCharCode(0x14, 0x59, 0xff, 0))
  }
  expect(terminalBrandCells(900, 1, true)).toBe(terminalBrandCells(1600, 1, true))
})

test('two-row frames keep upper padding blank at rest and use complete shaded glyphs when active', () => {
  const idle = atob(terminalBrandCells(0, 0))
  for (let x = 0; x < 10; x += 1) expect(idle.charCodeAt(x * 12)).toBe(32)
  const frames = [700, 900, 1200, 1600].map((time) => terminalBrandCells(time))
  expect(new Set(frames).size).toBe(4)
  for (const frame of frames) {
    const bytes = atob(frame)
    expect(bytes.length).toBe(240)
    for (let x = 0; x < 10; x += 1) {
      const glyph = bytes.charCodeAt(x * 12) | (bytes.charCodeAt(x * 12 + 1) << 8)
      expect(glyph === 32 || '▄░▒'.includes(String.fromCharCode(glyph))).toBe(true)
    }
  }
})

test('linked terminal letters retain raster colors and exact cell positions', () => {
  for (const elapsed of [0, 900, 1500]) {
    const cells = terminalBrandCells(elapsed, elapsed === 0 ? 0 : 1)
    const word = terminalWordmark(cells)
    expect(
      word
        .slice(1, 9)
        .map((cell) => cell.glyph)
        .join(''),
    ).toBe('antiburn')
    expect(word.slice(1, 9).every((cell) => cell.color === '#ff5914')).toBe(true)
    expect(word).toHaveLength(10)
    expect(word.every((cell) => /^#[0-9a-f]{6}$/.test(cell.backgroundColor))).toBe(true)
  }
})
