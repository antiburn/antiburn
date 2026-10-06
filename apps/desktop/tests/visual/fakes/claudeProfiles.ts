import type { ClaudeProfile, ClaudeProfilesPayload } from "../../../src/lib/claudeProfiles"

/** The fixture uses Claude profiles when the URL carries `profiles=1`. */
export function hasClaudeProfilesFixture(): boolean {
  return new URLSearchParams(window.location.search).get("profiles") === "1"
}

const MAX_PROFILES = 16
const MAX_LABEL_CHARS = 80

let profiles: ClaudeProfile[] = [
  { id: "default", label: "Claude", path: "/Users/fixture/.claude", builtIn: true },
  { id: "work", label: "Claude Work", path: "/Users/fixture/.claude-work", builtIn: false },
]
let suggestions = [{ path: "/Users/fixture/.claude-side", label: "Claude Side" }]

function payload(): ClaudeProfilesPayload {
  return {
    profiles: [...profiles],
    suggestions: [...suggestions],
    maxProfiles: MAX_PROFILES,
    maxLabelChars: MAX_LABEL_CHARS,
  }
}

/** The shell's own validation, so the editor's error path renders. */
function checkedLabel(label: unknown, except: string | null): string {
  const name = typeof label === "string" ? label.trim() : ""
  if (!name) throw "Enter a name for this profile."
  if (
    profiles.some(
      (profile) => profile.id !== except && profile.label.toLowerCase() === name.toLowerCase(),
    )
  ) {
    throw "Another profile already uses this name."
  }
  return name
}

export function claudeProfilesFixtureCommand(
  command: string,
  args: Record<string, unknown> | undefined,
): unknown {
  switch (command) {
    case "list_claude_profiles":
      return payload()
    case "add_claude_profile": {
      const label = checkedLabel(args?.label, null)
      const path = String(args?.path ?? "")
      if (profiles.some((profile) => profile.path === path)) throw "This folder is already a profile."
      profiles = [...profiles, { id: `fixture-${profiles.length}`, label, path, builtIn: false }]
      suggestions = suggestions.filter((suggestion) => suggestion.path !== path)
      return payload()
    }
    case "rename_claude_profile": {
      const id = String(args?.id ?? "")
      const label = checkedLabel(args?.label, id)
      profiles = profiles.map((profile) => (profile.id === id ? { ...profile, label } : profile))
      return payload()
    }
    case "remove_claude_profile":
      profiles = profiles.filter((profile) => profile.builtIn || profile.id !== args?.id)
      return payload()
    default:
      return undefined
  }
}

function claudeReading(accountKey: string, accountLabel: string, used: [number, number]) {
  return {
    provider: "anthropic",
    accountKey,
    accountLabel,
    displayName: "Claude",
    sourceLabel: "Asked Claude directly",
    freshness: "fresh",
    support: "live",
    observedAt: "2026-09-15T00:00:00.000Z",
    windows: [
      {
        id: "five-hour",
        role: "primaryShort",
        kind: "rolling",
        scopeModel: null,
        usedPercent: used[0],
        startsAt: "2026-09-14T21:00:00.000Z",
        resetsAt: "2026-09-15T02:00:00.000Z",
        elapsedFraction: 0.6,
        hasNonzeroUsageInCurrentPeriod: true,
        forecast: {
          unavailableReason: "sparseHistory",
          confidence: null,
          consumptionRate: null,
          paceRatio: null,
          paceTrend: null,
          runwayAt: null,
          usedToday: null,
        },
      },
      {
        id: "seven-day",
        role: "primaryLong",
        kind: "weekly",
        scopeModel: null,
        usedPercent: used[1],
        startsAt: "2026-09-10T00:00:00.000Z",
        resetsAt: "2026-09-17T00:00:00.000Z",
        elapsedFraction: 0.7,
        hasNonzeroUsageInCurrentPeriod: true,
        forecast: {
          unavailableReason: "sparseHistory",
          confidence: null,
          consumptionRate: null,
          paceRatio: null,
          paceTrend: null,
          runwayAt: null,
          usedToday: null,
        },
      },
    ],
    extraUsage: null,
    resetCredits: null,
    plan: { name: "max", tier: "default_claude_max_20x" },
    accountUuid: null,
    accountEmail: null,
  }
}

/**
 * Live usage with two named Claude profiles, Codex, and a rate-limited
 * profile whose provider asked for a wait until after the fixture's time.
 */
export function claudeProfilesLiveUsage(base: {
  providers: unknown[]
  meters: unknown[]
  generatedAt: string
}): unknown {
  return {
    ...base,
    providers: [
      claudeReading("fixture-personal", "Claude", [7, 2]),
      claudeReading("fixture-work", "Claude Work", [10, 53]),
      ...base.providers,
    ],
    errors: [
      {
        source: "claude-usage-fetch",
        provider: "anthropic",
        displayName: "Claude Side",
        category: "rateLimited",
        accountLabel: "Claude Side",
        retryAt: "2026-09-15T00:12:00.000Z",
      },
    ],
    meters: [{ provider: "anthropic", displayName: "Claude", shown: true }, ...base.meters],
  }
}
