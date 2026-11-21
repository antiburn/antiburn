/** Vendor artwork supplies both the branded image and its neutral mask. */

import ompSource from "./fixtures/omp-mark.svg?raw"

export interface VendorArtwork {
  light: string
  dark: string
  mask: string
  maskMode: "alpha" | "luminance"
}

const assets = import.meta.glob<string>("../assets/agents/*/*.svg", {
  eager: true,
  query: "?url",
  import: "default",
})
const sources = import.meta.glob<string>("../assets/agents/*/*.svg", {
  eager: true,
  query: "?raw",
  import: "default",
})

function svgUrl(source: string): string {
  return `data:image/svg+xml,${encodeURIComponent(source)}`
}

/** Remove app tiles and preserve negative space without storing a second mark. */
export function vendorMask(
  name: string,
  source: string,
): Pick<VendorArtwork, "mask" | "maskMode"> {
  if (name === "kiro") {
    return {
      mask: svgUrl(source.replace(/<rect\b[^>]*\/>/, "")),
      maskMode: "luminance",
    }
  }
  if (name === "opencode") {
    return {
      mask: svgUrl(source.replaceAll("#4B4646", "black").replaceAll("#F1ECEC", "white")),
      maskMode: "luminance",
    }
  }
  return { mask: svgUrl(source), maskMode: "alpha" }
}

const artwork = Object.fromEntries(
  Object.entries(assets)
    .filter(([path]) => path.includes("/light/"))
    .map(([path, light]) => {
      const name = path.slice(path.lastIndexOf("/") + 1, -4)
      const darkPath = path.replace("/light/", "/dark/")
      const dark = assets[darkPath]
      const source = sources[darkPath]
      if (!dark || !source) throw new Error(`Missing dark vendor artwork for ${name}`)
      return [name, { light, dark, ...vendorMask(name, source) }]
    }),
)

/** Expand the previous 40-unit glyph crop to 44 units to reduce its size by 9%. */
const ompSvg = ompSource
  .replace(/<rect\b[^>]*\/>/, "")
  .replace('viewBox="0 0 64 64"', 'viewBox="10 14 44 44"')

export const VENDOR_ARTWORK: Readonly<Record<string, VendorArtwork>> = {
  ...artwork,
  omp: { light: svgUrl(ompSvg), dark: svgUrl(ompSvg), ...vendorMask("omp", ompSvg) },
}
