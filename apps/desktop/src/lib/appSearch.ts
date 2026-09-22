import { AGENT_SLUGS, agentSessionFilterLabel } from "./presentation/agents"
import { CHECK_DEFINITIONS } from "./presentation/checkDefinitions"
import type { BurnCheckDetectorId } from "./insightsIpc"
import { detectPlatform, type Platform } from "./platform"
import { MAIN_VIEWS, type MainViewId } from "./navigation/mainViews"

import type { SessionFilter } from "./sessionFilters"
import { SETTINGS_PANES, settingsPaneLabel, type SettingsPane } from "./settingsPanes"
import {
  settingsControlLabel,
  SETTINGS_SEARCH_TARGETS,
  type SettingsControlId,
} from "./settingsSearchTargets"

export type SettingsSearchTarget =
  | { kind: "setting"; pane: SettingsPane; control?: never }
  | { kind: "setting"; control: SettingsControlId; pane?: never }

export type AppSearchTarget =
  | { kind: "view"; section: Exclude<MainViewId, "activity">; filter?: never }
  | { kind: "view"; section: "activity"; filter?: SessionFilter }
  | SettingsSearchTarget
  | { kind: "check"; check: BurnCheckDetectorId }

export function resolveSettingsSearchTarget(target: SettingsSearchTarget): {
  pane: SettingsPane
  control: SettingsControlId | undefined
} {
  return target.control
    ? { pane: SETTINGS_SEARCH_TARGETS[target.control].pane, control: target.control }
    : { pane: target.pane, control: undefined }
}

function viewTarget(section: MainViewId): AppSearchTarget {
  return section === "activity"
    ? { kind: "view", section, filter: { kind: "all" } }
    : { kind: "view", section }
}

export type AppSearchResult = {
  id: string
  label: string
  detail: string
  aliases: readonly string[]
  target: AppSearchTarget
  platform?: "macos"
}
export type AppSearchGroup = { label: string; results: AppSearchResult[] }

export const APP_SEARCH_CATALOG: readonly AppSearchResult[] = [
  ...MAIN_VIEWS.map(({ id: section, label, aliases }) => ({
    id: section,
    label,
    detail: "Features",
    aliases,
    target: viewTarget(section),
  })),
  ...AGENT_SLUGS.map((agent) => ({
    id: `filter:agent:${agent}`,
    label: agentSessionFilterLabel(agent).replace(/ Sessions$/, " sessions"),
    detail: "Features",
    aliases: [agent, "agent filter"],
    target: {
      kind: "view" as const,
      section: "activity" as const,
      filter: { kind: "agent" as const, agent },
    },
  })),
  ...SETTINGS_PANES.map(({ id: pane, label }) => ({
    id: `settings:${pane}`,
    label: `${label} settings`,
    detail: "Settings",
    aliases: [pane],
    target: { kind: "setting" as const, pane },
  })),
  ...Object.entries(SETTINGS_SEARCH_TARGETS).map(([control, entry]) => ({
    id: `settings:${entry.pane}:${control}`,
    label: entry.label,
    detail: `Settings · ${settingsPaneLabel(entry.pane)}`,
    aliases: entry.aliases,
    target: {
      kind: "setting" as const,
      control: control as SettingsControlId,
    },
    ...("platform" in entry ? { platform: entry.platform } : {}),
  })),
  ...Object.entries(CHECK_DEFINITIONS).map(([check, { label, aliases }]) => ({
    id: `check:${check}`,
    label,
    detail: "Checks",
    aliases: [check, ...aliases],
    target: { kind: "check" as const, check: check as BurnCheckDetectorId },
  })),
]

export function searchApp(
  query: string,
  platform: Platform = detectPlatform(),
): AppSearchResult[] {
  const normalized = query.trim().toLocaleLowerCase().slice(0, 200)
  const words = normalized.split(/\s+/).filter(Boolean)
  return APP_SEARCH_CATALOG.filter((result) => !result.platform || result.platform === platform)
    .map((result) => {
      if (result.target.kind !== "setting" || !result.target.control) return result
      const label = settingsControlLabel(result.target.control, platform)
      return label === result.label
        ? result
        : { ...result, label, aliases: [...result.aliases, result.label] }
    })
    .map((result, index) => {
      const label = result.label.toLocaleLowerCase()
      const text = [label, result.detail, ...result.aliases].join(" ").toLocaleLowerCase()
      const score =
        label === normalized
          ? 4
          : label.startsWith(normalized)
            ? 3
            : result.aliases.some((alias) => alias.toLocaleLowerCase() === normalized)
              ? 2
              : 1
      return { result, index, score, matches: words.every((word) => text.includes(word)) }
    })
    .filter((entry) => entry.matches)
    .sort((a, b) => b.score - a.score || a.index - b.index)
    .map((entry) => entry.result)
}

export function groupAppResults(query: string, results = searchApp(query)): AppSearchGroup[] {
  const best = query.trim() ? results[0] : undefined
  const remaining = results.filter((result) => result !== best)
  const groups: AppSearchGroup[] = best ? [{ label: "Best match", results: [best] }] : []
  for (const [kind, label] of [
    ["view", "Features"],
    ["setting", "Settings"],
    ["check", "Checks"],
  ] as const) {
    const matches = remaining.filter((result) => result.target.kind === kind).slice(0, 5)
    if (matches.length) groups.push({ label, results: matches })
  }
  return groups
}
