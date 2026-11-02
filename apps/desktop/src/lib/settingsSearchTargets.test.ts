import { expect, it } from "vitest"
import { resolveSettingsSearchTarget, searchApp } from "./appSearch"
import { settingsControlLabel } from "./settingsSearchTargets"

it.each(["macos", "windows", "linux"] as const)(
  "keeps model-based check search destinations on %s",
  (platform) => {
    expect(settingsControlLabel("smartChecksEnabled", platform)).toBe("Run checks")
    for (const [query, control] of [
      ["Run checks", "smartChecksEnabled"],
      ["Model-based checks", "smartChecksEnabled"],
      ["discovery", "checkDirectories"],
      ["check folders", "checkDirectories"],
      ["model usage", "checkModelUsage"],
      ["TypeSafe API key", "typeSafeApiKey"],
      ["Jev API key", "typeSafeApiKey"],
      ["decision model", "decisionProvider"],
      ["saved connections", "decisionConnection"],
      ["Ollama URL", "ollamaUrl"],
      ["Ollama model", "ollamaModel"],
      ["Ollama API key", "ollamaApiKey"],
      ["Cloudflare account ID", "cloudflareAccountId"],
      ["Cloudflare API token", "cloudflareApiToken"],
      ["clef-flash", "cloudflareModel"],
      ["Custom endpoint", "customUrl"],
      ["Custom model", "customModel"],
      ["Custom API key", "customApiKey"],
      ["response mode", "customResponseMode"],
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

it("does not keep removed model-limit destinations", () => {
  for (const query of ["manual limits", "total input tokens", "loaded context tokens"])
    expect(
      searchApp(query, "macos").filter((result) => result.target.kind === "setting"),
    ).toEqual([])
})
