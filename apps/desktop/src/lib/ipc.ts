// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * The typed edge of the shell's IPC surface.
 *
 * Every command the Rust side exposes has exactly one wrapper here, and the
 * views call nothing else. That keeps the command names in one file, gives the
 * payloads a declared shape, and means the whole surface can be mocked at one
 * module boundary in tests.
 *
 * The bundle also has to load in a plain browser (`pnpm dev:web`, unit tests)
 * where no shell is attached. Every wrapper therefore reports *absence* rather
 * than throwing, so views render a degraded state instead of crashing.
 *
 * None of these payloads comes from a service of ours. They are produced on
 * this machine by the local engine — see `tests/no-exfiltration.test.ts`,
 * which enforces that no code in this app's webview opens a network
 * connection on its own account.
 */

import { invoke, isTauri } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

import type { SettingsPane } from "./settingsPanes"
import type { FolderAccessOutcome, FolderPermissions, ProbeRecord } from "./types/repository"
import type {
  ActiveSessionsSummary,
  BillableTokens,
  SessionCostComponents,
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

/** Which usage-milestone thresholds notify, for one window class. */
export interface Milestones {
  at50: boolean
  at75: boolean
  at90: boolean
}

/** Every persisted preference. Mirrors Rust `AppSettings`. */
export interface AppSettings {
  theme: ThemePreference
  /** Calendar days of activity the popover list shows. */
  activityWindowDays: number
  /** False until the first-run flow finishes. */
  onboardingCompleted: boolean
  /** Recorded; applied by the platform at next launch. */
  launchAtLogin: boolean
  /** Whether the shell may check the release feed on its own schedule. */
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
  /** Notify when an automatic update check finds a newer version. */
  notifyUpdateAvailable: boolean
  /** Notify the first time a scan fails in this run of the app. */
  notifyScanFailure: boolean
  /** Where the notification window appears. */
  nudgePlacement: NudgePlacement
  /** Seconds a nudge stays before dismissing itself (3–30). */
  nudgeAutoDismissSecs: number
  /** Whether a nudge may play the notification chime. */
  notificationSound: boolean
  /** When the menu bar shows the free-disk-space number. */
  diskSpaceDisplay: DiskSpaceDisplay
  /** Free space, in GB, below which the disk counts as low (5–2000). */
  diskSpaceThresholdGb: number
  /** Notify once each time free space drops below the threshold. */
  notifyDiskSpaceLow: boolean
  /** Notify when sustained spend is unusually fast for this machine. */
  notifyUsageAnomalies: boolean
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
   * The consented analytics channel (D-027, deviations register D-28).
   *
   * On by default for a new install, which meets the control on the first-run
   * Ready screen before anything can be sent; off for a store that finished
   * onboarding under copy promising no analytics at all. Nothing is
   * transmitted until onboarding completes, and no build without an injected
   * endpoint transmits at all — see `AppInfo.usageAnalyticsSupported`.
   */
  usageAnalyticsEnabled: boolean
  /**
   * Whether the popover's usage-limits bar shows its per-provider rows.
   * This display preference never gates a fetch. It defaults open and stays
   * where the reader last left it.
   */
  overviewLimitsExpanded: boolean
}

/** Where the app came from. Mirrors Rust `AppInfo`. */
export interface AppInfo {
  appVersion: string
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
  /**
   * Whether this build can send consented analytics at all.
   *
   * Same rule as `updatesSupported`, for the same reason: derived shell-side
   * from the build that is actually running, never from a compile-time flag.
   * False in every development build and every build from a clean checkout of
   * this repository, because the endpoint is injected at build time and this
   * tree carries none.
   */
  usageAnalyticsSupported: boolean
  /** Who receives those events, in the reader's own words. Null when this
   *  build has no endpoint. */
  usageAnalyticsOperator: string | null
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

/** Identity of one local session, as the analytics view carries it. */
export interface SessionIdentityPayload {
  agent: string
  sessionId: string
  wslDistro: string | null
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
}

/** The sub-agent picture for one session. */
export interface OrchestrationPayload {
  orchestrating: boolean
  orchestratorAgent: string
  orchestratorSessionId: string
  subagentCount: number
  members: SubagentMemberPayload[]
}

/** Everything the session-analytics surface renders for one session. */
export interface SessionAnalyticsPayload {
  summary: ActiveSessionsSummary | null
  supportsAnalytics: boolean
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
  models: string[]
  /** Parent model runs followed by runs used only by sub-agents. */
  modelRuns: ModelRunPayload[]
  orchestration: OrchestrationPayload | null
  relations: SessionRelationsPayload | null
  /** The provider's own transcript, for the reveal action. */
  sourcePath: string | null
}

/**
 * How well the app can describe one provider's usage. Mirrors Rust
 * `ProviderUsageState`.
 *
 * `live` and `detected` are part of the contract but are **never** produced by
 * this build: a session transcript records what was spent, not what remains,
 * so neither an allowance nor a bare "signed in" signal can come out of it.
 * They are declared — and rendered — so a later, separately reviewed passive
 * source cannot arrive to a view that has no branch for it.
 */
export type ProviderUsageState = "live" | "estimated" | "observed" | "detected" | "unknown"

/** Whether a provider's newest local evidence still describes now. */
export type ProviderUsageStaleness = "fresh" | "stale" | "unknown"

/**
 * One provider's totals over one window.
 *
 * There is no percentage, allowance, remaining balance, or reset here, and
 * that is deliberate — see `ProviderUsageState`.
 */
export interface ProviderUsageWindowPayload {
  /** Fresh prompt tokens plus prompt-cache writes. */
  tokensIn: number
  tokensOut: number
  /** Prompt-cache reads, billed at their own rate. */
  cacheRead: number
  /**
   * On-device estimate for the models that could be priced, or null when none
   * could. A `observed` state means this is a floor rather than a total.
   */
  estimatedUsd: number | null
  /** Sessions that contributed. One session can count under two providers. */
  sessionCount: number
}

/** The three windows every provider is summarized over. */
export interface ProviderUsageWindowsPayload {
  today: ProviderUsageWindowPayload
  week: ProviderUsageWindowPayload
  month: ProviderUsageWindowPayload
}

/** Everything the usage surfaces show about one provider. */
export interface ProviderUsagePayload {
  /** Canonical id (`anthropic`, `openai`, `unknown`, …) — a key, not copy. */
  provider: string
  displayName: string
  state: ProviderUsageState
  staleness: ProviderUsageStaleness
  windows: ProviderUsageWindowsPayload
  lastActivityAt: string | null
}

/** Local provider usage as one snapshot. Mirrors Rust `ProviderUsageSummary`. */
export interface ProviderUsageSummaryPayload {
  providers: ProviderUsagePayload[]
  generatedAt: string
}

/* -------------------------------------------------------------------------
 * Live provider usage — the provider's own limit figures.
 *
 * A second payload rather than fields on `ProviderUsageSummaryPayload`, and
 * for the same reason on this side as on the shell's: that type's guarantee is
 * that it carries no percentage, allowance, or reset, and a test proves it.
 * The views layer the two.
 *
 * Still nothing is fetched here, in the webview. These figures were fetched
 * by the *agent* (optionally at antiburn's request — see Settings → Usage),
 * which cached them on this machine; the shell reads that file and this
 * module receives it over IPC like everything else. See
 * `tests/no-exfiltration.test.ts`.
 * ---------------------------------------------------------------------- */

/** Marks figures stated directly by a provider. Mirrors Rust `LiveUsageSupport`. */
export type LiveUsageSupport = "live"

/** Whether a live reading still describes now. */
export type LiveUsageFreshness = "fresh" | "stale"

/**
 * One provider-reported allowance.
 *
 * Every field through `resetsAt` is either something the provider stated or
 * null. In particular `usedPercent` is null — never 0 — when the provider did
 * not say, because a meter reading empty and a meter reading unknown are
 * different facts and the views render them differently. The last two fields
 * are the exception: both are derived from this window's own sample history
 * rather than stated by anyone.
 */
export interface LiveUsageWindowPayload {
  /** `five-hour`, `seven-day`, `weekly-<model>`, or the provider's own name. */
  id: string
  /** `primaryShort` | `primaryLong` | `supplemental` | the provider's word. */
  role: string
  /** `rolling` | `weekly` | `daily` | `monthly` | `billingCycle` | provider's. */
  kind: string
  /** The model a scoped window covers, when it covers one. */
  scopeModel: string | null
  /** Consumed capacity, 0–100. Never remaining. */
  usedPercent: number | null
  startsAt: string | null
  resetsAt: string | null
  /**
   * Whether trustworthy history shows non-zero usage anywhere in this
   * window's current allowance period. Consulted only for a supplemental,
   * model-scoped window — see `isUsageWindowVisible` in `liveUsage.ts`.
   */
  hasNonzeroUsageInCurrentPeriod: boolean
  /** What this window's own history supports saying about it. */
  forecast: LiveUsageForecastPayload
}

/**
 * The derived half of a window. Mirrors Rust `LiveUsageForecast`.
 *
 * Exactly one of `unavailableReason` and the value fields is populated, and
 * that is not a formality: "we have not seen enough of your week to say" and
 * "you are on track" are different answers, only one of them reassuring. A
 * null value here always means the former, so the views say so rather than
 * rendering a blank.
 */
export interface LiveUsageForecastPayload {
  /** `stale` | `transition` | `sparseHistory`, or null when there is one. */
  unavailableReason: string | null
  /** `low` | `medium` | `high`. */
  confidence: string | null
  /** Percentage points of the allowance consumed per hour. */
  consumptionRate: number | null
  /** Current rate over the rate that lands exactly at the reset. >1 overshoots. */
  paceRatio: number | null
  /** Last half hour's rate over the last two hours'. >1 is speeding up. */
  paceTrend: number | null
  /** When the allowance runs out at the current rate. */
  runwayAt: string | null
  /** Points of this window consumed since the reader's local midnight. */
  usedToday: number | null
}

/** Metered spend alongside the allowance. */
export interface LiveExtraUsagePayload {
  /** Whether the account permits this path. `false` differs from unknown. */
  enabled: boolean
  usedPercent: number | null
  used: number | null
  remaining: number | null
  limit: number | null
  currency: string | null
}

/** One provider account's live usage. Mirrors Rust `LiveProviderUsage`. */
export interface LiveProviderUsagePayload {
  /** Canonical id, matching `ProviderUsagePayload.provider` so the two join. */
  provider: string
  displayName: string
  support: LiveUsageSupport
  freshness: LiveUsageFreshness
  /** Where the figures came from. Safe to display; carries no account id. */
  sourceLabel: string
  /** When the *provider fact* was observed — not when the app read it. */
  observedAt: string
  windows: LiveUsageWindowPayload[]
  extraUsage: LiveExtraUsagePayload | null
}

/** A source that failed, in terms a reader can act on. */
export interface LiveUsageSourceErrorPayload {
  source: string
  /**
   * The canonical id of the provider the source answers for. A failed source
   * contributes no entry to `providers`, so this is how the views keep a
   * section for the provider the failure left without a reading. Empty on a
   * snapshot cached before the field existed.
   */
  provider: string
  /** The provider's display name, for example "Claude". Empty like `provider`. */
  displayName: string
  /** `authentication` | `rateLimited` | `schema` | `unavailable`. */
  category: string
}

/**
 * Live provider usage as one snapshot. Mirrors Rust `LiveUsageSummary`.
 *
 * An empty `providers` list is the ordinary state and the views render
 * nothing. `errors` is separate so "nothing found" and "something broke" never
 * look alike.
 */
export interface LiveUsageSummaryPayload {
  providers: LiveProviderUsagePayload[]
  errors: LiveUsageSourceErrorPayload[]
  generatedAt: string
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

/** The outcome of one update check. Mirrors Rust `UpdateStatus`. */
export interface UpdateStatusPayload {
  kind: "current" | "available" | "failed" | "unsupported"
  version: string | null
  message: string | null
  checkedAt: string
  /** True when the shell's own schedule ran the check rather than a reader. */
  automatic: boolean
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
  | "usageAnomaly"
  | "usageMilestone"
  | "test"

/** Visual tone — informational, positive, or attention. Mirrors Rust `NudgeTone`. */
export type NudgeTone = "info" | "success" | "warning"

/**
 * Optional structured target carried by a CTA and echoed back to the shell when
 * it is clicked, so the handler acts on what the nudge was actually about.
 */
export type NudgeActionTarget =
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
 * `recommendations` arrives absent rather than as `[]` — the view defaults it.
 */
export interface Nudge {
  id: string
  kind: NudgeKind
  tone: NudgeTone
  title: string
  /** Who or what this is about, when it is about one. Never drawn; the shell acts on it. */
  actor?: string
  /** Why this is being shown. Rendered only when present. */
  reason?: string
  /** Suggested steps, revealed when the notification expands on hover. */
  recommendations?: string[]
  actions: NudgeAction[]
  /** Auto-dismiss timeout in milliseconds; absent means sticky until acted on. */
  timeoutMs?: number
}

/* -------------------------------------------------------------------------
 * Presence
 * ---------------------------------------------------------------------- */

/** True when the bundle is running inside the antiburn shell. */
export function hasShell(): boolean {
  return isTauri()
}

/** What settings look like before anything has been stored, or without a shell. */
export const DEFAULT_SETTINGS: AppSettings = {
  theme: "system",
  activityWindowDays: 7,
  onboardingCompleted: false,
  launchAtLogin: true,
  autoUpdate: true,
  discoveryPaused: false,
  notificationsEnabled: true,
  notifyUpdateAvailable: true,
  notifyScanFailure: true,
  nudgePlacement: "menuBar",
  nudgeAutoDismissSecs: 10,
  notificationSound: true,
  diskSpaceDisplay: "whenLow",
  diskSpaceThresholdGb: 50,
  notifyDiskSpaceLow: true,
  notifyUsageAnomalies: true,
  milestones5h: { at50: true, at75: true, at90: true },
  milestonesWeekly: { at50: true, at75: true, at90: true },
  liveUsageEnabled: true,
  usageAnalyticsEnabled: true,
  overviewLimitsExpanded: true,
}

/* -------------------------------------------------------------------------
 * Commands
 * ---------------------------------------------------------------------- */

/**
 * Version stamp of the engine's bundled pricing catalog.
 *
 * The end-to-end proof that the shell links the local engine: the value
 * originates in `antiburn_local::pricing::PRICING_CATALOG_VERSION`.
 */
export async function engineCatalogVersion(): Promise<string | null> {
  if (!hasShell()) return null
  return invoke<string>("engine_catalog_version")
}

/**
 * Opens (or refocuses) the standalone settings window.
 *
 * `pane` is a request for a section — an attention banner uses it to land a
 * reader on the pane that can fix what they were told about. Omitting it leaves
 * the window wherever it was.
 */
export async function openSettingsWindow(pane?: SettingsPane): Promise<void> {
  if (!hasShell()) return
  await invoke("open_settings_window", { pane: pane ?? null })
}

/**
 * The pane a caller asked for, taken once, as the settings window mounts.
 *
 * Taken rather than read: a window opened later must not jump to a pane
 * somebody asked for an hour ago.
 */
export async function takeSettingsPane(): Promise<string | null> {
  if (!hasShell()) return null
  return invoke<string | null>("take_settings_pane")
}

/**
 * Asks the shell to close whichever window is calling (⌘W).
 *
 * A close *request*, not a destroy: the shell intercepts `CloseRequested` for
 * every window and hides instead (see `src-tauri/src/lib.rs`), so this takes
 * exactly the same path as the title-bar close button. Needs
 * `core:window:allow-close` in `capabilities/default.json` — the ACL's
 * `core:window:default` set is read-only.
 *
 * Used by the two windows that own a ⌘W: settings, and the first-run flow.
 * Both are accessory-app windows with no application menu, so the standard
 * shortcut has no owner unless the view handles it.
 */
export async function closeCurrentWindow(): Promise<void> {
  if (!hasShell()) return
  const { getCurrentWindow } = await import("@tauri-apps/api/window")
  await getCurrentWindow().close()
}

/**
 * Quit antiburn.
 *
 * Routed through the shell rather than closing windows, because a menu-bar app
 * outlives its windows: only `exit(0)` distinguishes a deliberate quit from the
 * window closes the shell suppresses.
 */
export async function quitApp(): Promise<void> {
  if (!hasShell()) return
  await invoke("quit_app")
}

/**
 * Post the settings pane's test notification.
 *
 * The one notification a view can cause, and only through this explicit
 * command: it takes the same delivery path as every real kind, bypassing only
 * the master preference — pressing the button is the permission.
 */
export async function postTestNotification(): Promise<void> {
  if (!hasShell()) return
  await invoke("post_test_notification")
}

/**
 * Dismisses the tray popover.
 *
 * A shell command rather than `getCurrentWindow().hide()`, because the shell
 * gates its background scan on the popover being visible and a window hidden
 * behind its back would leave that gate open.
 */
export async function hidePopover(): Promise<void> {
  if (!hasShell()) return
  await invoke("hide_popover")
}

/**
 * Run `action` (typically a native dialog) with the popover held on screen.
 *
 * The popover hides itself on focus loss, and a dialog it opens takes focus by
 * design. The hold tells the shell that this particular focus loss is part of
 * the popover's own flow; releasing it hands focus back. Released in a
 * `finally`, so a rejected dialog can never leave the window unhideable.
 */
export async function withPopoverHold<T>(action: () => Promise<T>): Promise<T> {
  if (!hasShell()) return action()
  await invoke("begin_popover_hold")
  try {
    return await action()
  } finally {
    await invoke("end_popover_hold").catch(() => undefined)
  }
}

/**
 * Resize the popover to the height the view now on screen needs.
 *
 * The shell clamps the value, so this is a request rather than an instruction.
 * `animate` is decided here because the reduced-motion preference is a
 * webview-side media query and a height change is motion.
 */
export async function setPopoverHeight(height: number, animate: boolean): Promise<void> {
  if (!hasShell()) return
  await invoke("set_popover_height", { height, animate })
}

/** Where the app came from and what it is running against. */
export async function appInfo(): Promise<AppInfo | null> {
  if (!hasShell()) return null
  return invoke<AppInfo>("app_info")
}

/** Every persisted preference. */
export async function getSettings(): Promise<AppSettings> {
  if (!hasShell()) return DEFAULT_SETTINGS
  return invoke<AppSettings>("get_settings")
}

/** Replace every preference, returning what was actually stored. */
export async function setSettings(settings: AppSettings): Promise<AppSettings> {
  if (!hasShell()) return settings
  return invoke<AppSettings>("set_settings", { settings })
}

/**
 * One interaction worth counting, in the shell's own closed vocabulary.
 *
 * Deliberately not `{ name: string; properties: Record<string, string> }`.
 * The shell deserialises this into a Rust enum and refuses anything outside
 * it, so the set of things that can ever be reported is fixed there rather
 * than here — no call site in this webview can widen it, and none can put a
 * path, a title, or a repository name into a payload. See
 * `src-tauri/src/usage_analytics/event.rs`.
 */
export type Interaction =
  | { kind: "sessionOpened"; agent: string; environment: "native" | "wsl" }
  | {
      kind: "usageViewed"
      providers: number
      evidence: "live" | "estimated_only" | "none"
    }

/**
 * Report one interaction. Fire-and-forget, and silent on failure.
 *
 * The shell decides whether anything is actually recorded: this is inert
 * unless the build has an endpoint, the reader has finished onboarding, and
 * the switch in Settings → Privacy is on. Callers do not check, so there is
 * one gate rather than two that can drift apart.
 */
export function noteInteraction(interaction: Interaction): void {
  if (!hasShell()) return
  void invoke("note_interaction", { interaction }).catch(() => {
    // Analytics must never surface an error into something the reader asked
    // for. A dropped event is not worth a line of user-facing text.
  })
}

/** Commit the first-run choices and finish onboarding in one shell transition. */
export async function finishOnboarding(
  activityWindowDays: number,
  launchAtLogin: boolean,
  usageAnalyticsEnabled: boolean,
): Promise<AppSettings> {
  if (!hasShell()) {
    return {
      ...DEFAULT_SETTINGS,
      activityWindowDays,
      launchAtLogin,
      usageAnalyticsEnabled,
      onboardingCompleted: true,
    }
  }
  return invoke<AppSettings>("finish_onboarding", {
    activityWindowDays,
    launchAtLogin,
    usageAnalyticsEnabled,
  })
}

/** The sessions to show in the popover, newest first. */
export async function listRecentSessions(windowDays?: number): Promise<ActivityEntryPayload[]> {
  if (!hasShell()) return []
  return invoke<ActivityEntryPayload[]>("list_recent_sessions", {
    windowDays: windowDays ?? null,
  })
}

/** One session's analysis, sub-agent roster, and fork relations. */
export async function getSessionAnalytics(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): Promise<SessionAnalyticsPayload | null> {
  if (!hasShell()) return null
  return invoke<SessionAnalyticsPayload>("get_session_analytics", {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
  })
}

/** One sub-agent's own analysis. */
export async function getSubagentAnalytics(
  agent: string,
  parentSessionId: string,
  subagentId: string,
  wslDistro?: string | null,
): Promise<SessionAnalyticsPayload | null> {
  if (!hasShell()) return null
  return invoke<SessionAnalyticsPayload>("get_subagent_analytics", {
    agent,
    parentSessionId,
    subagentId,
    wslDistro: wslDistro ?? null,
  })
}

/**
 * Per-provider token and cost totals derived from the sessions on this machine.
 *
 * The reader's UTC offset travels with the request because "today" and "this
 * month" are *their* calendar days, and the shell cannot resolve a local offset
 * reliably from inside a multi-threaded process. `getTimezoneOffset` returns
 * minutes *behind* UTC, so it is negated on the way out.
 *
 * Without a shell the answer is an empty snapshot rather than null: the usage
 * surfaces have an honest empty state and no separate "no shell" state.
 */
export async function getProviderUsage(): Promise<ProviderUsageSummaryPayload> {
  if (!hasShell()) return EMPTY_PROVIDER_USAGE
  return invoke<ProviderUsageSummaryPayload>("get_provider_usage", {
    utcOffsetMinutes: -new Date().getTimezoneOffset(),
  })
}

/** What provider usage looks like with nothing to report. */
export const EMPTY_PROVIDER_USAGE: ProviderUsageSummaryPayload = {
  providers: [],
  generatedAt: "",
}

/**
 * The last provider limit snapshot. This command does not contact a provider.
 */
export async function getLiveUsage(): Promise<LiveUsageSummaryPayload> {
  if (!hasShell()) return EMPTY_LIVE_USAGE
  // Coerced rather than passed through: a shell that answered with nothing is
  // the same fact as a shell with no source, and the views should not each
  // carry a null branch for a state that has a perfectly good empty value.
  return (
    (await invoke<LiveUsageSummaryPayload | null>("get_live_usage", {
      utcOffsetMinutes: -new Date().getTimezoneOffset(),
    })) ?? EMPTY_LIVE_USAGE
  )
}

/**
 * Ask each provider for current limits and replace the cached snapshot.
 *
 * The offset makes "used today" use the reader's calendar day.
 */
export async function refreshLiveUsage(): Promise<LiveUsageSummaryPayload> {
  if (!hasShell()) return EMPTY_LIVE_USAGE
  return (
    (await invoke<LiveUsageSummaryPayload | null>("refresh_live_usage", {
      utcOffsetMinutes: -new Date().getTimezoneOffset(),
    })) ?? EMPTY_LIVE_USAGE
  )
}

/** What live usage looks like with no source able to say anything. */
export const EMPTY_LIVE_USAGE: LiveUsageSummaryPayload = {
  providers: [],
  errors: [],
  generatedAt: "",
}

/** Return the newest recent transcript write as epoch seconds. */
export async function getLatestSessionActivity(): Promise<number | null> {
  if (!hasShell()) return null
  return invoke<number | null>("get_latest_session_activity")
}

/** Run a scan now, unless one is already in flight. */
export async function scanNow(activityWindowDays?: number): Promise<ScanStatus | null> {
  if (!hasShell()) return null
  return activityWindowDays === undefined
    ? invoke<ScanStatus>("scan_now")
    : invoke<ScanStatus>("scan_now", { activityWindowDays })
}

/** What the current or last scan is doing. */
export async function getScanStatus(): Promise<ScanStatus | null> {
  if (!hasShell()) return null
  return invoke<ScanStatus>("get_scan_status")
}

/**
 * Ask the scan in flight to stop at its next phase boundary.
 *
 * Everything it already found stays indexed: a cancelled pass is a shorter
 * pass, not an undone one.
 */
export async function cancelScan(): Promise<ScanStatus | null> {
  if (!hasShell()) return null
  return invoke<ScanStatus>("cancel_scan")
}

/**
 * Forget all session data in antiburn's local store, returning how many sessions went.
 *
 * antiburn's own records only. The agents' transcripts are never touched, and a
 * later scan finds all of this again.
 */
export async function clearLocalIndex(): Promise<number> {
  if (!hasShell()) return 0
  return invoke<number>("clear_local_index")
}

/**
 * Whether the local database is still accepting writes.
 *
 * Falls back to healthy rather than to null: "we do not know" and "something is
 * wrong" are different claims, and only the second one is worth showing a
 * reader a banner about.
 */
export async function getStorageHealth(): Promise<StorageHealthPayload> {
  if (!hasShell()) return HEALTHY_STORAGE
  return (await invoke<StorageHealthPayload | null>("get_storage_health")) ?? HEALTHY_STORAGE
}

/** What storage health looks like with nothing wrong. */
export const HEALTHY_STORAGE: StorageHealthPayload = { failing: false, message: null }

/** Every repository antiburn knows about on this machine. */
export async function listRepositories(): Promise<RepositoryItemPayload[]> {
  if (!hasShell()) return []
  return invoke<RepositoryItemPayload[]>("list_repositories")
}

/** Include or ignore one repository, returning the refreshed list. */
export async function setRepositoryEnabled(
  key: string,
  enabled: boolean,
): Promise<RepositoryItemPayload[]> {
  if (!hasShell()) return []
  return invoke<RepositoryItemPayload[]>("set_repository_enabled", { key, enabled })
}

/** Re-derive the repository list from what is on disk right now. */
export async function refreshRepositories(): Promise<RepositoryItemPayload[]> {
  if (!hasShell()) return []
  return invoke<RepositoryItemPayload[]>("refresh_repositories")
}

/** The extra directories the reader pointed the scanner at. */
export async function listScanRoots(): Promise<string[]> {
  if (!hasShell()) return []
  return invoke<string[]>("list_scan_roots")
}

/** The directories the engine already searches without being asked. */
export async function defaultScanRoots(): Promise<string[]> {
  if (!hasShell()) return []
  return invoke<string[]>("default_scan_roots")
}

/** Add a directory to scan. */
export async function addScanRoot(path: string): Promise<string[]> {
  if (!hasShell()) return []
  return invoke<string[]>("add_scan_root", { path })
}

/** Stop scanning a directory. */
export async function removeScanRoot(path: string): Promise<string[]> {
  if (!hasShell()) return []
  return invoke<string[]>("remove_scan_root", { path })
}

/**
 * Write one session's derived analysis to `destPath`.
 *
 * The transcript is not copied — the document references it. The caller is
 * still expected to warn first: an export describes real work.
 */
export async function exportSession(
  agent: string,
  sessionId: string,
  destPath: string,
  wslDistro?: string | null,
): Promise<string | null> {
  if (!hasShell()) return null
  return invoke<string>("export_session", {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
    destPath,
  })
}

/**
 * Delete antiburn's own records for one session.
 *
 * Only antiburn's records. The agent's transcript is never touched.
 */
export async function deleteSessionData(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): Promise<boolean> {
  if (!hasShell()) return false
  return invoke<boolean>("delete_session_data", {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
  })
}

/** Reveal a transcript in the platform's file manager. */
export async function revealSource(path: string): Promise<void> {
  if (!hasShell()) return
  await invoke("reveal_source", { path })
}

/* -------------------------------------------------------------------------
 * Nudge commands
 *
 * Registered by the shell from `antiburn_nudge::commands::`. Every one of them
 * is called from the notification window only, and every one is a request the
 * crate may decline — a nudge that has already been dismissed answers none of
 * them, which is why they all resolve to nothing.
 * ---------------------------------------------------------------------- */

/**
 * Report a clicked CTA back to the crate.
 *
 * The crate hands `(kind, actionId, target)` to the shell's `on_action`
 * callback and then dismisses the notification, so this both acts and closes.
 */
export async function nudgeAction(
  kind: NudgeKind,
  actionId: string,
  target?: NudgeActionTarget,
): Promise<void> {
  if (!hasShell()) return
  await invoke("nudge_action", { kind, actionId, target })
}

/** Hide the notification without acting (close button, or the auto-dismiss timeout). */
export async function dismissNudge(): Promise<void> {
  if (!hasShell()) return
  await invoke("nudge_dismiss")
}

/**
 * Reveal the notification at its measured content `height`.
 *
 * The crate keeps the window hidden until this arrives, then sizes, places, and
 * shows it in one step — which is what stops the notification from visibly
 * resizing on screen as it appears.
 */
export async function revealNudge(height: number): Promise<void> {
  if (!hasShell()) return
  await invoke("nudge_reveal", { height })
}

/**
 * Resize the *already visible* notification to a new measured `height` (it
 * expanded or collapsed on hover). On macOS the native frame animates with the
 * system's resize ease, anchored at its top edge; elsewhere it snaps.
 */
export async function resizeNudge(height: number): Promise<void> {
  if (!hasShell()) return
  await invoke("nudge_resize", { height })
}

/**
 * Signal that this window's `nudge:show` listener is attached.
 *
 * The window is prewarmed, so a nudge can be emitted before the webview is
 * listening. The crate retains the pending payload and re-delivers it here,
 * rather than the first nudge after a launch being silently lost.
 */
export async function nudgeReady(): Promise<void> {
  if (!hasShell()) return
  await invoke("nudge_ready")
}

/**
 * Report the notification's hover state (macOS only; a no-op elsewhere).
 *
 * An unprompted nudge never takes key-window status on its own, so hover-driven
 * CSS only receives the mouse-moved events it needs once the cursor has
 * genuinely entered the notification — at which point taking key is safe.
 * Deliberately *not* called for a hover the crate detected by sampling the
 * cursor (`NUDGE_HOVER_EVENT`): that path exists precisely because the window
 * is receiving no mouse events, so there is no `:hover` to feed.
 */
export async function setNudgeHovered(hovered: boolean): Promise<void> {
  if (!hasShell()) return
  await invoke("nudge_set_hovered", { hovered })
}

/* -------------------------------------------------------------------------
 * Events
 * ---------------------------------------------------------------------- */

const noShellUnlisten: UnlistenFn = () => undefined

/** Event names the scan emits. Mirrors `src-tauri/src/scan.rs`. */
export const SCAN_EVENTS = {
  started: "scan:started",
  progress: "scan:progress",
  finished: "scan:finished",
} as const

/**
 * Subscribe to every scan event. The callback receives the status snapshot the
 * event carried; the returned function unsubscribes.
 *
 * Resolves to a no-op unsubscribe without a shell, so a caller's cleanup path
 * is the same in every environment.
 */
export async function onScanEvent(
  handler: (status: ScanStatus, phase: keyof typeof SCAN_EVENTS) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  const unlisteners = await Promise.all(
    (Object.keys(SCAN_EVENTS) as (keyof typeof SCAN_EVENTS)[]).map((phase) =>
      listen<ScanStatus>(SCAN_EVENTS[phase], (event) => handler(event.payload, phase)),
    ),
  )
  return () => unlisteners.forEach((unlisten) => unlisten())
}

/**
 * Event the shell emits after every settings write, carrying the stored
 * (clamped) settings. Mirrors `SETTINGS_CHANGED_EVENT` in
 * `src-tauri/src/commands.rs`. Preferences are written in the settings window
 * but rendered in the popover too; this is what keeps a long-lived popover
 * webview current without a poll.
 */
export const SETTINGS_CHANGED_EVENT = "settings:changed"

/** Subscribe to settings writes from any window. The result unsubscribes. */
export async function onSettingsChanged(
  handler: (settings: AppSettings) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen<AppSettings>(SETTINGS_CHANGED_EVENT, (event) => handler(event.payload))
}

/**
 * Event the shell emits when stored sessions were removed outside a scan
 * (repository opt-out, index clearing). Mirrors `SESSIONS_INVALIDATED_EVENT`
 * in `src-tauri/src/commands.rs`.
 */
export const SESSIONS_INVALIDATED_EVENT = "sessions:invalidated"

/** Subscribe to out-of-band session-index changes. The result unsubscribes. */
export async function onSessionsInvalidated(handler: () => void): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen(SESSIONS_INVALIDATED_EVENT, () => handler())
}

/**
 * Event the shell emits the instant the popover reaches the screen. Mirrors
 * `popover::EVENT_SHOWN` in `src-tauri/src/popover.rs`.
 *
 * Deliberately not folded into `onScanEvent`: the scan it also kicks is
 * gated on discovery being unpaused and can take as long as a full disk
 * walk, neither of which has anything to do with a provider's own stated
 * limits. This is what a view subscribes to when what it wants is "the
 * popover just opened," full stop.
 */
export const POPOVER_SHOWN_EVENT = "popover:shown"

/** Subscribe to the popover reaching the screen. The result unsubscribes. */
export async function onPopoverShown(handler: () => void): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen(POPOVER_SHOWN_EVENT, () => handler())
}

/** Event the shell emits after it refreshes the cached live-usage snapshot. */
export const LIVE_USAGE_CHANGED_EVENT = "live-usage:changed"

/** Subscribe to refreshed provider limit snapshots. The result unsubscribes. */
export async function onLiveUsageChanged(
  handler: (usage: LiveUsageSummaryPayload) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen<LiveUsageSummaryPayload>(LIVE_USAGE_CHANGED_EVENT, (event) =>
    handler(event.payload),
  )
}

/** Event the shell emits when storage health changes. Mirrors `src-tauri/src/storage_health.rs`. */
export const STORAGE_HEALTH_EVENT = "storage:health"

/**
 * Subscribe to storage-health changes. The returned function unsubscribes.
 *
 * Only *changes* are emitted, so a machine whose disk is full produces one
 * event rather than one per scan tick.
 */
export async function onStorageHealth(
  handler: (status: StorageHealthPayload) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen<StorageHealthPayload>(STORAGE_HEALTH_EVENT, (event) => handler(event.payload))
}

/**
 * Event the shell emits to move an *already open* settings window to a pane.
 * Mirrors `src-tauri/src/settings.rs`.
 */
export const SETTINGS_PANE_EVENT = "settings:pane"

/** Subscribe to pane requests aimed at an open settings window. */
export async function onSettingsPaneRequest(
  handler: (pane: string) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen<string>(SETTINGS_PANE_EVENT, (event) => handler(event.payload))
}

/** Event the shell emits after every update check. Mirrors `src-tauri/src/updates.rs`. */
export const UPDATE_EVENT = "update:status"

/**
 * Subscribe to the outcome of every update check the shell runs on its own
 * schedule. The returned function unsubscribes.
 *
 * This is what makes the "check automatically" preference visible: the shell
 * does the checking, and the Updates pane reflects what it found rather than
 * inferring anything from silence.
 */
export async function onUpdateStatus(
  handler: (status: UpdateStatusPayload) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen<UpdateStatusPayload>(UPDATE_EVENT, (event) => handler(event.payload))
}

/**
 * Event the nudge crate emits to the notification window, carrying the
 * {@link Nudge} to draw. Mirrors `NUDGE_SHOW_EVENT` in
 * `src-tauri/crates/nudge/src/lib.rs`.
 */
export const NUDGE_SHOW_EVENT = "nudge:show"

/** Subscribe to incoming nudges. The returned function unsubscribes. */
export async function onNudgeShow(handler: (nudge: Nudge) => void): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen<Nudge>(NUDGE_SHOW_EVENT, (event) => handler(event.payload))
}

/**
 * Event the nudge crate emits carrying whether the cursor is over the
 * notification, sampled natively. Mirrors `NUDGE_HOVER_EVENT` in the crate.
 *
 * macOS only. It backs up — never replaces — the window's own
 * `mouseenter`/`mouseleave`, which stop firing whenever another antiburn window
 * (the settings window, or the popover) holds macOS key-window status, because
 * AppKit routes mouse-moved events there instead.
 */
export const NUDGE_HOVER_EVENT = "nudge:hover"

/** Subscribe to the native hover signal. The returned function unsubscribes. */
export async function onNudgeHover(handler: (hovered: boolean) => void): Promise<UnlistenFn> {
  if (!hasShell()) return noShellUnlisten
  return listen<boolean>(NUDGE_HOVER_EVENT, (event) => handler(event.payload === true))
}

/* -------------------------------------------------------------------------
 * Folder permissions
 * ---------------------------------------------------------------------- */

/** Which protected folders need permission, and which already have it. */
export async function getFolderPermissions(): Promise<FolderPermissions> {
  if (!hasShell()) return { deferred: [], granted: [], supported: false }
  return invoke<FolderPermissions>("get_folder_permissions")
}

/**
 * Ask the operating system for one protected folder.
 *
 * **This is what raises the system's consent dialog**, so call it only from a
 * deliberate action the reader took after being told what it does. The shell
 * focuses the window first and holds the popover open across the call, because
 * a dialog raised by an app with no visible window can otherwise open behind
 * everything else.
 */
export async function requestFolderAccess(dir: string): Promise<FolderAccessOutcome> {
  if (!hasShell()) return { outcome: "denied", elapsedMs: 0 }
  return invoke<FolderAccessOutcome>("request_folder_access", { dir })
}

/** Open the system pane where folder permissions are granted. */
export async function openFolderAccessSettings(): Promise<void> {
  if (!hasShell()) return
  await invoke("open_folder_access_settings")
}

/** Probe outcomes from this run, for a bug report. */
export async function getConsentDiagnostics(): Promise<ProbeRecord[]> {
  if (!hasShell()) return []
  return invoke<ProbeRecord[]>("get_consent_diagnostics")
}

/**
 * Re-check protected folders for grants made outside antiburn.
 *
 * Returns the folder names newly found readable. **This can raise the consent
 * dialog**, so it belongs behind an explicit control, never a background poll.
 */
export async function recheckFolderPermissions(): Promise<string[]> {
  if (!hasShell()) return []
  return invoke<string[]>("recheck_folder_permissions")
}
