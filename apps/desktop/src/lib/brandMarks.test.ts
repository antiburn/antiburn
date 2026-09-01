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
import { ANTIGRAVITY_MARK, OPENAI_MARK } from "./brandMarks"

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
    const digest = await crypto.subtle.digest(
      "SHA-256",
      new TextEncoder().encode(antigravityAsset),
    )
    const sha256 = Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("")

    expect(path).toBe(ANTIGRAVITY_MARK.path)
    expect(viewBox).toBe(ANTIGRAVITY_MARK.viewBox)
    expect(sha256).toBe(ANTIGRAVITY_MARK.provenance.assetSha256)
  })
})
