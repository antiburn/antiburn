import { existsSync, readFileSync, readdirSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

const REPOSITORY_ROOT = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  "..",
  "..",
  "..",
)
const BUNDLE = path.join(REPOSITORY_ROOT, "docs", "design", "burn-checks")

describe("Burn Check design bundle", () => {
  it("uses assessment wording and unprefixed token estimates in every HTML source", () => {
    const html = ["source", "standalone"].flatMap((directory) =>
      readdirSync(path.join(BUNDLE, directory))
        .filter((file) => file.endsWith(".html"))
        .map((file) => readFileSync(path.join(BUNDLE, directory, file), "utf8")),
    )
    for (const document of html) {
      expect(document).not.toMatch(/(?:2|two) processing|Some processing/i)
      expect(document).not.toMatch(/~\d+% token burn/)
    }
  })

  it.each(["light", "dark"])("includes the %s production-component capture", (theme) => {
    expect(
      existsSync(path.join(BUNDLE, "screenshots", `production-components-${theme}.png`)),
    ).toBe(true)
  })
})
