/**
 * Provenance check for hand-inlined artwork.
 *
 * `brandMarks.ts` records where each mark came from; this asserts the record
 * is true of the bytes actually shipped. The source packages are
 * devDependencies, present for exactly this reason and absent from the bundle.
 */

import { icons as logos } from "@iconify-json/logos"
import { describe, expect, it } from "vitest"

import antigravityAsset from "./fixtures/antigravity-mark.svg?raw"
import ompAsset from "./fixtures/omp-mark.svg?raw"
import { ANTIGRAVITY_MARK, OMP_MARK, OPENAI_MARK } from "./brandMarks"

/** The hex SHA-256 of a text asset. */
async function sha256Of(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text))
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join(
    "",
  )
}

/**
 * The bounds of a path made only of straight horizontal and vertical lines.
 *
 * Supports `M`, `H`, `V`, `h`, `v`, and `z`. Any other command fails the test,
 * so a changed source path cannot pass with wrong bounds.
 */
function rectilinearBounds(path: string) {
  let x = 0
  let y = 0
  const xs: number[] = []
  const ys: number[] = []
  for (const [, command, args] of path.matchAll(/([A-Za-z])([^A-Za-z]*)/g)) {
    const values = args!
      .trim()
      .split(/[\s,]+/)
      .filter(Boolean)
      .map(Number)
    if (command === "M") [x, y] = values as [number, number]
    else if (command === "H") x = values[0]!
    else if (command === "V") y = values[0]!
    else if (command === "h") x += values[0]!
    else if (command === "v") y += values[0]!
    else if (command === "z") continue
    else throw new Error(`unsupported path command ${command}`)
    xs.push(x)
    ys.push(y)
  }
  return {
    minX: Math.min(...xs),
    minY: Math.min(...ys),
    maxX: Math.max(...xs),
    maxY: Math.max(...ys),
  }
}

describe("OPENAI_MARK", () => {
  it("matches the path published by its recorded source", () => {
    const upstream = logos.icons[OPENAI_MARK.provenance.icon]
    expect(upstream, `${OPENAI_MARK.provenance.icon} is gone from the package`).toBeDefined()
    // The collection stores a full `<path .../>` element; compare the `d`.
    const d = /\sd="([^"]+)"/.exec(upstream!.body)?.[1]
    expect(d).toBe(OPENAI_MARK.path)
  })

  it("matches the viewBox published by its recorded source", () => {
    const upstream = logos.icons[OPENAI_MARK.provenance.icon]!
    const width = upstream.width ?? logos.width
    const height = upstream.height ?? logos.height
    expect(OPENAI_MARK.viewBox).toBe(`0 0 ${width} ${height}`)
  })

  it("names the version of the package it was taken from", () => {
    // A bare package name would let a later upgrade silently redefine what the
    // recorded provenance refers to.
    expect(OPENAI_MARK.provenance.package).toMatch(/@\d+\.\d+\.\d+$/)
  })

  it("draws a single path, so it inherits the theme ink", () => {
    expect(OPENAI_MARK.path).not.toContain("<")
  })
})

describe("ANTIGRAVITY_MARK", () => {
  it("matches the checksummed source fixture", async () => {
    const path = /<path d="([^"]+)"/.exec(antigravityAsset)?.[1]
    const viewBox = /viewBox="([^"]+)"/.exec(antigravityAsset)?.[1]

    expect(path).toBe(ANTIGRAVITY_MARK.path)
    expect(viewBox).toBe(ANTIGRAVITY_MARK.viewBox)
    expect(await sha256Of(antigravityAsset)).toBe(ANTIGRAVITY_MARK.provenance.assetSha256)
  })
})

describe("OMP_MARK", () => {
  it("matches the checksummed source fixture", async () => {
    // The favicon has one glyph path; its tile is a `<rect>`.
    const paths = [...ompAsset.matchAll(/<path\b[^>]*\sd="([^"]+)"/g)].map((match) => match[1])

    expect(paths).toEqual([OMP_MARK.path])
    expect(await sha256Of(ompAsset)).toBe(OMP_MARK.provenance.assetSha256)
  })

  it("crops the source tile to a square that centres the glyph", () => {
    const sourceViewBox = /viewBox="([^"]+)"/.exec(ompAsset)?.[1]
    expect(sourceViewBox).toBe("0 0 64 64")

    const { minX, minY, maxX, maxY } = rectilinearBounds(OMP_MARK.path)
    const side = Math.max(maxX - minX, maxY - minY)
    const left = (minX + maxX - side) / 2
    const top = (minY + maxY - side) / 2
    expect(OMP_MARK.viewBox).toBe(`${left} ${top} ${side} ${side}`)
  })

  it("draws a single path, so it inherits the theme ink", () => {
    expect(OMP_MARK.path).not.toContain("<")
    expect(OMP_MARK.hex).toBeUndefined()
  })
})
