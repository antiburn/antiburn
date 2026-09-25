import type { Platform } from "./platform"
import { isSettingsPane, type SettingsPane } from "./settingsPanes"

/** Stable targets keep navigation independent of translated or edited labels. */
export const SETTINGS_SEARCH_TARGETS = {
  monitoring: {
    pane: "general",
    label: "Keep looking for new sessions",
    aliases: ["automatic scan", "watch"],
  },
  historicalScan: { pane: "general", label: "Historical scan", aliases: ["import", "rescan"] },
  recentDays: {
    pane: "general",
    label: "Show the last",
    aliases: ["days", "recent sessions", "date range"],
  },
  indexedSessions: {
    pane: "general",
    label: "Indexed sessions",
    aliases: ["storage", "database size"],
  },
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
  setup: { pane: "general", label: "Run setup again", aliases: ["onboarding"] },
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
  retention: {
    pane: "privacy",
    label: "Keep session data",
    aliases: ["retention", "storage", "history"],
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
  sourceScanning: { pane: "sources", label: "Scanning", aliases: ["discovery", "rescan"] },
  sourceAgents: {
    pane: "sources",
    label: "Coding agents",
    aliases: ["harnesses", "enabled agents"],
  },
  sourceFolders: {
    pane: "sources",
    label: "Scan folders",
    aliases: ["directories", "folder access", "paths"],
  },
  sourceRepositories: {
    pane: "sources",
    label: "Repositories",
    aliases: ["projects", "repo", "enabled repositories"],
  },
  sourceNonRepoFolders: {
    pane: "sources",
    label: "Include folders without git",
    aliases: ["non-repo folders", "missing sessions", "not a repository"],
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
