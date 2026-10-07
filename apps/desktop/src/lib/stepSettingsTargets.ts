/**
 * A progress step that owns searchable controls. Narrower than
 * `StepSettingsStep` (`views/main-window/overview/stepSettings/StepSettings.tsx`):
 * `"limits"` has no modal to focus a control in — its settings stay in
 * Settings → Usage, searchable through `settingsSearchTargets.ts` instead —
 * and `"fixes"` has no settings at all. This file stays in `lib/` and does
 * not import that component, so a generic search result can depend on it
 * without pulling in the Overview's React tree.
 */
export type StepSettingsSearchStep = "agents" | "sessions" | "checks"

/** A step's display label, for a search result's detail line. */
export const STEP_SETTINGS_STEP_LABELS: Record<StepSettingsSearchStep, string> = {
  agents: "Agents",
  sessions: "Sessions",
  checks: "Checks",
}

/**
 * Stable targets for the controls the Agents, Sessions, and Checks progress
 * steps own. Same shape as `SETTINGS_SEARCH_TARGETS`, with `step` in place
 * of `pane`.
 */
export const STEP_SETTINGS_TARGETS = {
  monitoring: {
    step: "sessions",
    label: "Keep looking for new sessions",
    aliases: ["automatic scan", "watch"],
  },
  sourceScanning: {
    step: "sessions",
    label: "Recent sessions",
    aliases: ["scanning", "discovery", "rescan"],
  },
  sourceAgents: {
    step: "agents",
    label: "Coding agents",
    aliases: ["harnesses", "enabled agents"],
  },
  sourceRemoteHosts: {
    step: "sessions",
    label: "Remote hosts",
    aliases: ["ssh", "remote sessions", "other computers"],
  },
  sourceAutomaticSync: {
    step: "sessions",
    label: "Sync frequency",
    aliases: ["automatic sync", "remote schedule", "sync interval"],
  },
  sourceFolders: {
    step: "sessions",
    label: "Scan folders",
    aliases: ["directories", "folder access", "paths"],
  },
  sourceRepositories: {
    step: "sessions",
    label: "Repositories",
    aliases: ["projects", "repo", "enabled repositories"],
  },
  sourceNonRepoFolders: {
    step: "sessions",
    label: "Include folders without git",
    aliases: ["non-repo folders", "missing sessions", "not a repository"],
  },
  historicalScan: {
    step: "sessions",
    label: "Older sessions",
    aliases: ["historical scan", "history", "import", "rescan"],
  },
  recentDays: {
    step: "sessions",
    label: "Show the last",
    aliases: ["days", "recent sessions", "date range"],
  },
  indexedSessions: {
    step: "sessions",
    label: "Indexed sessions",
    aliases: ["storage", "database size"],
  },
  retention: {
    step: "sessions",
    label: "Keep session data",
    aliases: ["retention", "storage", "history"],
  },
  ignoredInstructions: {
    step: "checks",
    label: "Ignored Instructions",
    aliases: ["missed project instructions", "instruction conflicts"],
  },
  checkHistory: {
    step: "checks",
    label: "Check history",
    aliases: ["past sessions", "historical checks", "backfill"],
  },
  typeSafeApiKey: {
    step: "checks",
    label: "API key",
    aliases: ["TypeSafe API key", "Jev", "enable checks", "usage charges"],
  },
} as const satisfies Record<
  string,
  {
    step: StepSettingsSearchStep
    label: string
    aliases: readonly string[]
  }
>

export type StepSettingsControlId = keyof typeof STEP_SETTINGS_TARGETS

export function stepSettingsControlLabel(control: StepSettingsControlId): string {
  return STEP_SETTINGS_TARGETS[control].label
}
