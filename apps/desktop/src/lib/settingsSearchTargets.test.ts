import { expect, it } from "vitest"
import { resolveSettingsSearchTarget, searchApp } from "./appSearch"
import { settingsControlLabel } from "./settingsSearchTargets"

it.each(["macos", "windows", "linux"] as const)(
  "keeps decision model search destinations on %s",
  (platform) => {
    expect(settingsControlLabel("smartChecksEnabled", platform)).toBe(
      "Enable smart burn checks",
    )
    expect(settingsControlLabel("smartCheckProvider", platform)).toBe("Provider")
    for (const [query, control] of [
      ["Enable smart burn checks", "smartChecksEnabled"],
      ["Use Ollama or another provider", "smartCheckProvider"],
      ["decision model", "smartCheckProvider"],
      ["saved connections", "smartCheckProvider"],
      ["manual limits", "smartCheckLimits"],
      ["TypeSafe API key", "typeSafeApiKey"],
      ["Cloudflare API token", "typeSafeApiKey"],
      ["provider credential", "typeSafeApiKey"],
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
