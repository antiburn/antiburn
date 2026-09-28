import type { EvidenceReference, SessionEvidenceHit } from "./sessionEvidenceIpc"
import { sessionKey, type SessionSubject } from "./sessionSubject"
import type { SessionSearchEntry } from "./sessionSearchIpc"
import { AGENT_SLUGS, agentDisplayName, agentSessionFilterLabel } from "./presentation/agents"
import { CHECK_DEFINITIONS } from "./presentation/checkDefinitions"
import type { BurnCheckDetectorId } from "./insightsIpc"
import { detectPlatform, type Platform } from "./platform"
import { MAIN_VIEWS, type MainViewId } from "./navigation/mainViews"

import type { SessionFilters } from "./sessionFilters"
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
  | { kind: "view"; section: Exclude<MainViewId, "activity">; filters?: never }
  | { kind: "view"; section: "activity"; filters?: SessionFilters }
  | SettingsSearchTarget
  | { kind: "check"; check: BurnCheckDetectorId }
  | { kind: "session"; subject: SessionSubject; evidence?: EvidenceReference }

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
    ? { kind: "view", section, filters: { agents: [], result: "all", spend: "all" } }
    : { kind: "view", section }
}

export type AppSearchResult = {
  id: string
  label: string
  detail: string
  aliases: readonly string[]
  target: AppSearchTarget
  evidence?: SessionEvidenceHit
  platform?: "macos"
  session?: Pick<
    SessionSearchEntry,
    "agent" | "repository" | "cwdLabel" | "models" | "wslDistro"
  >
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
      filters: { agents: [agent], result: "all" as const, spend: "all" as const },
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

export function sessionAppResult(entry: SessionSearchEntry): AppSearchResult {
  return {
    id: `session:${encodeURIComponent(JSON.stringify([entry.environmentKey, entry.agent, entry.sessionId]))}`,
    label: entry.title || entry.sessionId,
    detail: [
      "Sessions",
      agentDisplayName(entry.agent),
      entry.repository || entry.cwdLabel,
      ...entry.models,
      entry.wslDistro ? `WSL: ${entry.wslDistro}` : null,
    ]
      .filter(Boolean)
      .join(" · "),
    aliases: [],
    session: {
      agent: entry.agent,
      repository: entry.repository,
      cwdLabel: entry.cwdLabel,
      models: entry.models,
      wslDistro: entry.wslDistro,
    },
    target: {
      kind: "session",
      subject: {
        agent: entry.agent,
        sessionId: entry.sessionId,
        wslDistro: entry.wslDistro,
        repo: entry.repository,
        timestamp: entry.timestamp,
        ...(entry.title ? { title: entry.title } : {}),
      },
    },
  }
}

export function evidenceAppResult(hit: SessionEvidenceHit): AppSearchResult {
  const result = sessionAppResult(hit.session)
  if (result.target.kind !== "session") return result
  return {
    ...result,
    evidence: hit,
    target: { ...result.target, evidence: hit.reference },
  }
}

export function searchTargetKey(result: AppSearchResult): string {
  const target = result.target
  return JSON.stringify(
    target.kind === "session" ? [target.kind, sessionKey(target.subject)] : target,
  )
}

export function groupAppResults(
  query: string,
  results = searchApp(query),
  sessions: AppSearchResult[] = [],
  selectedId?: string | null,
): AppSearchGroup[] {
  const normalized = query.trim().toLocaleLowerCase()
  const exactSession = sessions.find(
    (result) =>
      result.label.toLocaleLowerCase() === normalized ||
      (result.target.kind === "session" &&
        result.target.subject.sessionId.toLocaleLowerCase() === normalized),
  )
  const best = normalized
    ? results[0]?.label.toLocaleLowerCase() === normalized
      ? results[0]
      : (exactSession ?? results[0] ?? sessions[0])
    : undefined
  const remaining = [...results, ...sessions].filter((result) => result !== best)
  const groups: AppSearchGroup[] = best ? [{ label: "Best match", results: [best] }] : []
  for (const [kind, label] of [
    ["view", "Features"],
    ["setting", "Settings"],
    ["session", "Sessions"],
    ["check", "Checks"],
  ] as const) {
    const candidates = remaining.filter((result) => result.target.kind === kind)
    const matches = candidates.slice(0, kind === "session" ? undefined : 5)
    const selected = candidates.find((result) => result.id === selectedId)
    if (selected && !matches.includes(selected)) matches[matches.length - 1] = selected
    if (matches.length) groups.push({ label, results: matches })
  }
  return groups
}
