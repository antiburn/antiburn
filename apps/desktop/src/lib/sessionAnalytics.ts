import { noteInteraction, type AppSettings } from "./ipc"
import type { BurnCheckDetectorId, SessionHygienePayload } from "./insightsIpc"
import { AGENT_SLUGS } from "./presentation/agents"
import { sessionBurnCheckPresentation } from "./presentation/checkStatus"
import { sessionHygieneChecks } from "./presentation/sessionHygiene"
import { visibleSessionHygieneChecks } from "./snoozedBurnChecks"
import type { SessionSubject } from "./sessionSubject"

export type SessionEntryRoute =
  | "popover"
  | "session_list"
  | "filtered_list"
  | "adjacent"
  | "related"
  | "burn_check_sample"
  | "history"
  | "automatic"
  | "return"

/** Local navigation metadata. The subject identity stays in the owning controller. */
export type SessionNavigationHint = { route: SessionEntryRoute; visit?: string }

export type SessionDetailTab = "context" | "cost" | "tools"
export type SessionDetailAction =
  | "fork_menu_opened"
  | "open_parent"
  | "open_child"
  | "open_subagent"
  | "open_orchestrator"
  | "reveal_transcript"
  | "copy_path"
  | "copy_discussion_prompt"
  | "request_removal"
  | "open_project_folder"
  | "copy_project_path"
  | "open_quota"
  | "open_memory"
  | "pin_cost_series"
  | "limits_expanded"

export type SessionFilterControl =
  | "search_field_clicked"
  | "search_dismissed"
  | "search_dismissed_no_results"
  | "search_dismissed_results_unavailable"
  | "search_dismissed_with_results"
  | "search_retry_checks"
  | "search_sessions_attempted"
  | "search_agent_attempted"
  | "search_settings_attempted"
  | "search_sessions_opened"
  | "search_agent_opened"
  | "search_settings_opened"
  | "filter_menu_opened"
  | "filter_menu_dismissed"
  | "all_agents"
  | "agent_option"
  | "result_all"
  | "result_passing"
  | "result_failing"
  | "spend_all"
  | "spend_notable"
  | "spend_material"
  | "remove_agent_chip"
  | "remove_result_chip"
  | "remove_spend_chip"
  | "clear_filters"
  | "change_time_range"
  | "find_older_matches"
  | "retry_list"

export type SessionMetric = "cost" | "weekly_percent" | "five_hour_percent"
export type SessionMetricSurface = "popover" | "main_window"
export type SessionFilterResult = "results_pending" | "no_matches"

type CheckBadge = "passing" | "failing" | "mixed" | "unknown"
type Recency = "up_to_3h" | "over_3h_to_24h" | "over_24h_to_7d" | "over_7d" | "unknown"

/** Allowlisted context sent with session visits. */
export interface SessionContext {
  agent?: string
  environment: "native" | "wsl" | "remote"
  activity: "active" | "inactive" | "unknown"
  sessionKind: "session" | "subagent"
  entryRoute: SessionEntryRoute
  recency: Recency
  checkBadge: CheckBadge
}

const HOUR_MS = 60 * 60 * 1000

function sessionRecency(timestamp: string | undefined, now: number): Recency {
  const age = now - Date.parse(timestamp ?? "")
  if (!Number.isFinite(age) || age < 0) return "unknown"
  if (age <= 3 * HOUR_MS) return "up_to_3h"
  if (age <= 24 * HOUR_MS) return "over_3h_to_24h"
  if (age <= 7 * 24 * HOUR_MS) return "over_24h_to_7d"
  return "over_7d"
}

/** Map the available session check badge to a closed verdict. */
export function sessionCheckBadge(
  hygiene: SessionHygienePayload | undefined,
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): CheckBadge {
  if (!hygiene) return "unknown"
  const checks = visibleSessionHygieneChecks(sessionHygieneChecks(hygiene), snoozed)
  const { indicator } = sessionBurnCheckPresentation(checks, hygiene.evidenceState)
  if (indicator.kind === "pass") return "passing"
  if (indicator.kind === "fail") return "failing"
  if (indicator.kind !== "segments") return "unknown"
  const passed = indicator.segments.some(({ outcome }) => outcome === "passed")
  const failed = indicator.segments.some(({ outcome }) => outcome === "failed")
  return failed ? (passed ? "mixed" : "failing") : passed ? "passing" : "unknown"
}

/** Derive a payload without passing source identities to analytics. */
export function sessionContext(
  subject: Pick<
    SessionSubject,
    "agent" | "wslDistro" | "remoteHostId" | "timestamp" | "subagent"
  >,
  options: {
    route: SessionEntryRoute
    isActive: boolean | undefined
    hygiene?: SessionHygienePayload | undefined
    snoozed: ReadonlySet<BurnCheckDetectorId>
    now: number
  },
): SessionContext {
  return {
    ...(AGENT_SLUGS.includes(subject.agent) ? { agent: subject.agent } : {}),
    environment: subject.remoteHostId ? "remote" : subject.wslDistro ? "wsl" : "native",
    activity:
      options.isActive === undefined ? "unknown" : options.isActive ? "active" : "inactive",
    sessionKind: subject.subagent ? "subagent" : "session",
    entryRoute: options.route,
    recency: sessionRecency(subject.timestamp, options.now),
    checkBadge: sessionCheckBadge(options.hygiene, options.snoozed),
  }
}

/** Mint an unlinkable visit ID when the webview provides a secure random source. */
export function newSessionVisitId(): string | undefined {
  return globalThis.crypto?.randomUUID?.()
}

/** Report a fixed search or filter activation. */
export function noteSessionFilter(action: SessionFilterControl): void {
  noteInteraction({ kind: "sessionFilterInteraction", action })
}

const METRICS: Record<AppSettings["sessionBadgeMetric"], SessionMetric> = {
  cost: "cost",
  weeklyPercent: "weekly_percent",
  fiveHourPercent: "five_hour_percent",
}

/** Report an explicit change to the session list badge. */
export function noteSessionMetric(
  surface: SessionMetricSurface,
  current: AppSettings["sessionBadgeMetric"],
  selected: AppSettings["sessionBadgeMetric"],
): void {
  if (selected !== current)
    noteInteraction({ kind: "sessionMetricSelected", metric: METRICS[selected], surface })
}
