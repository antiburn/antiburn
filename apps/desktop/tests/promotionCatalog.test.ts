import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises"
import os from "node:os"
import path from "node:path"

import { describe, expect, it } from "vitest"

import { createPromotionFixture } from "./promotionFixture"
import { generatePromotionModule } from "../scripts/promotionCatalog"
import { parsePromotionYaml, validatePromotionDefinition } from "../src/promotions/validate"

const promotionsDirectory = path.resolve(process.cwd(), "src/promotions")
const claudeDirectory = path.join(promotionsDirectory, "claude-mod")

function baseDefinition(overrides: Record<string, unknown> = {}) {
  return {
    schema_version: 1,
    id: "sample-promo",
    revision: 1,
    title: "Sample",
    screens: {
      announcement: {
        title: "Sample",
        description: "Sample description",
      },
    },
    ...overrides,
  }
}

describe("promotion YAML definitions", () => {
  it("parses the production Claude Mod content and its media files", async () => {
    const source = await readFile(path.join(claudeDirectory, "promo.yaml"), "utf8")
    const definition = parsePromotionYaml(source, "claude-mod/promo.yaml")

    expect(definition.id).toBe("claude-mod")
    expect(definition.screens.announcement.features).toHaveLength(2)
    expect(definition.screens.setup?.steps).toHaveLength(3)
    expect(definition.screens.announcement.primary_action).toEqual({
      label: "Set up",
      type: "screen",
      target: "setup",
    })
  })

  it("rejects unknown keys, unsafe links, and missing setup destinations", () => {
    expect(() => validatePromotionDefinition(baseDefinition({ surprise: true }))).toThrow(
      'unknown key "surprise"',
    )

    const unsafe = baseDefinition({
      screens: {
        announcement: {
          title: "Sample",
          description: "Sample description",
          secondary_action: { type: "external", label: "Open", url: "javascript:alert(1)" },
        },
      },
    })
    expect(() => validatePromotionDefinition(unsafe)).toThrow(
      "only credential-free HTTPS links within the antiburn GitHub repository are supported",
    )

    const missingSetup = baseDefinition({
      screens: {
        announcement: {
          title: "Sample",
          description: "Sample description",
          primary_action: { type: "screen", label: "Set up", target: "setup" },
        },
      },
    })
    expect(() => validatePromotionDefinition(missingSetup)).toThrow(
      "setup screen is not defined",
    )
  })

  it("rejects traversal and missing assets", () => {
    const withMedia = (src: string) =>
      baseDefinition({
        screens: {
          announcement: {
            title: "Sample",
            description: "Sample description",
            media: { kind: "image", src, alt: "Sample image" },
          },
        },
      })

    expect(() => validatePromotionDefinition(withMedia("../../outside.png"))).toThrow(
      "expected a relative local asset path",
    )
    expect(() =>
      validatePromotionDefinition(withMedia("https://example.com/image.png")),
    ).toThrow("expected a relative local asset path")
  })

  it("validates light variants and includes their source and poster in the catalog", async () => {
    const source = await readFile(path.join(claudeDirectory, "promo.yaml"), "utf8")
    const definition = parsePromotionYaml(source, "claude-mod/promo.yaml")
    expect(definition.screens.announcement.media?.light).toEqual({
      kind: "image",
      src: "./assets/claude-mod-desktop-light-no-label-800.webp",
      poster: "./assets/claude-mod-desktop-light-no-label-poster.png",
    })
    const module = await generatePromotionModule({ production: true })
    expect(module.code).toContain("media.light.src = __promotionAsset")
    expect(module.code).toContain("media.light.poster = __promotionAsset")
    expect(module.watchFiles).toContain(
      path.join(claudeDirectory, "assets/claude-mod-desktop-light-no-label-800.webp"),
    )
    const base = { kind: "image", src: "./dark.png", alt: "Preview" }
    const withLight = (light: unknown) =>
      baseDefinition({
        screens: {
          announcement: { title: "Sample", description: "Example", media: { ...base, light } },
        },
      })
    expect(() =>
      validatePromotionDefinition(withLight({ kind: "video", src: "./light.mp4" })),
    ).toThrow("expected image")
    expect(() =>
      validatePromotionDefinition(withLight({ kind: "image", src: "../outside.png" })),
    ).toThrow("expected a relative local asset path")
    expect(() =>
      validatePromotionDefinition(withLight({ kind: "image", src: "./light.png", light: {} })),
    ).toThrow('unknown key "light"')
  })

  it("rejects media files that are missing from a promotion folder", async () => {
    const directory = await mkdtemp(path.join(os.tmpdir(), "promotion-missing-asset-"))
    try {
      const folderPath = path.join(directory, "missing")
      await mkdir(folderPath)
      await writeFile(
        path.join(folderPath, "promo.yaml"),
        `schema_version: 1\nid: missing-asset\nrevision: 1\ntitle: Missing\nscreens:\n  announcement:\n    title: Missing\n    description: Example\n    media:\n      kind: image\n      src: ./assets/missing.svg\n      alt: Missing image\n`,
      )
      await expect(generatePromotionModule({ directory, production: true })).rejects.toThrow(
        'Missing promotion asset "./assets/missing.svg"',
      )
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  it("accepts still posters and rejects animated posters for both themes", async () => {
    const directory = await mkdtemp(path.join(os.tmpdir(), "promotion-poster-"))
    try {
      const folder = path.join(directory, "sample")
      await mkdir(folder)
      const webp = Buffer.from(
        "UklGRiIAAABXRUJQVlA4IBYAAAAwAQCdASoBAAEADsD+JaQAA3AAAAAA",
        "base64",
      )
      const gif = Buffer.from(
        "R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7",
        "base64",
      )
      const frame = gif.subarray(gif.indexOf(Buffer.from([0x2c])), gif.length - 1)
      const animatedGif = Buffer.concat([
        gif.subarray(0, gif.length - 1),
        frame,
        Buffer.from([0x3b]),
      ])
      const animatedWebp = await readFile(
        path.join(claudeDirectory, "assets/claude-mod-desktop-dark-no-label-800.webp"),
      )
      const png = await readFile(
        path.join(claudeDirectory, "assets/claude-mod-desktop-dark-no-label-poster.png"),
      )
      const animatedPng = Buffer.concat([
        png.subarray(0, 33),
        Buffer.from([0, 0, 0, 8, 97, 99, 84, 76, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0]),
        png.subarray(33),
      ])
      await writeFile(path.join(folder, "source.png"), png)
      const yaml = (poster: string, light: boolean) => `schema_version: 1
id: sample
revision: 1
title: Sample
screens:
  announcement:
    title: Sample
    description: Example
    media:
      kind: image
      src: ./source.png
      alt: Example
${
  light
    ? `      light:
        kind: image
        src: ./source.png
        poster: ./${poster}
`
    : `      poster: ./${poster}
`
}`
      for (const [name, bytes, valid] of [
        ["still.gif", gif, true],
        ["still.webp", webp, true],
        ["still.png", png, true],
        ["animated.gif", animatedGif, false],
        ["animated.webp", animatedWebp, false],
        ["disguised.png", animatedWebp, false],
        ["disguised.jpg", animatedGif, false],
        ["animated.png", animatedPng, false],
      ] as const) {
        await writeFile(path.join(folder, name), bytes)
        for (const light of [false, true]) {
          await writeFile(path.join(folder, "promo.yaml"), yaml(name, light))
          const result = generatePromotionModule({ directory, production: true })
          if (valid) await expect(result).resolves.toBeDefined()
          else await expect(result).rejects.toThrow("must be a still")
        }
      }
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })

  it("rejects duplicate IDs across the catalog", async () => {
    const directory = await mkdtemp(path.join(os.tmpdir(), "promotion-catalog-"))
    try {
      const definition = `schema_version: 1\nid: repeated\nrevision: 1\ntitle: Repeated\nscreens:\n  announcement:\n    title: Repeated\n    description: Example\n`
      for (const folder of ["one", "two"]) {
        const folderPath = path.join(directory, folder)
        await mkdir(folderPath)
        await writeFile(path.join(folderPath, "promo.yaml"), definition)
      }
      await expect(generatePromotionModule({ directory, production: true })).rejects.toThrow(
        'Duplicate promotion id "repeated"',
      )
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  })
})

describe("promotion catalog module", () => {
  it("bundles Claude Mod in production and removes the dummy definition and asset", async () => {
    const module = await generatePromotionModule({ production: true })

    expect(module.ids).toEqual(["claude-mod"])
    expect(module.code).toContain("Pace your Claude Code usage across sessions.")
    expect(module.code).toContain("__promotionAsset0")
    expect(module.code).toContain("claude-mod-desktop-dark-no-label-800.webp?no-inline")
    expect(module.code).not.toContain("dummy-promo")
    expect(module.code).not.toContain("Different content")
    expect(module.code).not.toContain("dummy-promo.svg")
  })

  it("loads the dummy YAML and image through the shared catalog in development", async () => {
    const developmentDirectory = await createPromotionFixture()
    const module = await generatePromotionModule({ production: false, developmentDirectory })

    expect(module.ids).toEqual(["claude-mod", "dummy-promo"])
    expect(module.code).toContain('"development_only":true')
    expect(module.code).toContain("Rendered from YAML without feature-specific code.")
    expect(module.code).toContain("dummy-promo.svg?no-inline")
    expect(module.code).toContain('"target":"settings","pane":"usage"')
  })
})

it("accepts registered YAML analytics enrollment and rejects arbitrary tracking fields", () => {
  expect(
    validatePromotionDefinition(baseDefinition({ analytics: { campaign: "claude_mod" } }))
      .analytics,
  ).toEqual({ campaign: "claude_mod" })
  expect(validatePromotionDefinition(baseDefinition()).analytics).toBeUndefined()
  for (const analytics of [
    { campaign: "unknown" },
    { campaign: "claude_mod", event: "anything" },
    { campaign: "claude_mod", properties: { command: "private" } },
    {},
  ]) {
    expect(() => validatePromotionDefinition(baseDefinition({ analytics }))).toThrow()
  }
})

it("rejects video media, which the Linux webview cannot play safely", () => {
  const definition = baseDefinition({
    screens: {
      announcement: {
        title: "Video",
        description: "Example",
        media: { kind: "video", src: "./assets/demo.mp4", alt: "Example" },
      },
    },
  })
  expect(() => validatePromotionDefinition(definition)).toThrow("expected image")
})

it("requires a static poster for an animated image", () => {
  const animated = (src: string) =>
    baseDefinition({
      screens: {
        announcement: {
          title: "Animated",
          description: "Example",
          media: { kind: "image", src, alt: "Example" },
        },
      },
    })
  for (const src of ["./assets/demo.webp", "./assets/demo.gif"]) {
    expect(() => validatePromotionDefinition(animated(src))).toThrow(
      "an animated image requires a static poster",
    )
  }
  // A still format carries no motion, so it needs no poster.
  expect(() => validatePromotionDefinition(animated("./assets/demo.png"))).not.toThrow()
})

it("keeps unknown HTTPS destinations outside the native opener scope", () => {
  const definition = baseDefinition({
    screens: {
      announcement: {
        title: "Example",
        description: "Example",
        primary_action: { type: "external", label: "Open", url: "https://example.com/" },
      },
    },
  })
  expect(() => validatePromotionDefinition(definition)).toThrow("antiburn GitHub repository")
})

it("never reads test-only fixture directories during a production build", async () => {
  const module = await generatePromotionModule({
    production: true,
    developmentDirectory: "/missing/fixture/directory",
  })
  expect(module.ids).toEqual(["claude-mod"])
  expect(
    module.watchFiles.every(
      (file) =>
        !file.startsWith(path.resolve(process.cwd(), "../../.agent-artifacts/promotions")),
    ),
  ).toBe(true)
})

it("preserves content that resembles an internal asset binding", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "promotion-binding-"))
  try {
    const folder = path.join(directory, "sample")
    await mkdir(folder)
    await writeFile(path.join(folder, "image.svg"), '<svg xmlns="http://www.w3.org/2000/svg"/>')
    await writeFile(
      path.join(folder, "promo.yaml"),
      "schema_version: 1\nid: sample\nrevision: 1\ntitle: __promotionAsset0\nscreens:\n  announcement:\n    title: __promotionAsset0\n    description: Example\n    media:\n      kind: image\n      src: ./image.svg\n      alt: Example\n",
    )
    const module = await generatePromotionModule({ directory, production: true })
    expect(module.code).toContain('"title":"__promotionAsset0"')
    expect(module.code).toContain("media.src = __promotionAsset0")
  } finally {
    await rm(directory, { recursive: true, force: true })
  }
})

describe("promotion sidebar schema", () => {
  const sidebar = {
    title: "New feature",
    description: "A feature for everyone.",
    audience: { type: "all" },
  }

  it("keeps sidebar-free definitions compatible and normalizes sidebar defaults", () => {
    expect(validatePromotionDefinition(baseDefinition()).sidebar).toBeUndefined()
    expect(validatePromotionDefinition(baseDefinition({ sidebar })).sidebar).toEqual({
      ...sidebar,
      icon: "megaphone",
      priority: 0,
    })
  })

  it.each(["megaphone", "claude-code", "codex", "cursor"])(
    "accepts the single icon value %s independently of audience",
    (icon) => {
      expect(
        validatePromotionDefinition(baseDefinition({ sidebar: { ...sidebar, icon } })).sidebar
          ?.icon,
      ).toBe(icon)
    },
  )

  it("accepts agent targeting, optional copy, and negative priority", () => {
    const configured = {
      ...sidebar,
      icon: "megaphone",
      badge: "NEW",
      link_label: "Explore",
      priority: -10,
      audience: { type: "detected_agents", agents: ["claude-code", "codex"] },
    }
    expect(
      validatePromotionDefinition(baseDefinition({ sidebar: configured })).sidebar,
    ).toEqual(configured)
  })

  it.each([
    ["unknown field", { ...sidebar, target: "other" }],
    ["missing audience", { title: "Title", description: "Description" }],
    ["missing title", { description: "Description", audience: { type: "all" } }],
    ["blank description", { ...sidebar, description: " " }],
    ["blank badge", { ...sidebar, badge: " " }],
    ["blank link label", { ...sidebar, link_label: " " }],
    ["unknown icon", { ...sidebar, icon: "unknown-agent" }],
    ["nested icon", { ...sidebar, icon: { agent: "claude-code" } }],
    ["fractional priority", { ...sidebar, priority: 1.5 }],
    ["unsafe priority", { ...sidebar, priority: Number.MAX_SAFE_INTEGER + 1 }],
    ["string priority", { ...sidebar, priority: "10" }],
    ["null priority", { ...sidebar, priority: null }],
    ["unknown audience", { ...sidebar, audience: { type: "unknown" } }],
    [
      "agents on all audience",
      { ...sidebar, audience: { type: "all", agents: ["claude-code"] } },
    ],
    ["empty agent audience", { ...sidebar, audience: { type: "detected_agents", agents: [] } }],
    ["missing agent list", { ...sidebar, audience: { type: "detected_agents" } }],
    [
      "unknown agent",
      { ...sidebar, audience: { type: "detected_agents", agents: ["unknown"] } },
    ],
    ["unknown audience field", { ...sidebar, audience: { type: "all", surprise: true } }],
  ])("rejects %s", (_label, value) => {
    expect(() => validatePromotionDefinition(baseDefinition({ sidebar: value }))).toThrow(
      "sidebar",
    )
  })

  it("includes the production sidebar in the generated catalog", async () => {
    const module = await generatePromotionModule({ production: true })
    expect(module.code).toContain('"sidebar":{"title":"Claude Mod"')
    expect(module.code).toContain(
      '"audience":{"type":"detected_agents","agents":["claude-code"]}',
    )
  })
})

/**
 * Read the animation facts out of a WebP file.
 *
 * A WebP is RIFF: a 12-byte header, then chunks of a 4-byte name, a 4-byte
 * little-endian length, and a payload padded to an even length. `VP8X` holds
 * the flags and the canvas size, `ANIM` holds the loop count, and each `ANMF`
 * holds one frame and its duration.
 */
function readWebpAnimation(bytes: Buffer) {
  if (bytes.toString("ascii", 0, 4) !== "RIFF" || bytes.toString("ascii", 8, 12) !== "WEBP")
    throw new Error("not a WebP file")
  let animated = false
  let loops: number | null = null
  let width = 0
  let height = 0
  const durations: number[] = []
  for (let at = 12; at + 8 <= bytes.length;) {
    const name = bytes.toString("ascii", at, at + 4)
    const size = bytes.readUInt32LE(at + 4)
    const body = at + 8
    if (name === "VP8X") {
      animated = (bytes[body] & 0x02) !== 0
      width = bytes.readUIntLE(body + 4, 3) + 1
      height = bytes.readUIntLE(body + 7, 3) + 1
    }
    if (name === "ANIM") loops = bytes.readUInt16LE(body + 4)
    if (name === "ANMF") durations.push(bytes.readUIntLE(body + 12, 3))
    at = body + size + (size % 2)
  }
  return { animated, loops, width, height, durations }
}

it.each(["dark", "light"])(
  "ships the %s Claude Mod media as a looping animation, not a still frame",
  async (variant) => {
    const name = `claude-mod-desktop-${variant}-no-label-800`
    const media = readWebpAnimation(
      await readFile(path.join(claudeDirectory, `assets/${name}.webp`)),
    )
    expect(media.animated).toBe(true)
    expect(media.durations.length).toBeGreaterThan(1)
    // `0` means the animation repeats forever.
    expect(media.loops).toBe(0)
    // The encoder merges equal neighbouring frames and adds their durations,
    // so the frame count moves between encodes but the total length does not.
    expect(media.durations.reduce((whole, each) => whole + each, 0)).toBe(12_800)
    // A size change against the poster would move the panel layout.
    expect([media.width, media.height]).toEqual([800, 416])
  },
)
