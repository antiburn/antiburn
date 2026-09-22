import { ESLint } from "eslint"
import { describe, expect, it } from "vitest"

describe("navigation registry boundary", () => {
  const filePath = "src/lib/navigation/mainViews.ts"
  const eslint = new ESLint()

  it.each([
    'import "../ipc"',
    'import type { MainWindowSectionId } from "../ipc"',
    'void import("../ipc")',
    'export type Route = import("../ipc").MainWindowSectionId',
    'export { openSettingsWindow } from "../ipc"',
    'export * from "../ipc"',
    'require("../ipc")',
  ])("rejects module dependencies: %s", async (source) => {
    const [result] = await eslint.lintText(source, { filePath })
    expect(result?.messages.some(({ ruleId }) => ruleId === "no-restricted-syntax")).toBe(true)
  })

  it.each([
    "src/lib/navigation/sessionFilterDefinitions.ts",
    "src/lib/settingsPanes.ts",
    "src/lib/presentation/checkDefinitions.ts",
    "src/lib/settingsSearchTargets.ts",
  ])("keeps feature descriptors pure: %s", async (filePath) => {
    for (const source of [
      'import "../ipc"',
      'void import("../ipc")',
      'import type { Controller } from "../Controller"',
    ]) {
      const [result] = await eslint.lintText(source, { filePath })
      expect(result?.messages.some(({ ruleId }) => ruleId === "no-restricted-syntax")).toBe(
        true,
      )
    }
  })

  it.each(["Row", "ToggleRow", "SectionGroup"])(
    "keeps %s independent of Settings",
    async (name) => {
      const filePath = `src/components/ui/${name}.tsx`
      const [result] = await eslint.lintText(
        'import type { SettingsControlId } from "../../lib/settingsSearchTargets"',
        { filePath },
      )
      expect(result?.messages.some(({ ruleId }) => ruleId === "no-restricted-imports")).toBe(
        true,
      )
    },
  )
})
