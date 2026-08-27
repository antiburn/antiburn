import { readFileSync, readdirSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

const DESKTOP_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const DESIGN_PATH = path.join(DESKTOP_ROOT, "design.md")

const TOKEN_DECLARATION = /^\s*(--color-[\w-]+):\s*([^;]+);/gm
const HSL_VALUE =
  /^hsl\((-?\d+(?:\.\d+)?) (\d+(?:\.\d+)?)% (\d+(?:\.\d+)?)%(?: \/ (\d+(?:\.\d+)?))?\)$/
const DOCUMENTED_THEME_VALUE = /^\s+(?:light|dark):\s+"([^"]+)"/gm
const DOCUMENTED_ALTERNATE_VALUE =
  /#\s*(?:reduced-transparency|@media\s+(?:light|dark)):\s*(.+)$/gm
const VARIABLE_REFERENCE = /^var\(--[\w-]+\)$/
const VARIABLE_FALLBACK = /^var\(--[\w-]+,\s*(.+)\)$/
const SYSTEM_COLORS = new Set([
  "accentcolor",
  "accentcolortext",
  "activetext",
  "buttonborder",
  "buttonface",
  "buttontext",
  "canvas",
  "canvastext",
  "field",
  "fieldtext",
  "graytext",
  "highlight",
  "highlighttext",
  "linktext",
  "mark",
  "marktext",
  "selecteditem",
  "selecteditemtext",
  "visitedtext",
  "-apple-system-label",
  "-apple-system-secondary-label",
  "-apple-system-tertiary-label",
  "-apple-system-separator",
  "-apple-system-accent-color",
])

type Hsl = {
  hue: number
  saturation: number
  lightness: number
  alpha?: number
}

type Candidate = {
  value: string
  length: number
  decimals: number
}

function cssFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(directory, entry.name)
    if (entry.isDirectory()) return cssFiles(entryPath)
    return entry.isFile() && entry.name.endsWith(".css") ? [entryPath] : []
  })
}

const TOKEN_STYLE_PATHS = cssFiles(path.join(DESKTOP_ROOT, "src")).sort()

function decimalCount(value: string): number {
  const point = value.indexOf(".")
  return point === -1 ? 0 : value.length - point - 1
}

function formatNumber(value: number, precision: number): string {
  const rounded = Number(value.toFixed(precision))
  return String(Object.is(rounded, -0) ? 0 : rounded)
}

function nearbyValues(value: number, precision: number, maximum: number): string[] {
  const scale = 10 ** precision
  const units = [Math.floor(value * scale), Math.round(value * scale), Math.ceil(value * scale)]
  return [
    ...new Set(units.map((unit) => formatNumber(Math.min(maximum, unit / scale), precision))),
  ]
}

function nearbyHues(value: number, precision: number): string[] {
  const scale = 10 ** precision
  const units = [Math.floor(value * scale), Math.round(value * scale), Math.ceil(value * scale)]
  return [
    ...new Set(
      units.map((unit) => {
        const hue = (((unit / scale) % 360) + 360) % 360
        return formatNumber(hue, precision)
      }),
    ),
  ]
}

function rgbToHsl(red: number, green: number, blue: number): [number, number, number] {
  const r = red / 255
  const g = green / 255
  const b = blue / 255
  const maximum = Math.max(r, g, b)
  const minimum = Math.min(r, g, b)
  const delta = maximum - minimum
  const lightness = (maximum + minimum) / 2

  if (delta === 0) return [0, 0, lightness * 100]

  const saturation = delta / (1 - Math.abs(2 * lightness - 1))
  let hue: number

  if (maximum === r) {
    hue = 60 * (((g - b) / delta) % 6)
  } else if (maximum === g) {
    hue = 60 * ((b - r) / delta + 2)
  } else {
    hue = 60 * ((r - g) / delta + 4)
  }

  if (hue < 0) hue += 360
  return [hue, saturation * 100, lightness * 100]
}

function hslToRgb(
  hue: number,
  saturation: number,
  lightness: number,
): [number, number, number] {
  const s = saturation / 100
  const l = lightness / 100
  const chroma = (1 - Math.abs(2 * l - 1)) * s
  const secondary = chroma * (1 - Math.abs(((hue / 60) % 2) - 1))
  const match = l - chroma / 2
  let channels: [number, number, number]

  if (hue < 60) {
    channels = [chroma, secondary, 0]
  } else if (hue < 120) {
    channels = [secondary, chroma, 0]
  } else if (hue < 180) {
    channels = [0, chroma, secondary]
  } else if (hue < 240) {
    channels = [0, secondary, chroma]
  } else if (hue < 300) {
    channels = [secondary, 0, chroma]
  } else {
    channels = [chroma, 0, secondary]
  }

  return channels.map((channel) => Math.round((channel + match) * 255)) as [
    number,
    number,
    number,
  ]
}

function parseHsl(value: string): Hsl | null {
  const match = HSL_VALUE.exec(value)
  if (!match) return null

  const hue = Number(match[1])
  const saturation = Number(match[2])
  const lightness = Number(match[3])
  const alpha = match[4] === undefined ? undefined : Number(match[4])

  if (hue < 0 || hue >= 360) return null
  if (saturation < 0 || saturation > 100) return null
  if (lightness < 0 || lightness > 100) return null
  if (alpha !== undefined && (alpha < 0 || alpha > 1)) return null

  return { hue, saturation, lightness, alpha }
}

function canonicalHsl(red: number, green: number, blue: number): string | null {
  const exact = rgbToHsl(red, green, blue)
  const expected = `${red},${green},${blue}`
  const candidates: Candidate[] = []

  for (let huePrecision = 0; huePrecision <= 1; huePrecision += 1) {
    for (let saturationPrecision = 0; saturationPrecision <= 2; saturationPrecision += 1) {
      for (let lightnessPrecision = 0; lightnessPrecision <= 2; lightnessPrecision += 1) {
        const hues = nearbyHues(exact[0], huePrecision)
        const saturations = nearbyValues(exact[1], saturationPrecision, 100)
        const lightnesses = nearbyValues(exact[2], lightnessPrecision, 100)

        for (const hue of hues) {
          for (const saturation of saturations) {
            for (const lightness of lightnesses) {
              const parts = [hue, saturation, lightness]
              if (
                hslToRgb(...(parts.map(Number) as [number, number, number])).join() !== expected
              ) {
                continue
              }

              const value = `hsl(${hue} ${saturation}% ${lightness}%)`
              candidates.push({
                value,
                length: value.length,
                decimals: parts.reduce((total, part) => total + decimalCount(part), 0),
              })
            }
          }
        }
      }
    }
  }

  candidates.sort(
    (left, right) =>
      left.length - right.length ||
      left.decimals - right.decimals ||
      left.value.localeCompare(right.value),
  )
  return candidates[0]?.value ?? null
}

function canonicalize(value: string): string | null {
  const parsed = parseHsl(value)
  if (!parsed) return null

  const color = canonicalHsl(...hslToRgb(parsed.hue, parsed.saturation, parsed.lightness))
  if (!color) return null
  if (parsed.alpha === undefined || parsed.alpha === 1) return color

  return `${color.slice(0, -1)} / ${parsed.alpha})`
}

function concreteColorValues(value: string): string[] {
  const trimmed = value.trim()
  if (VARIABLE_REFERENCE.test(trimmed)) return []

  const fallback = VARIABLE_FALLBACK.exec(trimmed)
  return fallback ? concreteColorValues(fallback[1]) : [trimmed]
}

function isSystemColor(value: string): boolean {
  return SYSTEM_COLORS.has(value.toLowerCase())
}

function tokenValues(): Array<{ source: string; name: string; value: string }> {
  return TOKEN_STYLE_PATHS.flatMap((file) => {
    const source = path.relative(DESKTOP_ROOT, file)
    const contents = readFileSync(file, "utf8")
    return [...contents.matchAll(TOKEN_DECLARATION)].map((match) => ({
      source,
      name: match[1],
      value: match[2].trim(),
    }))
  })
}

function designColors(): string {
  const design = readFileSync(DESIGN_PATH, "utf8")
  const start = design.indexOf("colors:")
  const end = design.indexOf("fonts:")
  if (start === -1 || end === -1 || end <= start)
    throw new Error("The design color contract is missing")
  return design.slice(start, end)
}

function documentedColorValues(): string[] {
  const colors = designColors()
  const themes = [...colors.matchAll(DOCUMENTED_THEME_VALUE)].map((match) => match[1].trim())
  const alternates = [...colors.matchAll(DOCUMENTED_ALTERNATE_VALUE)].map((match) =>
    match[1].trim(),
  )
  return [...themes, ...alternates]
}

describe("the design-token color convention", () => {
  it("uses HSL for every concrete token color", () => {
    const values = tokenValues()
    const violations = values.flatMap(({ source, name, value }) =>
      concreteColorValues(value).flatMap((concrete) =>
        concrete.startsWith("hsl(") || isSystemColor(concrete)
          ? []
          : [`${source}: ${name} uses ${concrete}`],
      ),
    )

    expect(values.length).toBeGreaterThan(100)
    expect(violations).toEqual([])
  })

  it("uses canonical HSL values with bounded precision", () => {
    const concreteValues = tokenValues()
      .flatMap(({ value }) => concreteColorValues(value))
      .filter((value) => value.startsWith("hsl("))
    const documentedValues = documentedColorValues()
    const violations = [...concreteValues, ...documentedValues].flatMap((value) => {
      const parsed = HSL_VALUE.exec(value)
      if (!parsed) return [`${value}: use modern space-separated HSL syntax`]

      const precisions = [
        decimalCount(parsed[1]),
        decimalCount(parsed[2]),
        decimalCount(parsed[3]),
      ]
      if (precisions[0] > 1 || precisions[1] > 2 || precisions[2] > 2) {
        return [`${value}: exceeds the HSL precision limits`]
      }
      if (parsed[4] !== undefined && decimalCount(parsed[4]) > 3) {
        return [`${value}: exceeds the alpha precision limit`]
      }

      const canonical = canonicalize(value)
      return canonical === value ? [] : [`${value}: use ${canonical ?? "a valid HSL value"}`]
    })

    expect(concreteValues.length).toBeGreaterThan(100)
    expect(documentedValues.length).toBeGreaterThan(50)
    expect(violations).toEqual([])
  })

  it("keeps the documented palette free of RGB and hexadecimal colors", () => {
    const colors = designColors()
    const documentedValues = documentedColorValues()

    expect(documentedValues.length).toBeGreaterThan(50)
    expect(documentedValues.every((value) => value.startsWith("hsl("))).toBe(true)
    expect(colors).not.toMatch(/rgba?\(/i)
    expect(colors).not.toMatch(/#[\da-f]{3,8}\b/i)
  })

  it("normalizes equivalent syntax to one representation", () => {
    expect(canonicalize("hsl(211.2 100% 50%)")).toBe("hsl(211.2 100% 50%)")
    expect(canonicalize("hsl(211.30 100.0% 50.00%)")).toBe("hsl(211.2 100% 50%)")
    expect(canonicalize("hsl(211.333 100% 50%)")).toBe("hsl(211.2 100% 50%)")
    expect(canonicalize("hsl(211.3 100% 50% / 1.000)")).toBe("hsl(211.2 100% 50%)")
    expect(canonicalize("hsl(33.8 100% 31.4%)")).toBe("hsl(34 100% 31.3%)")
    expect(canonicalize("hsl(135 70% 52.35%)")).toBe("hsl(135 70% 52.3%)")
    expect(canonicalize("hsl(252.5 94.7% 85%)")).toBe("hsl(253 94% 85%)")
    expect(canonicalize("hsl(211.3, 100%, 50%)")).toBeNull()
    expect(canonicalize("rgb(0 122 255)")).toBeNull()
  })

  it("checks concrete variable fallbacks", () => {
    expect(concreteColorValues("var(--color-accent)")).toEqual([])
    expect(concreteColorValues("var(--missing, hsl(211.3 100% 50%))")).toEqual([
      "hsl(211.3 100% 50%)",
    ])
    expect(concreteColorValues("var(--missing, rgb(0 122 255))")).toEqual(["rgb(0 122 255)"])
  })
})
