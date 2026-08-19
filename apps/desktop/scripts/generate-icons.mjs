// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Generator for every PNG under `src-tauri/icons/`.
 *
 * The antiburn mark is three descending rounded bars — a burn-down of agent
 * spend. It is drawn analytically here (signed-distance coverage, no raster
 * input, no third-party dependency) so the checked-in binaries have a
 * reviewable source: re-running `pnpm icons` reproduces them byte for byte.
 *
 * Usage: `pnpm --filter @antiburn/desktop icons`
 */

import { deflateSync } from "node:zlib"
import { mkdirSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const OUTPUT_DIR = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons")

/** App-icon plate color (opaque, dark neutral). */
const PLATE = [22, 24, 29]
/** App-icon mark color. */
const MARK_ON_PLATE = [255, 255, 255]

/**
 * Mark geometry in unit coordinates (0..1 of the canvas edge).
 *
 * `inset` leaves room for the plate's rounded corners on the app icon; the
 * tray glyph uses the full canvas because it has no plate.
 */
const APP_MARK = { left: 0.24, right: 0.76, baseline: 0.735, heights: [0.44, 0.325, 0.21] }
const TRAY_MARK = { left: 0.06, right: 0.94, baseline: 0.9, heights: [0.76, 0.56, 0.36] }

/** Corner radius of the app icon plate, as a fraction of the canvas edge. */
const PLATE_RADIUS = 0.2237

function main() {
  mkdirSync(OUTPUT_DIR, { recursive: true })

  for (const size of [32, 128, 256, 512, 1024]) {
    const name = size === 256 ? "128x128@2x.png" : `${size}x${size}.png`
    writePng(join(OUTPUT_DIR, size === 1024 ? "icon.png" : name), renderAppIcon(size))
  }

  // The tray glyph is a macOS template image: shape carried entirely by the
  // alpha channel so the system can tint it for light, dark, and pressed
  // menu-bar states. It is deliberately black-on-transparent.
  writePng(join(OUTPUT_DIR, "tray.png"), renderTrayIcon(128))

  console.log(`wrote antiburn icons to ${OUTPUT_DIR}`)
}

/** Opaque rounded plate with the mark knocked out in white. */
function renderAppIcon(size) {
  const canvas = createCanvas(size)
  const plate = roundedRect(0, 0, size, size, PLATE_RADIUS * size)
  fill(canvas, plate, PLATE, 1)
  for (const bar of markBars(APP_MARK, size)) {
    fill(canvas, bar, MARK_ON_PLATE, 1)
  }
  return canvas
}

/** Transparent canvas carrying only the mark, in black. */
function renderTrayIcon(size) {
  const canvas = createCanvas(size)
  for (const bar of markBars(TRAY_MARK, size)) {
    fill(canvas, bar, [0, 0, 0], 1)
  }
  return canvas
}

/**
 * Three descending bars with fully rounded caps, laid out left to right with
 * gaps half a bar wide.
 */
function markBars(spec, size) {
  const span = (spec.right - spec.left) * size
  const count = spec.heights.length
  // width * count + (width / 2) * (count - 1) === span
  const width = span / (count + (count - 1) / 2)
  const gap = width / 2

  return spec.heights.map((height, index) => {
    const x = spec.left * size + index * (width + gap)
    const bottom = spec.baseline * size
    const top = bottom - height * size
    return roundedRect(x, top, width, bottom - top, width / 2)
  })
}

function createCanvas(size) {
  return { size, pixels: new Float64Array(size * size * 4) }
}

/**
 * Signed-distance description of a rounded rectangle. Negative inside,
 * positive outside, measured in pixels.
 */
function roundedRect(x, y, width, height, radius) {
  const cx = x + width / 2
  const cy = y + height / 2
  const halfWidth = Math.max(width / 2 - radius, 0)
  const halfHeight = Math.max(height / 2 - radius, 0)
  return (px, py) => {
    const dx = Math.max(Math.abs(px - cx) - halfWidth, 0)
    const dy = Math.max(Math.abs(py - cy) - halfHeight, 0)
    return Math.hypot(dx, dy) - radius
  }
}

/**
 * Source-over composite of a shape onto the canvas.
 *
 * Coverage comes from the signed distance at the pixel center, clamped across
 * one pixel of width. That is a close enough analytic antialias for shapes
 * this simple and keeps the generator dependency-free.
 */
function fill(canvas, shape, [r, g, b], alpha) {
  const { size, pixels } = canvas
  for (let y = 0; y < size; y += 1) {
    for (let x = 0; x < size; x += 1) {
      const distance = shape(x + 0.5, y + 0.5)
      const coverage = clamp(0.5 - distance, 0, 1) * alpha
      if (coverage <= 0) {
        continue
      }
      const offset = (y * size + x) * 4
      const dstAlpha = pixels[offset + 3]
      const outAlpha = coverage + dstAlpha * (1 - coverage)
      for (let channel = 0; channel < 3; channel += 1) {
        const src = [r, g, b][channel] / 255
        const dst = pixels[offset + channel]
        pixels[offset + channel] =
          outAlpha === 0 ? 0 : (src * coverage + dst * dstAlpha * (1 - coverage)) / outAlpha
      }
      pixels[offset + 3] = outAlpha
    }
  }
}

function clamp(value, min, max) {
  return Math.min(Math.max(value, min), max)
}

/** Encodes the canvas as an 8-bit RGBA PNG. */
function writePng(path, canvas) {
  const { size, pixels } = canvas
  // One filter byte (0 = None) per scanline, then RGBA samples.
  const raw = Buffer.alloc(size * (size * 4 + 1))
  let cursor = 0
  for (let y = 0; y < size; y += 1) {
    raw[cursor] = 0
    cursor += 1
    for (let x = 0; x < size; x += 1) {
      const offset = (y * size + x) * 4
      for (let channel = 0; channel < 4; channel += 1) {
        raw[cursor] = Math.round(clamp(pixels[offset + channel], 0, 1) * 255)
        cursor += 1
      }
    }
  }

  const ihdr = Buffer.alloc(13)
  ihdr.writeUInt32BE(size, 0)
  ihdr.writeUInt32BE(size, 4)
  ihdr[8] = 8 // bit depth
  ihdr[9] = 6 // color type: truecolor with alpha
  ihdr[10] = 0 // deflate
  ihdr[11] = 0 // adaptive filtering
  ihdr[12] = 0 // no interlace

  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ])

  writeFileSync(path, png)
}

function chunk(type, data) {
  const length = Buffer.alloc(4)
  length.writeUInt32BE(data.length, 0)
  const body = Buffer.concat([Buffer.from(type, "ascii"), data])
  const crc = Buffer.alloc(4)
  crc.writeUInt32BE(crc32(body), 0)
  return Buffer.concat([length, body, crc])
}

const CRC_TABLE = (() => {
  const table = new Uint32Array(256)
  for (let n = 0; n < 256; n += 1) {
    let c = n
    for (let k = 0; k < 8; k += 1) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    }
    table[n] = c >>> 0
  }
  return table
})()

function crc32(buffer) {
  let crc = 0xffffffff
  for (const byte of buffer) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8)
  }
  return (crc ^ 0xffffffff) >>> 0
}

main()
