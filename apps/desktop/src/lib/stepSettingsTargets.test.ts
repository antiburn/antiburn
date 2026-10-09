import { expect, it } from "vitest"
import { resolveStepSettingsSearchTarget, searchApp } from "./appSearch"
import { stepSettingsControlLabel } from "./stepSettingsTargets"

it.each(["macos", "windows", "linux"] as const)(
  "keeps decision model search destinations on %s",
  (platform) => {
    expect(stepSettingsControlLabel("smartChecksEnabled")).toBe("Enable smart burn checks")
    expect(stepSettingsControlLabel("smartCheckProvider")).toBe("Provider")
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
        (result) => result.target.kind === "stepSetting" && result.target.control === control,
      )
      if (!result || result.target.kind !== "stepSetting")
        throw new Error(`Missing search destination for ${query}`)
      expect(resolveStepSettingsSearchTarget(result.target)).toEqual({
        step: "checks",
        control,
      })
    }
  },
)
