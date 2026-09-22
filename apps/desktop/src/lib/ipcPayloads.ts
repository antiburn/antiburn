/**
 * The payload shapes the shell and the views agree on.
 *
 * These are mirrors of the Rust types. They carry no behavior, so they sit
 * apart from the command wrappers that send and receive them.
 */

import type { SessionIdentityPayload } from "./sessionIpc"

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
  /** Weeks start on Monday. */
  workingWeek: "five" | "six" | "seven"
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
interface AgentScanState {
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
