import { describe, expect, it } from "vitest"
import { APP_SEARCH_CATALOG, groupAppResults, searchApp } from "./appSearch"
import { SETTINGS_PANES } from "./settingsPanes"
import { CHECK_LABELS } from "./presentation/checks"
import { AGENT_SLUGS } from "./presentation/agents"
import {
  SETTINGS_SEARCH_TARGETS,
  parseSettingsSearchRequest,
  settingsSearchRequest,
} from "./settingsSearchTargets"

describe("static app search", () => {
  it("keeps the top-level Sessions destination searchable", () => {
    expect(searchApp("Sessions")[0]).toMatchObject({
      id: "activity",
      target: { kind: "view", section: "activity", filter: { kind: "all" } },
    })
  })
  it.each(["macos", "windows", "linux"] as const)(
    "uses the visible tray label on %s",
    (platform) => {
      expect(searchApp("tray icon", platform)[0]?.label).toBe(
        platform === "macos" ? "Show in menubar" : "Show system tray icon",
      )
    },
  )
  it("covers every pane and check with unique canonical destinations", () => {
    expect(new Set(APP_SEARCH_CATALOG.map((result) => result.id)).size).toBe(
      APP_SEARCH_CATALOG.length,
    )
    for (const { id: pane } of SETTINGS_PANES)
      expect(
        APP_SEARCH_CATALOG.some(
          ({ target }) => target.kind === "setting" && target.pane === pane && !target.control,
        ),
      ).toBe(true)
    for (const check of Object.keys(CHECK_LABELS))
      expect(
        APP_SEARCH_CATALOG.some(
          ({ target }) => target.kind === "check" && target.check === check,
        ),
      ).toBe(true)
  })
  it("prioritizes exact labels and resolves aliases without duplicate results", () => {
    expect(searchApp("Limits")[0]?.target).toEqual({ kind: "view", section: "quota" })
    expect(searchApp("Sound")[0]?.target).toEqual({
      kind: "setting",
      control: "sound",
    })
    expect(searchApp("telemetry")[0]?.target).toEqual({
      kind: "setting",
      control: "analytics",
    })
    expect(searchApp("MCP").filter(({ id }) => id === "check:unusedMcpServers")).toHaveLength(1)
    expect(searchApp("cache misses")[0]?.target).toEqual({ kind: "check", check: "cacheChurn" })
    expect(searchApp("  SOuNd  ")[0]?.label).toBe("Sound")
  })
  it("groups and bounds matches, with no session-data group in PR1", () => {
    for (const query of ["", "settings", "session", "check"]) {
      const groups = groupAppResults(query)
      expect(groups.every((group) => group.results.length <= 5)).toBe(true)
      expect(groups.some((group) => group.label === "Sessions")).toBe(false)
      const ids = groups.flatMap((group) => group.results.map(({ id }) => id))
      expect(new Set(ids).size).toBe(ids.length)
    }
    expect(groupAppResults("nonsense-no-match")).toEqual([])
  })
  it.each(["macos", "windows", "linux"] as const)(
    "keeps configured labels and aliases discoverable on %s",
    (platform) => {
      for (const result of APP_SEARCH_CATALOG) {
        for (const query of [result.label, ...result.aliases]) {
          const matches = searchApp(query, platform).map(({ id }) => id)
          if (!result.platform || result.platform === platform) {
            expect(matches, query).toContain(result.id)
          } else {
            expect(matches, query).not.toContain(result.id)
          }
        }
      }
      expect(searchApp("burn checks", platform)[0]).toMatchObject({
        id: "burnChecks",
        label: "Checks",
        target: { kind: "view", section: "burnChecks" },
      })
    },
  )
  it("retains settings controls and agent filter targets", () => {
    for (const [control, entry] of Object.entries(SETTINGS_SEARCH_TARGETS)) {
      expect(searchApp(entry.label, "macos")).toContainEqual(
        expect.objectContaining({
          target: { kind: "setting", control },
        }),
      )
    }
    for (const agent of AGENT_SLUGS) {
      expect(searchApp(agent)).toContainEqual(
        expect.objectContaining({
          target: { kind: "view", section: "activity", filter: { kind: "agent", agent } },
        }),
      )
    }
  })
  it("excludes fixed session-filter shortcuts and aliases on every platform", () => {
    for (const platform of ["macos", "windows", "linux"] as const) {
      for (const label of ["Notable", "Material", "Failing", "Passing", "All"]) {
        expect(searchApp(`${label} Sessions`, platform)).toEqual([])
      }
      expect(
        searchApp("", platform).filter(
          ({ target }) =>
            target.kind === "view" &&
            target.filter &&
            target.filter.kind !== "agent" &&
            target.filter.kind !== "all",
        ),
      ).toEqual([])
    }
  })
  it("roundtrips stable settings targets and rejects unknown or mismatched targets", () => {
    for (const [control, target] of Object.entries(SETTINGS_SEARCH_TARGETS)) {
      const key = control as keyof typeof SETTINGS_SEARCH_TARGETS
      expect(parseSettingsSearchRequest(settingsSearchRequest(target.pane, key))).toEqual({
        pane: target.pane,
        control,
      })
    }
    expect(parseSettingsSearchRequest("privacy")).toEqual({ pane: "privacy", control: null })
    for (const invalid of [
      "other",
      "general#theme",
      "privacy#unknown",
      "privacy#analytics#extra",
      "privacy#__proto__",
    ])
      expect(parseSettingsSearchRequest(invalid)).toBeNull()
  })
})
