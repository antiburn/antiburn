/**
 * Brand marks that `simple-icons` does not carry.
 *
 * `simple-icons` supplies most of the agent marks, and where it does it is the
 * only source used — the package is a dependency, so its path data and licence
 * travel with the lockfile and the SBOM. This module is the narrow exception:
 * artwork inlined by hand because no runtime dependency can supply it at a
 * sane cost.
 *
 * Vendored artwork must carry source and licence evidence. Each entry below records its
 * collection or product asset, licence status, and icon name. Package-backed
 * marks have source-equivalence tests. Product assets record a source checksum
 * and pin the extracted path in `brandMarks.test.ts`.
 *
 * Inlining rather than importing is deliberate: `@iconify-json/logos` ships a
 * single 7.4 MB `icons.json` with no per-icon entry point, so importing it
 * would put the whole collection in the bundle to draw one mark. It is a
 * devDependency — present for the provenance test, absent from the app.
 *
 * Marks remain their owners' trademarks. This nominative use has the same
 * footing as the `simple-icons` marks.
 */

import type { SimpleIcon } from "simple-icons"

/**
 * One brand mark, normalized across sources.
 *
 * Mirrors the fields of `simple-icons`' `SimpleIcon` that this app uses, plus
 * the `viewBox` that package hard-codes to `0 0 24 24` and other collections
 * do not.
 */
export interface BrandMark {
  /** The `d` attribute of a single path. */
  path: string
  /** The coordinate system `path` is drawn in. */
  viewBox: string
  /**
   * The brand's own published colour, without the leading `#`. Only read for
   * marks that opt into brand colour; ink-rendered marks may omit it.
   */
  hex?: string
  /** Where the artwork came from, for the reader and for the test. */
  provenance: {
    /** Package version or product asset that supplied the path data. */
    package: string
    /** The icon's name within that package. */
    icon: string
    /** SPDX identifier of the collection's licence. */
    license: string
    /** The collection's upstream home. */
    source: string
    /** SHA-256 of a source asset when no versioned package supplies it. */
    assetSha256?: string
  }
}

/** Normalize a `simple-icons` entry into this module's mark shape. */
export function fromSimpleIcons(icon: SimpleIcon): BrandMark {
  return {
    path: icon.path,
    // The package draws every icon in a 24×24 box.
    viewBox: "0 0 24 24",
    hex: icon.hex,
    provenance: {
      package: "simple-icons",
      icon: icon.title,
      license: "CC0-1.0",
      source: icon.source,
    },
  }
}

/**
 * OpenAI's mark, used nominatively on Codex rows and OpenAI usage figures.
 *
 * `simple-icons@16` carries no OpenAI icon — its only near match is
 * `OpenAI Gym`, a different and discontinued product, which would put the
 * wrong mark on the row. Taken instead from Gil Barbara's SVG Logos, released
 * under CC0-1.0 as a collection.
 */
export const OPENAI_MARK: BrandMark = {
  path: "M239.184 106.203a64.72 64.72 0 0 0-5.576-53.103C219.452 28.459 191 15.784 163.213 21.74A65.586 65.586 0 0 0 52.096 45.22a64.72 64.72 0 0 0-43.23 31.36c-14.31 24.602-11.061 55.634 8.033 76.74a64.67 64.67 0 0 0 5.525 53.102c14.174 24.65 42.644 37.324 70.446 31.36a64.72 64.72 0 0 0 48.754 21.744c28.481.025 53.714-18.361 62.414-45.481a64.77 64.77 0 0 0 43.229-31.36c14.137-24.558 10.875-55.423-8.083-76.483m-97.56 136.338a48.4 48.4 0 0 1-31.105-11.255l1.535-.87l51.67-29.825a8.6 8.6 0 0 0 4.247-7.367v-72.85l21.845 12.636c.218.111.37.32.409.563v60.367c-.056 26.818-21.783 48.545-48.601 48.601M37.158 197.93a48.35 48.35 0 0 1-5.781-32.589l1.534.921l51.722 29.826a8.34 8.34 0 0 0 8.441 0l63.181-36.425v25.221a.87.87 0 0 1-.358.665l-52.335 30.184c-23.257 13.398-52.97 5.431-66.404-17.803M23.549 85.38a48.5 48.5 0 0 1 25.58-21.333v61.39a8.29 8.29 0 0 0 4.195 7.316l62.874 36.272l-21.845 12.636a.82.82 0 0 1-.767 0L41.353 151.53c-23.211-13.454-31.171-43.144-17.804-66.405zm179.466 41.695l-63.08-36.63L161.73 77.86a.82.82 0 0 1 .768 0l52.233 30.184a48.6 48.6 0 0 1-7.316 87.635v-61.391a8.54 8.54 0 0 0-4.4-7.213m21.742-32.69l-1.535-.922l-51.619-30.081a8.39 8.39 0 0 0-8.492 0L99.98 99.808V74.587a.72.72 0 0 1 .307-.665l52.233-30.133a48.652 48.652 0 0 1 72.236 50.391zM88.061 139.097l-21.845-12.585a.87.87 0 0 1-.41-.614V65.685a48.652 48.652 0 0 1 79.757-37.346l-1.535.87l-51.67 29.825a8.6 8.6 0 0 0-4.246 7.367zm11.868-25.58L128.067 97.3l28.188 16.218v32.434l-28.086 16.218l-28.188-16.218z",
  viewBox: "0 0 256 260",
  provenance: {
    package: "@iconify-json/logos@1.2.12",
    icon: "openai-icon",
    license: "CC0-1.0",
    source: "https://github.com/gilbarbara/logos",
  },
}

/**
 * Antigravity's monochrome product silhouette.
 *
 * The path is the mask from Google's multicolor product asset. The desktop
 * menu uses the same silhouette as a monochrome mark.
 */
export const ANTIGRAVITY_MARK: BrandMark = {
  path: "m21.751 22.607c1.34 1.005 3.35.335 1.508-1.508-5.529-5.359-4.355-20.099-11.222-20.099s-5.695 14.74-11.222 20.1c-2.01 2.009.167 2.511 1.507 1.506 5.192-3.517 4.857-9.714 9.715-9.714 4.857 0 4.522 6.197 9.714 9.715z",
  viewBox: "0 0 24 24",
  provenance: {
    package: "src/lib/fixtures/antigravity-mark.svg",
    icon: "Antigravity",
    license: "LicenseRef-Trademark",
    source: "https://antigravity.google/",
    assetSha256: "37bf1d6e27179dcf8b0e46b18bd65a38ff555e58cd6a6156902784a92a905628",
  },
}
