import { afterEach, describe, expect, it, vi } from "vitest"

import { analyticsDefaultsOff } from "./euLocale"

/** Pretend to be a machine configured for `languages` in `zone`. */
function machine(languages: string[], zone: string) {
  vi.spyOn(navigator, "languages", "get").mockReturnValue(languages)
  vi.spyOn(navigator, "language", "get").mockReturnValue(languages[0] ?? "en")
  vi.spyOn(Intl, "DateTimeFormat").mockReturnValue({
    resolvedOptions: () => ({ timeZone: zone }),
  } as unknown as Intl.DateTimeFormat)
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe("where analytics must be opted into rather than out of", () => {
  it("starts off across the EU, the EEA, and the UK", () => {
    for (const [tag, zone] of [
      ["de-DE", "Europe/Berlin"],
      ["fr-FR", "Europe/Paris"],
      ["en-IE", "Europe/Dublin"],
      ["en-GB", "Europe/London"],
      ["nb-NO", "Europe/Oslo"],
      ["is-IS", "Atlantic/Reykjavik"],
    ] as const) {
      machine([tag], zone)
      expect(analyticsDefaultsOff(), tag).toBe(true)
    }
  })

  it("starts on elsewhere", () => {
    for (const [tag, zone] of [
      ["en-US", "America/New_York"],
      ["ja-JP", "Asia/Tokyo"],
      ["en-AU", "Australia/Sydney"],
      ["pt-BR", "America/Sao_Paulo"],
    ] as const) {
      machine([tag], zone)
      expect(analyticsDefaultsOff(), tag).toBe(false)
    }
  })

  /// A bare `de` names no region, so the time zone is the only evidence left.
  /// Over-reaching here is the direction to over-reach in.
  it("falls back to the time zone when the locale names no region", () => {
    machine(["de"], "Europe/Vienna")
    expect(analyticsDefaultsOff()).toBe(true)

    machine(["de"], "America/Chicago")
    expect(analyticsDefaultsOff()).toBe(false)
  })

  /// A script subtag must not be mistaken for a region.
  it("reads past a script subtag to the real region", () => {
    machine(["zh-Hant-TW"], "Asia/Taipei")
    expect(analyticsDefaultsOff()).toBe(false)
  })

  /// A secondary preferred language still counts: the reader said it.
  it("honours any configured language, not just the first", () => {
    machine(["en-US", "fr-FR"], "America/New_York")
    expect(analyticsDefaultsOff()).toBe(true)
  })
})
