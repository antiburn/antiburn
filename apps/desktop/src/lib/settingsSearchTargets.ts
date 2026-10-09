import type { Platform } from "./platform"
import { isSettingsPane, type SettingsPane } from "./settingsPanes"

/** Stable targets keep navigation independent of translated or edited labels. */
export const SETTINGS_SEARCH_TARGETS = {
  trayIcon: {
    pane: "general",
    label: "Show in menubar",
    platformLabels: {
      windows: "Show system tray icon",
      linux: "Show system tray icon",
      unknown: "Show system tray icon",
    },
    aliases: ["system tray", "tray icon"],
  },
  dockIcon: {
    pane: "general",
    label: "Show in Dock",
    aliases: ["application icon"],
    platform: "macos",
  },
  startAtLogin: { pane: "general", label: "Start at login", aliases: ["startup", "launch"] },
  monitoring: {
    pane: "sessions",
    label: "Keep looking for new sessions",
    aliases: ["automatic scan", "watch"],
  },
  sourceScanning: {
    pane: "sessions",
    label: "Recent sessions",
    aliases: ["scanning", "discovery", "rescan"],
  },
  sourceAgents: {
    pane: "agents",
    label: "Coding agents",
    aliases: ["harnesses", "enabled agents"],
  },
  sourceRemoteHosts: {
    pane: "sessions",
    label: "Remote hosts",
    aliases: ["ssh", "remote sessions", "other computers"],
  },
  sourceAutomaticSync: {
    pane: "sessions",
    label: "Sync frequency",
    aliases: ["automatic sync", "remote schedule", "sync interval"],
  },
  sourceFolders: {
    pane: "sessions",
    label: "Scan folders",
    aliases: ["directories", "folder access", "paths"],
  },
  sourceRepositories: {
    pane: "sessions",
    label: "Repositories",
    aliases: ["projects", "repo", "enabled repositories"],
  },
  sourceNonRepoFolders: {
    pane: "sessions",
    label: "Include folders without git",
    aliases: ["non-repo folders", "missing sessions", "not a repository"],
  },
  historicalScan: {
    pane: "sessions",
    label: "Older sessions",
    aliases: ["historical scan", "history", "import", "rescan"],
  },
  recentDays: {
    pane: "sessions",
    label: "Show the last",
    aliases: ["days", "recent sessions", "date range"],
  },
  indexedSessions: {
    pane: "sessions",
    label: "Indexed sessions",
    aliases: ["storage", "database size"],
  },
  retention: {
    pane: "sessions",
    label: "Keep session data",
    aliases: ["retention", "storage", "history"],
  },
  sessionsOverDepthCheck: {
    pane: "checks",
    label: "Session overdepth",
    aliases: ["long sessions", "context depth", "enable check"],
  },
  modelOverthinkingCheck: {
    pane: "checks",
    label: "Model overthinking",
    aliases: ["reasoning effort", "thinking budget", "enable check"],
  },
  overpoweredSubagentsCheck: {
    pane: "checks",
    label: "Overpowered subagents",
    aliases: ["expensive subagents", "model routing", "enable check"],
  },
  unusedMcpServersCheck: {
    pane: "checks",
    label: "Unused MCP servers",
    aliases: ["unused servers", "MCP tools", "enable check"],
  },
  unusedBuiltInToolsCheck: {
    pane: "checks",
    label: "Unused built-in tools",
    aliases: ["tool definitions", "built in tools", "enable check"],
  },
  unusedSkillsCheck: {
    pane: "checks",
    label: "Unused skills",
    aliases: ["skill instructions", "enable check"],
  },
  oldModelUsageCheck: {
    pane: "checks",
    label: "Old model usage",
    aliases: ["outdated models", "model versions", "enable check"],
  },
  overuseOfFastModeCheck: {
    pane: "checks",
    label: "Fast mode overuse",
    aliases: ["fast mode", "priority", "enable check"],
  },
  cacheChurnCheck: {
    pane: "checks",
    label: "Excess cache rehydration",
    aliases: ["prompt cache", "cache misses", "enable check"],
  },
  ignoredInstructions: {
    pane: "checks",
    label: "Ignored instructions",
    aliases: ["missed project instructions", "instruction conflicts"],
  },
  checkHistory: {
    pane: "checks",
    label: "Check history",
    aliases: ["past sessions", "historical checks", "backfill"],
  },
  typeSafeApiKey: {
    pane: "checks",
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
  skillOpportunitiesCheck: {
    pane: "checks",
    label: "Enable skill opportunities",
    aliases: ["enable skill opportunities", "skill check preference"],
  },
  overExploringCheck: {
    pane: "checks",
    label: "Enable over-exploring",
    aliases: ["enable over-exploring", "reading check preference"],
  },
  scopeCreepCheck: {
    pane: "checks",
    label: "Enable scope creep",
    aliases: ["enable scope creep", "task scope preference"],
  },
  smartCheckProvider: {
    pane: "checks",
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
    pane: "checks",
    label: "Model limits",
    aliases: ["capabilities", "context tokens", "refresh model", "manual limits"],
  },
  smartChecksEnabled: {
    pane: "checks",
    label: "Enable smart burn checks",
    aliases: ["pause smart checks", "Jev checks"],
  },
  theme: {
    pane: "appearance",
    label: "Appearance",
    aliases: ["theme", "light", "dark", "system"],
  },
  allResources: {
    pane: "appearance",
    label: "Show every skill and MCP",
    aliases: ["tools", "unused resources"],
  },
  interfaceSize: {
    pane: "appearance",
    label: "Interface size",
    aliases: ["zoom", "scaling", "text size"],
  },
  analytics: {
    pane: "privacy",
    label: "Share product analytics",
    aliases: ["telemetry", "tracking", "opt out"],
  },
  clearIndex: {
    pane: "privacy",
    label: "Clear the local index",
    aliases: ["delete", "reset database"],
  },
  diagnostics: { pane: "privacy", label: "Export diagnostics", aliases: ["logs", "support"] },
  planLimits: {
    pane: "usage",
    label: "Keep my plan limits current",
    aliases: ["quota", "refresh", "providers"],
  },
  workingWeek: {
    pane: "usage",
    label: "Days you work",
    aliases: ["working week", "workdays", "pace marker"],
  },
  floatingHud: {
    pane: "usage",
    label: "Show floating usage HUD",
    aliases: ["meter", "overlay"],
    platform: "macos",
  },
  notifications: {
    pane: "notifications",
    label: "Notify me",
    aliases: ["alerts", "notifications"],
  },
  sound: { pane: "notifications", label: "Sound", aliases: ["audio", "mute"] },
  doNotDisturb: { pane: "notifications", label: "Respect Do Not Disturb", aliases: ["focus"] },
  autoDismiss: {
    pane: "notifications",
    label: "Auto-dismiss time",
    aliases: ["notification duration"],
  },
  testNotification: {
    pane: "notifications",
    label: "Test notification",
    aliases: ["preview alert"],
  },
  shortMilestones: {
    pane: "notifications",
    label: "5-hour milestones",
    aliases: ["short usage limit", "threshold"],
  },
  weeklyMilestones: {
    pane: "notifications",
    label: "Weekly milestones",
    aliases: ["long usage limit", "threshold"],
  },
  notificationPosition: {
    pane: "notifications",
    label: "Position",
    aliases: ["notification placement"],
    platform: "macos",
  },
  diskDisplay: {
    pane: "notifications",
    label: "Show in menu bar",
    aliases: ["disk space display"],
    platform: "macos",
  },
  diskThreshold: {
    pane: "notifications",
    label: "Low when below",
    aliases: ["disk space threshold"],
    platform: "macos",
  },
  diskNotify: {
    pane: "notifications",
    label: "Notify when low",
    aliases: ["disk space alert"],
    platform: "macos",
  },
  usageMeters: {
    pane: "usage",
    label: "Track Limits for",
    aliases: ["usage meters", "provider limits", "anthropic", "openai", "google"],
  },
  softwareUpdate: {
    pane: "about",
    label: "Software update",
    aliases: ["check for updates", "app version", "upgrade"],
  },
  automaticUpdates: {
    pane: "about",
    label: "Install updates automatically",
    aliases: ["auto update"],
  },
  pricingCatalog: { pane: "about", label: "Pricing catalog", aliases: ["prices"] },
  localDatabase: { pane: "about", label: "Local database", aliases: ["schema"] },
  license: { pane: "about", label: "Licence", aliases: ["license"] },
  privacyPolicy: {
    pane: "about",
    label: "Privacy and data handling",
    aliases: ["privacy policy"],
  },
  legalNotices: { pane: "about", label: "Legal notices", aliases: ["legal"] },
  thirdParty: {
    pane: "about",
    label: "Third-party attributions",
    aliases: ["open source", "dependencies", "notices"],
  },
  dataFolder: {
    pane: "about",
    label: "Data folder",
    aliases: ["application data", "storage location"],
  },
} as const satisfies Record<
  string,
  {
    pane: SettingsPane
    label: string
    aliases: readonly string[]
    platform?: "macos"
    platformLabels?: Partial<Record<Platform, string>>
  }
>

export type SettingsControlId = keyof typeof SETTINGS_SEARCH_TARGETS
export type SettingsSearchRequest = { pane: SettingsPane; control: SettingsControlId | null }

export function settingsSearchRequest(pane: SettingsPane, control?: SettingsControlId): string {
  return control && SETTINGS_SEARCH_TARGETS[control].pane === pane ? `${pane}#${control}` : pane
}

export function parseSettingsSearchRequest(
  request: string | null,
): SettingsSearchRequest | null {
  if (!request) return null
  const [pane, control, extra] = request.split("#")
  if (!isSettingsPane(pane) || extra !== undefined) return null
  if (!control) return { pane, control: null }
  if (!Object.hasOwn(SETTINGS_SEARCH_TARGETS, control)) return null
  const id = control as SettingsControlId
  return SETTINGS_SEARCH_TARGETS[id].pane === pane ? { pane, control: id } : null
}

export function settingsControlLabel(control: SettingsControlId, platform: Platform): string {
  const entry = SETTINGS_SEARCH_TARGETS[control]
  if ("platformLabels" in entry && platform in entry.platformLabels) {
    return entry.platformLabels[platform as keyof typeof entry.platformLabels]
  }
  return entry.label
}
