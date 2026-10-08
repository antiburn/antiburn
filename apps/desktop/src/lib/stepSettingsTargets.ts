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
    label: "Ignored instructions",
    aliases: ["missed project instructions", "instruction conflicts"],
  },
  sessionsOverDepthCheck: {
    step: "checks",
    label: "Session overdepth",
    aliases: ["long sessions", "context depth", "enable check"],
  },
  modelOverthinkingCheck: {
    step: "checks",
    label: "Model overthinking",
    aliases: ["reasoning effort", "thinking budget", "enable check"],
  },
  overpoweredSubagentsCheck: {
    step: "checks",
    label: "Overpowered subagents",
    aliases: ["expensive subagents", "model routing", "enable check"],
  },
  unusedMcpServersCheck: {
    step: "checks",
    label: "Unused MCP servers",
    aliases: ["unused servers", "MCP tools", "enable check"],
  },
  unusedBuiltInToolsCheck: {
    step: "checks",
    label: "Unused built-in tools",
    aliases: ["tool definitions", "built in tools", "enable check"],
  },
  unusedSkillsCheck: {
    step: "checks",
    label: "Unused skills",
    aliases: ["skill instructions", "enable check"],
  },
  oldModelUsageCheck: {
    step: "checks",
    label: "Old model usage",
    aliases: ["outdated models", "model versions", "enable check"],
  },
  overuseOfFastModeCheck: {
    step: "checks",
    label: "Fast mode overuse",
    aliases: ["fast mode", "priority", "enable check"],
  },
  cacheChurnCheck: {
    step: "checks",
    label: "Excess cache rehydration",
    aliases: ["prompt cache", "cache misses", "enable check"],
  },
  checkHistory: {
    step: "checks",
    label: "Check history",
    aliases: ["past sessions", "historical checks", "backfill"],
  },
  skillOpportunitiesCheck: {
    step: "checks",
    label: "Enable skill opportunities",
    aliases: ["enable skill opportunities", "skill check preference"],
  },
  overExploringCheck: {
    step: "checks",
    label: "Enable over-exploring",
    aliases: ["enable over-exploring", "reading check preference"],
  },
  scopeCreepCheck: {
    step: "checks",
    label: "Enable scope creep",
    aliases: ["enable scope creep", "task scope preference"],
  },
  smartCheckProvider: {
    step: "checks",
    label: "Provider",
    aliases: [
      "smart check provider",
      "Use Ollama or another provider",
      "decision model",
      "Ollama",
      "Cloudflare",
      "custom endpoint",
      "saved connections",
    ],
  },
  smartCheckLimits: {
    step: "checks",
    label: "Model limits",
    aliases: ["capabilities", "context tokens", "refresh model", "manual limits"],
  },
  smartChecksEnabled: {
    step: "checks",
    label: "Enable smart burn checks",
    aliases: ["pause smart checks", "Jev checks"],
  },
  typeSafeApiKey: {
    step: "checks",
    label: "API key",
    aliases: [
      "TypeSafe API key",
      "Cloudflare API token",
      "provider credential",
      "Jev",
      "enable checks",
      "usage charges",
    ],
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
