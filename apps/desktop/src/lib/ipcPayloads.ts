/**
 * The payload shapes the shell and the views agree on.
 *
 * These are mirrors of the Rust types. They carry no behavior, so they sit
 * apart from the command wrappers that send and receive them.
 */

import type {
  ActiveSessionsSummary,
  BillableTokens,
  SessionCostComponents,
  SessionEfficiency,
} from "./types/session"

/* -------------------------------------------------------------------------
 * Payload shapes — mirrors of `src-tauri/src/dto.rs`
 * ---------------------------------------------------------------------- */

/** How the app renders itself. `system` follows the OS appearance. */
export type ThemePreference = "system" | "light" | "dark"

/** Where the notification window appears. `menuBar` is macOS-only. */
export type NudgePlacement = "menuBar" | "topRight"

/** When the menu bar shows the free-disk-space number. */
export type DiskSpaceDisplay = "always" | "whenLow" | "never"

/** Selected usage-milestone percentages for one window class. */
export type Milestones = number[]

/** Every persisted preference. Mirrors Rust `AppSettings`. */
export interface AppSettings {
  theme: ThemePreference
  /** Calendar days of activity the popover list shows. */
  activityWindowDays: number
  /** Days to keep local session data. `-1` keeps it until explicit deletion. */
  sessionDataRetentionDays: number
  /** False until the first-run flow finishes. */
  onboardingCompleted: boolean
  /** Recorded; applied by the platform at next launch. */
  launchAtLogin: boolean
  /** Whether the menu-bar or system-tray icon is visible. */
  trayIconVisible: boolean
  /** Whether the app is visible in the macOS Dock. */
  dockIconVisible: boolean
  /** Whether the shell may install and restart for updates on its schedule. */
  autoUpdate: boolean
  /**
   * Whether background discovery and indexing are paused.
   *
   * Paused stops scheduled scans only. An explicit rescan still runs, and
   * everything already indexed stays browsable.
   */
  discoveryPaused: boolean
  /**
   * The master switch for desktop notifications. Off means nothing is
   * delivered, whatever the per-kind preferences say.
   */
  notificationsEnabled: boolean
  /** Notify before an automatic update installs and restarts the app. */
  notifyUpdateAvailable: boolean
  /** Notify the first time a scan fails in this run of the app. */
  notifyScanFailure: boolean
  /** Where the notification window appears. */
  nudgePlacement: NudgePlacement
  /** Seconds a nudge stays before dismissing itself (3–30). */
  nudgeAutoDismissSecs: number
  /** Whether a nudge may play the notification chime. */
  notificationSound: boolean
  /**
   * Whether Focus and Do Not Disturb suppress automated nudges. Off by
   * default: the macOS Focus check needs its own authorization prompt, so
   * the check is an opt-in.
   */
  nudgesRespectDnd: boolean
  /** When the menu bar shows the free-disk-space number. */
  diskSpaceDisplay: DiskSpaceDisplay
  /** Free space, in GB, below which the disk counts as low (5–2000). */
  diskSpaceThresholdGb: number
  /** Notify once each time free space drops below the threshold. */
  notifyDiskSpaceLow: boolean
  /** Five-hour-window milestones. Only fire while live usage is enabled. */
  milestones5h: Milestones
  /** Weekly-window milestones. */
  milestonesWeekly: Milestones
  /**
   * The per-feature online opt-out for live usage limits. On by default, once
   * first-run setup is complete.
   *
   * On, antiburn asks each provider directly for the reader's current usage,
   * periodically, using the credentials the reader's own coding tools already
   * hold — that is antiburn going online as the reader, not a server of ours,
   * and it is ordinary traffic rather than something that needs a separate
   * go-ahead — and milestone notifications become able to fire. Off, antiburn
   * makes none of these requests and has no plan limits to show; this is the
   * setting for a reader who wants no background traffic at all.
   */
  liveUsageEnabled: boolean
  /**
   * Canonical ids of the providers whose meter the reader turned off.
   *
   * A narrower form of the switch above: that one stops every provider, this
   * one stops the named provider. Both stop requests, so a hidden provider
   * also stops its milestone notifications.
   */
  liveUsageHiddenProviders: string[]
  /**
   * Discovery slugs of the coding agents the reader turned off.
   *
   * A display filter only: a disabled agent's sessions stay indexed and
   * analyzed, but the session list does not show them. An agent absent from
   * this list shows by default, so a newly detected agent surfaces on its
   * own.
   */
  disabledAgents: string[]
  /**
   * The consented analytics channel.
   *
   * On by default for a new install. No build without the analytics feature
   * and an injected endpoint transmits at all; see `AppInfo.analyticsSupported`.
   */
  analyticsEnabled: boolean
  /**
   * Whether the popover's usage-limits bar shows its per-provider rows.
   * This display preference never gates a fetch. It defaults open and stays
   * where the reader last left it.
   */
  overviewLimitsExpanded: boolean
  /**
   * Whether a session's Skills & MCPs table shows every row, rather than only
   * the first group behind a "Show more" button. One answer for the whole
   * app, across every session and every launch. It defaults closed and stays
   * where the reader last left it.
   */
  skillsMcpExpanded: boolean
  /** The metric shown in each activity-session badge. */
  sessionBadgeMetric: "cost" | "weeklyPercent" | "fiveHourPercent"
  /**
   * The selected Sessions sidebar filter, as its persisted id (see
   * `sessionFilterId`/`parseSessionFilterId` in `lib/sessionFilters.ts`). An
   * id this release does not recognize parses back to `all`.
   */
  sessionFilter: string
}

/** Where the app came from. Mirrors Rust `AppInfo`. */
export interface AppInfo {
  appVersion: string
  /** True when the binary enables Rust debug assertions. */
  debugBuild: boolean
  /** CPU architecture this binary was compiled for, e.g. `aarch64`. */
  arch: string
  pricingCatalogVersion: string
  schemaVersion: number
  dataDir: string
  /** Sessions currently in the local index. */
  indexedSessions: number
  /** Size of the local database on disk, in bytes. */
  databaseBytes: number
  /**
   * Whether this build can check for updates at all.
   *
   * Derived shell-side from *real* plugin-registration state plus a configured
   * signing key — never from a compile-time flag — so a build that cannot
   * update never renders a control implying it can.
   */
  updatesSupported: boolean
  /** Whether this build includes a configured analytics client. */
  analyticsSupported: boolean
  /** Whether `ANTIBURN_ANALYTICS_ENABLED=false` overrides the stored setting. */
  analyticsEnvironmentDisabled: boolean
  /** Who receives those events. Null when this build has no endpoint. */
  analyticsOperator: string | null
}

/** One row of the activity list, before it is shaped for presentation. */
export interface ModelRunPayload {
  model: string
  thinkingMode?: string
}

export interface ActivityEntryPayload {
  agent: string
  sessionId: string
  repo: string
  timestamp: string
  isActive: boolean
  surface: string
  wslDistro: string | null
  title: string | null
  hasForkParent: boolean
  forkChildCount: number
  /** Cost of the parent transcript plus every sub-agent the session launched. */
  cost: SessionCostComponents | null
  /** Every model that contributed billable tokens. */
  models: string[]
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns: ModelRunPayload[]
}

/** Identity of one local session, as the analysis view carries it. */
export interface SessionIdentityPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
}

/** One revisioned request to show a session in the retained main window. */
export interface MainWindowSessionRequest {
  revision: number
  target: SessionIdentityPayload
}

export type MainWindowSectionId = "overview" | "activity" | "burnChecks"

/** One revisioned request to select a retained main-window section. */
export interface MainWindowSectionRequest {
  revision: number
  section: MainWindowSectionId
}

/** One end of a local fork relation. */
export interface SessionRelationPayload {
  identity: SessionIdentityPayload
  title: string | null
  available: boolean
}

/** Direct fork relations for one session. */
export interface SessionRelationsPayload {
  title: string | null
  parent: SessionRelationPayload | null
  children: SessionRelationPayload[]
}

/** One sub-agent an orchestrator launched. */
export interface SubagentMemberPayload {
  agent: string
  subagentId: string
  label: string
  /** The sub-agent's own priced cost, or null when it is not yet analyzed. */
  cost: SessionCostComponents | null
  /** Billable tokens that back `cost`. */
  tokens: BillableTokens | null
  /** Unix seconds of the sub-agent's first transcript event, or null when unknown. */
  startedAtEpoch: number | null
  /** Every model/thinking-mode pair the sub-agent used. */
  modelRuns: ModelRunPayload[]
}

/** The sub-agent picture for one session. */
export interface OrchestrationPayload {
  orchestrating: boolean
  orchestratorAgent: string
  orchestratorSessionId: string
  subagentCount: number
  members: SubagentMemberPayload[]
}

/** Everything the session-analysis surface renders for one session. */
export interface SessionAnalysisPayload {
  summary: ActiveSessionsSummary | null
  supportsAnalysis: boolean
  title: string | null
  wslDistro: string | null
  isActive: boolean
  /** Cost of the parent transcript plus every sub-agent it launched. */
  cost: SessionCostComponents | null
  /** Cost of the parent transcript, without any sub-agent. */
  topLevelCost: SessionCostComponents | null
  /** Cost of every sub-agent this session launched, combined. The value is
   * `null` when the session has no sub-agent, or when no sub-agent could
   * be priced. */
  subagentsCost: SessionCostComponents | null
  /** Billable tokens that back `cost`. The count sums the parent transcript
   * and every sub-agent. */
  inclusiveTokens: BillableTokens | null
  /** Billable tokens that back `subagentsCost`. The count sums every
   * sub-agent. The value is `null` when the session has no sub-agent. */
  subagentsTokens: BillableTokens | null
  /** Where the spend behind `cost` went. The same subject as `cost`. */
  efficiency: SessionEfficiency | null
  models: string[]
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns: ModelRunPayload[]
  orchestration: OrchestrationPayload | null
  relations: SessionRelationsPayload | null
  /** The provider's own transcript, for the reveal action. */
  sourcePath: string | null
  /** The stored absolute working directory. */
  projectPath: string | null
  /** Unix seconds of this session's own first transcript event, or null when
   * unknown. The sub-agent roster uses it to show each member's start as
   * elapsed time from the session start. */
  startedAtEpoch: number | null
  /** True when no published row set exists yet for this session, so every
   * other field above is a placeholder rather than a real read. The worker
   * fills the gap on its own; the view should show an indexing state, not
   * an empty-transcript state. */
  analysisPending: boolean
  /** True when the fields above come from a published fence that a fresher
   * pass is already queued or running behind, or whose transcript has since
   * moved on. The data on screen is real, just not the latest — unlike
   * `analysisPending`, which means there is nothing to show yet. */
  analysisStale: boolean
}

/** One repository row. Mirrors Rust `RepositoryItem`. */
export interface RepositoryItemPayload {
  key: string
  repoName: string
  fullName: string
  status: string
  repoRoot: string | null
  suspectedPath: string | null
  worktreeCount: number
  sessionCount: number
  wslDistro: string | null
  enabled: boolean
}

/** What one agent's last pass saw. */
export interface AgentScanState {
  agent: string
  lastCompletedAt: string | null
  sessionsSeen: number
}

/** What a scan is doing, or last did. Mirrors Rust `ScanStatus`. */
export interface ScanStatus {
  running: boolean
  completedAgents: number
  totalAgents: number
  sessions: number
  finishedAt: string | null
  /** True when the last pass stopped because it was asked to, not because it failed. */
  cancelled: boolean
  error: string | null
  agents: AgentScanState[]
  /** True when this pass indexed a session the list has never shown, or evicted a rejected one. */
  listChanged: boolean
  /**
   * R5: how many sessions the last pass re-described — new, or a moved
   * cursor — never a row it reused verbatim. Tells an idle pass from a
   * productive one without inferring it from `listChanged` alone.
   */
  reDescribed: number
}

/**
 * Whether the local database is still accepting writes. Mirrors Rust
 * `StorageHealthStatus`.
 *
 * A store that has stopped accepting writes looks exactly like a quiet machine
 * from the outside — the list keeps rendering, it just stops changing — so the
 * shell reports it and the popover surfaces it.
 */
export interface StorageHealthPayload {
  failing: boolean
  /** What failed, in the store's own words. Present only while `failing`. */
  message: string | null
}

/** One state in the update lifecycle. Mirrors Rust `UpdateStatus`. */
export interface UpdateStatusPayload {
  kind:
    | "current"
    | "available"
    | "downloading"
    | "installing"
    | "installed"
    | "failed"
    | "unsupported"
  version: string | null
  message: string | null
  checkedAt: string
  /** True when the shell schedule started this operation. */
  automatic: boolean
  downloadedBytes: number | null
  totalBytes: number | null
  failureOperation: "check" | "install" | null
  /** Monotonic process-local order for event and snapshot reconciliation. */
  revision: number
}

/* -------------------------------------------------------------------------
 * Nudge payloads — mirrors `src-tauri/crates/nudge/src/model.rs`
 *
 * The nudge crate is the *mechanism* behind the floating notification window:
 * it owns that window and its placement, and knows nothing about why a nudge
 * fires. These shapes are the whole contract between it and `NudgeView`.
 * ---------------------------------------------------------------------- */

/** Window label the shell gives the notification window. Mirrors `NUDGE_LABEL`. */
export const NUDGE_WINDOW_LABEL = "nudge"

/**
 * What surfaced a nudge. Mirrors Rust `NudgeKind`.
 *
 * The view is deliberately kind-agnostic — it draws whatever fields arrived —
 * so a new trigger is a new variant here and a new payload builder in Rust,
 * with no change to the notification UI.
 */
export type NudgeKind =
  | "updateAvailable"
  | "scanFailure"
  | "diskSpaceLow"
  | "usageMilestone"
  | "menuBarLocation"
  | "test"

/** Visual tone — informational, positive, or attention. Mirrors Rust `NudgeTone`. */
export type NudgeTone = "info" | "success" | "warning"

/**
 * Optional structured target carried by a CTA and echoed back to the shell when
 * it is clicked, so the handler acts on what the nudge was actually about.
 */
export type NudgeActionTarget =
  | { type: "update"; expectedVersion: string }
  | { type: "providerUsage"; provider: string; accountKey: string | null }
  | { type: "session"; agent: string; sessionId: string; environment: string | null }

/** One actionable CTA on the notification. Mirrors Rust `NudgeAction`. */
export interface NudgeAction {
  /** Stable identifier routed back to the shell on click. */
  id: string
  label: string
  /** Rendered as the emphasized button, and always last (macOS convention). */
  primary: boolean
  target?: NudgeActionTarget
}

/**
 * Payload of the `nudge:show` event. Mirrors Rust `Nudge`.
 *
 * Empty optionals are omitted on the wire (`skip_serializing_if` in Rust), so
 * `recommendations` arrives absent rather than as `[]`.
 */
export interface Nudge {
  id: string
  kind: NudgeKind
  tone: NudgeTone
  title: string
  /** Short summary that stays visible in collapsed and expanded states. */
  subtitle: string
  /** Detailed copy revealed when the notification expands. */
  description: string
  /** Who or what this is about, when it is about one. Never drawn; the shell acts on it. */
  actor?: string
  /** Suggested steps, revealed when the notification expands on hover. */
  recommendations?: string[]
  actions: NudgeAction[]
  /** Auto-dismiss timeout in milliseconds; absent means sticky until acted on. */
  timeoutMs?: number
}
