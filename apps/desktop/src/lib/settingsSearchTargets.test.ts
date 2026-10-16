import { expect, it } from "vitest"
import { resolveSettingsSearchTarget, searchApp } from "./appSearch"
import { settingsControlLabel } from "./settingsSearchTargets"

it.each(["macos", "windows", "linux"] as const)(
  "keeps decision model search destinations on %s",
  (platform) => {
    expect(settingsControlLabel("smartChecksEnabled", platform)).toBe("Model-based Checks")
    for (const [query, control] of [
      ["Model-based Checks", "smartChecksEnabled"],
      ["discovery", "smartChecksEnabled"],
      ["TypeSafe API key", "typeSafeApiKey"],
    ] as const) {
      const result = searchApp(query, platform).find(
        (result) => result.target.kind === "setting" && result.target.control === control,
      )
      if (!result || result.target.kind !== "setting")
        throw new Error(`Missing search destination for ${query}`)
      expect(resolveSettingsSearchTarget(result.target)).toEqual({
        pane: "checks",
        control,
      })
    }
  },
)
