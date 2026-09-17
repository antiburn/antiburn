import { readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const read = (file: string) => readFileSync(path.join(ROOT, file), "utf8")

describe("interface scale ownership", () => {
  it.each(["InterfaceSizeControl", "ResponsiveNavigation"])(
    "keeps %s independent of application state and IPC",
    (name) => {
      const source = read(`src/components/ui/${name}.tsx`)
      const imports = [...source.matchAll(/from\s+["']([^"']+)["']/g)].map((match) => match[1]!)
      expect(
        imports.filter((value) => /ipc|settings|views|tauri|analytics/i.test(value)),
      ).toEqual([])
    },
  )

  it("uses one preset document and does not infer a preference from the display", () => {
    expect(read("src/lib/interfaceScale.ts")).toContain('"../../interface-scale.json"')
    expect(read("src-tauri/src/interface_scale.rs")).toContain('"../../interface-scale.json"')
    for (const file of [
      "src/lib/interfaceScale.ts",
      "src/lib/viewport.ts",
      "src-tauri/src/interface_scale.rs",
    ]) {
      expect(read(file)).not.toMatch(/devicePixelRatio|screen\.(width|height)|retina/i)
    }
  })

  it("keeps reusable native window crates free of the application scale controller", () => {
    for (const name of ["main-window", "anchored-window", "hud", "nudge"]) {
      expect(read(`src-tauri/crates/${name}/Cargo.toml`)).not.toMatch(/antiburn-desktop\s*=/)
    }
  })

  it("keeps HUD database and resize locks outside main-thread reconciliation", () => {
    expect(read("src-tauri/src/interface_scale.rs")).not.toContain("crate::hud::")
    const command = read("src-tauri/src/commands.rs")
      .split("pub async fn set_interface_scale(")[1]!
      .split("pub async fn restart_onboarding(")[0]!
    const worker = command.indexOf("let hud_error = run_blocking(move ||")
    const hud = command.indexOf("crate::hud::reconcile_interface_scale")
    const main = command.indexOf("crate::main_window::on_main_value")
    expect(worker).toBeGreaterThan(-1)
    expect(hud).toBeGreaterThan(worker)
    expect(main).toBeGreaterThan(hud)
  })
})
