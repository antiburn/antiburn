import { readFileSync, readdirSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "src")
const IMPORT_SOURCE = /from\s+["']([^"']+)["']/g

function imports(relativePath: string): string[] {
  const source = readFileSync(path.join(ROOT, relativePath), "utf8")
  return [...source.matchAll(IMPORT_SOURCE)].map((match) => match[1]!)
}

describe("Burn Check import boundaries", () => {
  it("keeps the generic dial independent of Burn Check and domain modules", () => {
    expect(imports("components/ui/SegmentedRadialDial.tsx")).toEqual([])
  })

  it("keeps the presentation adapter independent of UI", () => {
    const sources = imports("lib/presentation/burnChecks.ts")
    expect(
      sources.some((source) => source.includes("components") || source.includes("views")),
    ).toBe(false)
  })

  it("keeps Burn Check components independent of IPC and window controllers", () => {
    const directory = path.join(ROOT, "components", "burn-checks")
    const sources = readdirSync(directory)
      .filter((file) => /\.tsx?$/.test(file) && !/\.test\.tsx?$/.test(file))
      .flatMap((file) => imports(path.join("components", "burn-checks", file)))
    expect(
      sources.filter(
        (source) =>
          source.toLowerCase().includes("ipc") ||
          source.includes("/views/") ||
          source.toLowerCase().includes("controller"),
      ),
    ).toEqual([])
  })
})
