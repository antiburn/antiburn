import { describe, expect, it } from "vitest"

import ompSource from "./fixtures/omp-mark.svg?raw"
import provenance from "../assets/agents/provenance.json"
import { VENDOR_ARTWORK, vendorMask } from "./vendorArtwork"
import { OMP_MARK } from "./brandMarks"

const sources = import.meta.glob<string>("../assets/agents/*/*.svg", {
  eager: true,
  query: "?raw",
  import: "default",
})

const urls = import.meta.glob<string>("../assets/agents/*/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
})

describe("vendor artwork provenance", () => {
  it("pins each copied asset to its recorded source", async () => {
    expect(provenance.source).toBe("https://github.com/TeamCadence/cadence-app")
    expect(provenance.revision).toMatch(/^[a-f0-9]{40}$/)
    expect(Object.keys(sources)).toHaveLength(Object.keys(provenance.sha256).length)
    for (const [file, checksum] of Object.entries(provenance.sha256)) {
      const source = sources[`../assets/agents/${file}`]!
      const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(source))
      const actual = Array.from(new Uint8Array(digest), (byte) =>
        byte.toString(16).padStart(2, "0"),
      ).join("")
      expect(actual, file).toBe(checksum)
      const svg = new DOMParser().parseFromString(source, "image/svg+xml")
      expect(svg.querySelector("parsererror"), file).toBeNull()
      expect(svg.documentElement.tagName, file).toBe("svg")
      expect(svg.querySelector("script, foreignObject, image"), file).toBeNull()
    }
  })

  it("maps each vendor's light and dark artwork to its own asset", () => {
    expect(Object.keys(VENDOR_ARTWORK)).toHaveLength(12)
    for (const [name, artwork] of Object.entries(VENDOR_ARTWORK)) {
      if (name === "omp") continue
      expect(artwork.light, name).toBe(urls[`../assets/agents/light/${name}.svg`])
      expect(artwork.dark, name).toBe(urls[`../assets/agents/dark/${name}.svg`])
    }
  })
})

describe("derived vendor masks", () => {
  function decode(mask: string) {
    return decodeURIComponent(mask.slice(mask.indexOf(",") + 1))
  }

  it("uses the source SVG for every neutral mark", () => {
    for (const [path, source] of Object.entries(sources)) {
      if (!path.includes("/dark/")) continue
      const name = path.slice(path.lastIndexOf("/") + 1, -4)
      expect(VENDOR_ARTWORK[name]).toMatchObject(vendorMask(name, source))
      const original = new DOMParser().parseFromString(source, "image/svg+xml")
      const derived = new DOMParser().parseFromString(
        decode(VENDOR_ARTWORK[name]!.mask),
        "image/svg+xml",
      )
      expect(derived.querySelector("parsererror")).toBeNull()
      expect(
        Array.from(derived.querySelectorAll("path"), (path) => path.getAttribute("d")),
      ).toEqual(Array.from(original.querySelectorAll("path"), (path) => path.getAttribute("d")))
      expect(derived.documentElement.getAttribute("viewBox")).toBe(
        original.documentElement.getAttribute("viewBox"),
      )
    }
  })

  it("preserves Oh My Pi's official gradient with smaller, centered artwork", () => {
    const source = new DOMParser().parseFromString(ompSource, "image/svg+xml")
    const svg = new DOMParser().parseFromString(
      decode(VENDOR_ARTWORK.omp!.light),
      "image/svg+xml",
    )
    expect(svg.querySelector("rect")).toBeNull()
    expect(svg.documentElement.getAttribute("viewBox")).toBe("10 14 44 44")
    const [oldX, oldY, oldWidth, oldHeight] = OMP_MARK.viewBox.split(" ").map(Number)
    expect(oldWidth! / 44).toBeCloseTo(0.90909)
    expect(oldHeight! / 44).toBeCloseTo(0.90909)
    expect([10 + 44 / 2, 14 + 44 / 2]).toEqual([oldX! + oldWidth! / 2, oldY! + oldHeight! / 2])
    expect(svg.querySelector("path")?.getAttribute("d")).toBe(
      source.querySelector("path")?.getAttribute("d"),
    )
    expect(svg.querySelector("path")?.getAttribute("fill")).toBe("url(#g)")
    expect(
      Array.from(svg.querySelectorAll("stop"), (stop) => stop.getAttribute("stop-color")),
    ).toEqual(["#ed4abf", "#9b4dff", "#5ad8e6"])
    expect(VENDOR_ARTWORK.omp!.dark).toBe(VENDOR_ARTWORK.omp!.light)
    expect(VENDOR_ARTWORK.omp!.mask).toBe(VENDOR_ARTWORK.omp!.light)
    expect(VENDOR_ARTWORK.omp!.maskMode).toBe("alpha")
  })

  it("keeps the Kiro mask and Amp wordmark unboxed", () => {
    for (const name of ["kiro", "amp"]) {
      const svg = new DOMParser().parseFromString(
        decode(VENDOR_ARTWORK[name]!.mask),
        "image/svg+xml",
      )
      expect(svg.querySelector("rect")).toBeNull()
      expect(svg.querySelector("path")).not.toBeNull()
    }
    expect(decode(VENDOR_ARTWORK.kiro!.mask)).toContain('fill="black"')
    expect(VENDOR_ARTWORK.kiro!.maskMode).toBe("luminance")
    expect(decode(VENDOR_ARTWORK.amp!.mask)).not.toContain(' filter="')
  })

  it("keeps OpenCode's interior as negative space", () => {
    const mask = decode(VENDOR_ARTWORK.opencode!.mask)
    const svg = new DOMParser().parseFromString(mask, "image/svg+xml")
    expect(
      Array.from(svg.querySelectorAll("g[mask] > path"), (path) => [
        path.getAttribute("d"),
        path.getAttribute("fill"),
      ]),
    ).toEqual([
      ["M180 240H60V120H180V240Z", "black"],
      ["M180 60H60V240H180V60ZM240 300H0V0H240V300Z", "white"],
    ])
    expect(VENDOR_ARTWORK.opencode!.maskMode).toBe("luminance")
  })

  it("uses Devin's current official hexagonal symbol instead of the Windsurf mark", () => {
    const light = new DOMParser().parseFromString(
      sources["../assets/agents/light/windsurf.svg"]!,
      "image/svg+xml",
    )
    const dark = new DOMParser().parseFromString(
      sources["../assets/agents/dark/windsurf.svg"]!,
      "image/svg+xml",
    )
    expect(provenance.officialOverrides.windsurf.source).toBe(
      "https://devin.ai/assets/brand/logos/devin/symbol-light.svg",
    )
    expect(provenance.officialOverrides.windsurf.darkSource).toBe(
      "https://devin.ai/assets/brand/logos/devin/symbol-dark.svg",
    )
    for (const svg of [light, dark]) {
      expect(svg.documentElement.getAttribute("viewBox")).toBe("0 0 128 128")
      expect(svg.querySelector("rect")).toBeNull()
      expect(svg.querySelectorAll("path")).toHaveLength(1)
      expect(svg.querySelector("path")?.getAttribute("d")).toMatch(/^M23 48\.6315V29\./)
    }
    expect(light.querySelector("path")?.getAttribute("fill")).toBe("black")
    expect(dark.querySelector("path")?.getAttribute("fill")).toBe("white")
    expect(light.querySelector("path")?.getAttribute("d")).toBe(
      dark.querySelector("path")?.getAttribute("d"),
    )
    expect(VENDOR_ARTWORK.windsurf!.maskMode).toBe("alpha")
  })

  it("preserves Pi's official colors and derives its neutral alpha mask", () => {
    const source = sources["../assets/agents/dark/pi.svg"]!
    expect(sources["../assets/agents/light/pi.svg"]).toBe(source)
    const svg = new DOMParser().parseFromString(source, "image/svg+xml")
    expect(svg.documentElement.getAttribute("viewBox")).toBe("0 0 800 800")
    expect(
      Array.from(svg.querySelectorAll("path"), (path) => path.getAttribute("fill")),
    ).toEqual(["#F09082", "#4D9ABF", "#F1BE58"])
    expect(svg.querySelector("rect")).toBeNull()
    expect(decode(VENDOR_ARTWORK.pi!.mask)).toBe(source)
    expect(VENDOR_ARTWORK.pi!.maskMode).toBe("alpha")
    expect(provenance.officialOverrides.pi.source).toBe("https://pi.dev/logo-auto.svg")
  })

  it("pins the official colored app icons separately from Cadence", () => {
    expect(provenance.officialOverrides.kiro.source).toMatch(/^https:\/\/kiro\.dev\//)
    expect(Object.keys(provenance.officialOverrides)).toEqual(["kiro", "windsurf", "pi"])
    for (const [name, entry] of Object.entries(provenance.officialOverrides)) {
      expect(
        provenance.sha256[`${"light"}/${name}.svg` as keyof typeof provenance.sha256],
      ).toBe(entry.sha256)
      expect(provenance.sha256[`${"dark"}/${name}.svg` as keyof typeof provenance.sha256]).toBe(
        "darkSha256" in entry ? entry.darkSha256 : entry.sha256,
      )
    }
  })
})
