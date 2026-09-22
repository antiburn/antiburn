import { readFileSync } from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createContext, runInContext } from "node:vm"

import { describe, expect, it } from "vitest"

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const read = (file: string) => readFileSync(path.join(ROOT, file), "utf8")

function scaleScript(name: "initialization_script" | "update_script", percent: number) {
  const body = read("src-tauri/src/interface_scale.rs")
    .split(`fn ${name}(scale: InterfaceScale) -> String {`)[1]!
    .split("\n}")[0]!
  const template = body.match(/r#"([\s\S]*?)"#/)?.[1]
  expect(template).toBeDefined()
  return template!
    .replaceAll("{percent}", String(percent))
    .replaceAll("{factor}", String(percent / 100))
    .replaceAll("{CHANGED_EVENT}", "antiburn:interface-scale-changed")
    .replaceAll("{{", "{")
    .replaceAll("}}", "}")
}

describe("native scale initialization ordering", () => {
  const initializers = [
    "shell",
    "src-tauri/crates/hud/src/lib.rs",
    "src-tauri/crates/nudge/src/window.rs",
    "src-tauri/crates/anchored-window/src/manager/window.rs",
    "src-tauri/crates/anchored-window/src/macos/native.rs",
  ]

  it.each(initializers)("keeps the latest scale after DOMContentLoaded: %s", (owner) => {
    for (const percent of [90, 100, 110, 125, 150, 175, 200]) {
      for (const readyFirst of [false, true]) {
        const properties = new Map<string, string>()
        let ready: (() => void) | undefined
        const context = createContext({
          document: {
            documentElement: {
              style: {
                setProperty: (key: string, value: string) => properties.set(key, value),
              },
            },
            addEventListener: (_name: string, listener: () => void) => {
              ready = listener
            },
          },
          dispatchEvent() {},
          CustomEvent: class {},
        })
        let initial = scaleScript("initialization_script", 100)
        if (owner !== "shell") {
          const literal = read(owner)
            .split("\n")
            .find(
              (line) =>
                line.includes("DOMContentLoaded") &&
                line.includes("__ANTIBURN_INTERFACE_SCALE_PERCENT__"),
            )
          expect(literal).toBeDefined()
          initial = (JSON.parse(literal!.trim().replace(/,$/, "")) as string)
            .replaceAll("{}", "100")
            .replaceAll("{renderer_generation}", "1")
            .replaceAll("{interface_scale}", "1")
            .replaceAll("{{", "{")
            .replaceAll("}}", "}")
        }
        runInContext(initial, context)
        expect(ready).toBeDefined()
        if (readyFirst) ready!()
        runInContext(scaleScript("update_script", percent), context)
        if (!readyFirst) ready!()
        expect(context.__ANTIBURN_INTERFACE_SCALE_PERCENT__).toBe(percent)
        expect(properties.get("--interface-scale")).toBe(String(percent / 100))
      }
    }
  })
})

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
