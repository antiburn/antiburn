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
 * None of these payloads is fetched. They are produced on this machine by the
 * local engine — see `tests/offline.test.ts`, which enforces that no code in
 * this app opens a network connection.
 */

import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

import type { SettingsPane } from './settingsPanes';
import type {
  ActiveSessionsSummary,
  SessionCostComponents,
  SkillDetail,
} from './types/session';

/* -------------------------------------------------------------------------
 * Payload shapes — mirrors of `src-tauri/src/dto.rs`
 * ---------------------------------------------------------------------- */

/** How the app renders itself. `system` follows the OS appearance. */
export type ThemePreference = 'system' | 'light' | 'dark';

/** Where the notification window appears. `menuBar` is macOS-only. */
export type NudgePlacement = 'menuBar' | 'topRight';

/** When the menu bar shows the free-disk-space number. */
export type DiskSpaceDisplay = 'always' | 'whenLow' | 'never';

/** Which usage-milestone thresholds notify, for one window class. */
export interface Milestones {
  at50: boolean;
  at75: boolean;
  at90: boolean;
}

/** Every persisted preference. Mirrors Rust `AppSettings`. */
export interface AppSettings {
  theme: ThemePreference;
  /** Calendar days of activity the popover list shows. */
  activityWindowDays: number;
  /** False until the first-run flow finishes. */
  onboardingCompleted: boolean;
  /** Recorded; applied by the platform at next launch. */
  launchAtLogin: boolean;
  /** Whether the shell may check the release feed on its own schedule. */
  autoUpdate: boolean;
  /**
   * Whether background discovery and indexing are paused.
   *
   * Paused stops scheduled scans only. An explicit rescan still runs, and
   * everything already indexed stays browsable.
   */
  discoveryPaused: boolean;
  /**
   * The master switch for desktop notifications. Off means nothing is
   * delivered, whatever the per-kind preferences say.
   */
  notificationsEnabled: boolean;
  /** Notify when an automatic update check finds a newer version. */
  notifyUpdateAvailable: boolean;
  /** Notify the first time a scan fails in this run of the app. */
  notifyScanFailure: boolean;
  /** Where the notification window appears. */
  nudgePlacement: NudgePlacement;
  /** Seconds a nudge stays before dismissing itself (3–30). */
  nudgeAutoDismissSecs: number;
  /** Whether a nudge may play the notification chime. */
  notificationSound: boolean;
  /** When the menu bar shows the free-disk-space number. */
  diskSpaceDisplay: DiskSpaceDisplay;
  /** Free space, in GB, below which the disk counts as low (5–2000). */
  diskSpaceThresholdGb: number;
  /** Notify once each time free space drops below the threshold. */
  notifyDiskSpaceLow: boolean;
  /** Notify when sustained spend is unusually fast for this machine. */
  notifyUsageAnomalies: boolean;
  /** Five-hour-window milestones. Only fire while live usage is enabled. */
  milestones5h: Milestones;
  /** Weekly-window milestones. */
  milestonesWeekly: Milestones;
  /**
   * The per-feature online opt-in for live usage limits. Off by default: the
   * app calls no provider endpoint until the reader turns this on.
   */
  liveUsageEnabled: boolean;
}

/** Where the app came from. Mirrors Rust `AppInfo`. */
export interface AppInfo {
  appVersion: string;
  /** CPU architecture this binary was compiled for, e.g. `aarch64`. */
  arch: string;
  pricingCatalogVersion: string;
  schemaVersion: number;
  dataDir: string;
  /** Sessions currently in the local index. */
  indexedSessions: number;
  /** Size of the local database on disk, in bytes. */
  databaseBytes: number;
  /**
   * Whether this build can check for updates at all.
   *
   * Derived shell-side from *real* plugin-registration state plus a configured
   * signing key — never from a compile-time flag — so a build that cannot
   * update never renders a control implying it can.
   */
  updatesSupported: boolean;
}

/** One row of the activity list, before it is shaped for presentation. */
export interface ActivityEntryPayload {
  agent: string;
  sessionId: string;
  repo: string;
  timestamp: string;
  isActive: boolean;
  surface: string;
  wslDistro: string | null;
  title: string | null;
  hasForkParent: boolean;
  forkChildCount: number;
  subagentCount: number;
  cost: SessionCostComponents | null;
  models: string[];
  activeSecs: number | null;
  durationSecs: number | null;
}

/** Identity of one local session, as the analytics view carries it. */
export interface SessionIdentityPayload {
  agent: string;
  sessionId: string;
  wslDistro: string | null;
}

/** One end of a local fork relation. */
export interface SessionRelationPayload {
  identity: SessionIdentityPayload;
  title: string | null;
  available: boolean;
}

/** Direct fork relations for one session. */
export interface SessionRelationsPayload {
  title: string | null;
  parent: SessionRelationPayload | null;
  children: SessionRelationPayload[];
}

/** One sub-agent an orchestrator launched. */
export interface SubagentMemberPayload {
  agent: string;
  subagentId: string;
  label: string;
  patternScore: number;
  spawnProgress: number | null;
}

/** The sub-agent picture for one session. */
export interface OrchestrationPayload {
  orchestrating: boolean;
  orchestratorAgent: string;
  orchestratorSessionId: string;
  subagentCount: number;
  members: SubagentMemberPayload[];
}

/** Everything the session-analytics surface renders for one session. */
export interface SessionAnalyticsPayload {
  summary: ActiveSessionsSummary | null;
  supportsAnalytics: boolean;
  title: string | null;
  wslDistro: string | null;
  isActive: boolean;
  cost: SessionCostComponents | null;
  models: string[];
  skills: SkillDetail[];
  orchestration: OrchestrationPayload | null;
  relations: SessionRelationsPayload | null;
  /** The provider's own transcript, for the reveal action. */
  sourcePath: string | null;
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
export type ProviderUsageState = 'live' | 'estimated' | 'observed' | 'detected' | 'unknown';

/** Whether a provider's newest local evidence still describes now. */
export type ProviderUsageStaleness = 'fresh' | 'stale' | 'unknown';

/**
 * One provider's totals over one window.
 *
 * There is no percentage, allowance, remaining balance, or reset here, and
 * that is deliberate — see `ProviderUsageState`.
 */
export interface ProviderUsageWindowPayload {
  /** Fresh prompt tokens plus prompt-cache writes. */
  tokensIn: number;
  tokensOut: number;
  /** Prompt-cache reads, billed at their own rate. */
  cacheRead: number;
  /**
   * On-device estimate for the models that could be priced, or null when none
   * could. A `observed` state means this is a floor rather than a total.
   */
  estimatedUsd: number | null;
  /** Sessions that contributed. One session can count under two providers. */
  sessionCount: number;
}

/** The three windows every provider is summarized over. */
export interface ProviderUsageWindowsPayload {
  today: ProviderUsageWindowPayload;
  week: ProviderUsageWindowPayload;
  month: ProviderUsageWindowPayload;
}

/** Everything the usage surfaces show about one provider. */
export interface ProviderUsagePayload {
  /** Canonical id (`anthropic`, `openai`, `unknown`, …) — a key, not copy. */
  provider: string;
  displayName: string;
  state: ProviderUsageState;
  staleness: ProviderUsageStaleness;
  windows: ProviderUsageWindowsPayload;
  lastActivityAt: string | null;
}

/** Local provider usage as one snapshot. Mirrors Rust `ProviderUsageSummary`. */
export interface ProviderUsageSummaryPayload {
  providers: ProviderUsagePayload[];
  generatedAt: string;
  /** Days of session history the app keeps — shorter than a calendar month. */
  retentionDays: number;
  /** Earliest activity these windows can include. */
  coverageSince: string;
}

/** One repository row. Mirrors Rust `RepositoryItem`. */
export interface RepositoryItemPayload {
  key: string;
  repoName: string;
  fullName: string;
  status: string;
  repoRoot: string | null;
  suspectedPath: string | null;
  worktreeCount: number;
  sessionCount: number;
  wslDistro: string | null;
  enabled: boolean;
}

/** What one agent's last pass saw. */
export interface AgentScanState {
  agent: string;
  lastCompletedAt: string | null;
  sessionsSeen: number;
}

/** What a scan is doing, or last did. Mirrors Rust `ScanStatus`. */
export interface ScanStatus {
  running: boolean;
  completedAgents: number;
  totalAgents: number;
  sessions: number;
  finishedAt: string | null;
  /** True when the last pass stopped because it was asked to, not because it failed. */
  cancelled: boolean;
  error: string | null;
  agents: AgentScanState[];
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
  failing: boolean;
  /** What failed, in the store's own words. Present only while `failing`. */
  message: string | null;
}

/** The outcome of one update check. Mirrors Rust `UpdateStatus`. */
export interface UpdateStatusPayload {
  kind: 'current' | 'available' | 'failed' | 'unsupported';
  version: string | null;
  message: string | null;
  checkedAt: string;
  /** True when the shell's own schedule ran the check rather than a reader. */
  automatic: boolean;
}

/* -------------------------------------------------------------------------
 * Presence
 * ---------------------------------------------------------------------- */

/** True when the bundle is running inside the antiburn shell. */
export function hasShell(): boolean {
  return isTauri();
}

/** What settings look like before anything has been stored, or without a shell. */
export const DEFAULT_SETTINGS: AppSettings = {
  theme: 'system',
  activityWindowDays: 7,
  onboardingCompleted: false,
  launchAtLogin: false,
  autoUpdate: true,
  discoveryPaused: false,
  notificationsEnabled: true,
  notifyUpdateAvailable: true,
  notifyScanFailure: true,
  nudgePlacement: 'menuBar',
  nudgeAutoDismissSecs: 10,
  notificationSound: true,
  diskSpaceDisplay: 'whenLow',
  diskSpaceThresholdGb: 50,
  notifyDiskSpaceLow: true,
  notifyUsageAnomalies: true,
  milestones5h: { at50: true, at75: true, at90: true },
  milestonesWeekly: { at50: true, at75: true, at90: true },
  liveUsageEnabled: false,
};

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
  if (!hasShell()) return null;
  return invoke<string>('engine_catalog_version');
}

/**
 * Opens (or refocuses) the standalone settings window.
 *
 * `pane` is a request for a section — an attention banner uses it to land a
 * reader on the pane that can fix what they were told about. Omitting it leaves
 * the window wherever it was.
 */
export async function openSettingsWindow(pane?: SettingsPane): Promise<void> {
  if (!hasShell()) return;
  await invoke('open_settings_window', { pane: pane ?? null });
}

/**
 * The pane a caller asked for, taken once, as the settings window mounts.
 *
 * Taken rather than read: a window opened later must not jump to a pane
 * somebody asked for an hour ago.
 */
export async function takeSettingsPane(): Promise<string | null> {
  if (!hasShell()) return null;
  return invoke<string | null>('take_settings_pane');
}

/**
 * Asks the shell to close the settings window (⌘W).
 *
 * A close *request*, not a destroy: the shell intercepts `CloseRequested` for
 * every window and hides instead (see `src-tauri/src/lib.rs`), so this takes
 * exactly the same path as the title-bar close button. Needs
 * `core:window:allow-close` in `capabilities/default.json` — the ACL's
 * `core:window:default` set is read-only.
 */
export async function closeSettingsWindow(): Promise<void> {
  if (!hasShell()) return;
  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  await getCurrentWindow().close();
}

/**
 * Quit antiburn.
 *
 * Routed through the shell rather than closing windows, because a menu-bar app
 * outlives its windows: only `exit(0)` distinguishes a deliberate quit from the
 * window closes the shell suppresses.
 */
export async function quitApp(): Promise<void> {
  if (!hasShell()) return;
  await invoke('quit_app');
}

/**
 * Dismisses the tray popover.
 *
 * A shell command rather than `getCurrentWindow().hide()`, because the shell
 * gates its background scan on the popover being visible and a window hidden
 * behind its back would leave that gate open.
 */
export async function hidePopover(): Promise<void> {
  if (!hasShell()) return;
  await invoke('hide_popover');
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
  if (!hasShell()) return action();
  await invoke('begin_popover_hold');
  try {
    return await action();
  } finally {
    await invoke('end_popover_hold').catch(() => undefined);
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
  if (!hasShell()) return;
  await invoke('set_popover_height', { height, animate });
}

/** Where the app came from and what it is running against. */
export async function appInfo(): Promise<AppInfo | null> {
  if (!hasShell()) return null;
  return invoke<AppInfo>('app_info');
}

/** Every persisted preference. */
export async function getSettings(): Promise<AppSettings> {
  if (!hasShell()) return DEFAULT_SETTINGS;
  return invoke<AppSettings>('get_settings');
}

/** Replace every preference, returning what was actually stored. */
export async function setSettings(settings: AppSettings): Promise<AppSettings> {
  if (!hasShell()) return settings;
  return invoke<AppSettings>('set_settings', { settings });
}

/** The sessions to show in the popover, newest first. */
export async function listRecentSessions(windowDays?: number): Promise<ActivityEntryPayload[]> {
  if (!hasShell()) return [];
  return invoke<ActivityEntryPayload[]>('list_recent_sessions', {
    windowDays: windowDays ?? null,
  });
}

/** One session's analysis, sub-agent roster, and fork relations. */
export async function getSessionAnalytics(
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): Promise<SessionAnalyticsPayload | null> {
  if (!hasShell()) return null;
  return invoke<SessionAnalyticsPayload>('get_session_analytics', {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
  });
}

/** One sub-agent's own analysis. */
export async function getSubagentAnalytics(
  agent: string,
  parentSessionId: string,
  subagentId: string,
  wslDistro?: string | null,
): Promise<SessionAnalyticsPayload | null> {
  if (!hasShell()) return null;
  return invoke<SessionAnalyticsPayload>('get_subagent_analytics', {
    agent,
    parentSessionId,
    subagentId,
    wslDistro: wslDistro ?? null,
  });
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
  if (!hasShell()) return EMPTY_PROVIDER_USAGE;
  return invoke<ProviderUsageSummaryPayload>('get_provider_usage', {
    utcOffsetMinutes: -new Date().getTimezoneOffset(),
  });
}

/** What provider usage looks like with nothing to report. */
export const EMPTY_PROVIDER_USAGE: ProviderUsageSummaryPayload = {
  providers: [],
  generatedAt: '',
  retentionDays: 0,
  coverageSince: '',
};

/** Run a scan now, unless one is already in flight. */
export async function scanNow(): Promise<ScanStatus | null> {
  if (!hasShell()) return null;
  return invoke<ScanStatus>('scan_now');
}

/** What the current or last scan is doing. */
export async function getScanStatus(): Promise<ScanStatus | null> {
  if (!hasShell()) return null;
  return invoke<ScanStatus>('get_scan_status');
}

/**
 * Ask the scan in flight to stop at its next phase boundary.
 *
 * Everything it already found stays indexed: a cancelled pass is a shorter
 * pass, not an undone one.
 */
export async function cancelScan(): Promise<ScanStatus | null> {
  if (!hasShell()) return null;
  return invoke<ScanStatus>('cancel_scan');
}

/**
 * Forget antiburn's entire derived index, returning how many sessions went.
 *
 * antiburn's own records only. The agents' transcripts are never touched, and a
 * later scan finds all of this again.
 */
export async function clearLocalIndex(): Promise<number> {
  if (!hasShell()) return 0;
  return invoke<number>('clear_local_index');
}

/**
 * Whether the local database is still accepting writes.
 *
 * Falls back to healthy rather than to null: "we do not know" and "something is
 * wrong" are different claims, and only the second one is worth showing a
 * reader a banner about.
 */
export async function getStorageHealth(): Promise<StorageHealthPayload> {
  if (!hasShell()) return HEALTHY_STORAGE;
  return (await invoke<StorageHealthPayload | null>('get_storage_health')) ?? HEALTHY_STORAGE;
}

/** What storage health looks like with nothing wrong. */
export const HEALTHY_STORAGE: StorageHealthPayload = { failing: false, message: null };

/** Every repository antiburn knows about on this machine. */
export async function listRepositories(): Promise<RepositoryItemPayload[]> {
  if (!hasShell()) return [];
  return invoke<RepositoryItemPayload[]>('list_repositories');
}

/** Include or ignore one repository, returning the refreshed list. */
export async function setRepositoryEnabled(
  key: string,
  enabled: boolean,
): Promise<RepositoryItemPayload[]> {
  if (!hasShell()) return [];
  return invoke<RepositoryItemPayload[]>('set_repository_enabled', { key, enabled });
}

/** Re-derive the repository list from what is on disk right now. */
export async function refreshRepositories(): Promise<RepositoryItemPayload[]> {
  if (!hasShell()) return [];
  return invoke<RepositoryItemPayload[]>('refresh_repositories');
}

/** The extra directories the reader pointed the scanner at. */
export async function listScanRoots(): Promise<string[]> {
  if (!hasShell()) return [];
  return invoke<string[]>('list_scan_roots');
}

/** The directories the engine already searches without being asked. */
export async function defaultScanRoots(): Promise<string[]> {
  if (!hasShell()) return [];
  return invoke<string[]>('default_scan_roots');
}

/** Add a directory to scan. */
export async function addScanRoot(path: string): Promise<string[]> {
  if (!hasShell()) return [];
  return invoke<string[]>('add_scan_root', { path });
}

/** Stop scanning a directory. */
export async function removeScanRoot(path: string): Promise<string[]> {
  if (!hasShell()) return [];
  return invoke<string[]>('remove_scan_root', { path });
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
  if (!hasShell()) return null;
  return invoke<string>('export_session', {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
    destPath,
  });
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
  if (!hasShell()) return false;
  return invoke<boolean>('delete_session_data', {
    agent,
    sessionId,
    wslDistro: wslDistro ?? null,
  });
}

/** Reveal a transcript in the platform's file manager. */
export async function revealSource(path: string): Promise<void> {
  if (!hasShell()) return;
  await invoke('reveal_source', { path });
}

/* -------------------------------------------------------------------------
 * Events
 * ---------------------------------------------------------------------- */

/** Event names the scan emits. Mirrors `src-tauri/src/scan.rs`. */
export const SCAN_EVENTS = {
  started: 'scan:started',
  progress: 'scan:progress',
  finished: 'scan:finished',
} as const;

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
  if (!hasShell()) return () => {};
  const unlisteners = await Promise.all(
    (Object.keys(SCAN_EVENTS) as (keyof typeof SCAN_EVENTS)[]).map((phase) =>
      listen<ScanStatus>(SCAN_EVENTS[phase], (event) => handler(event.payload, phase)),
    ),
  );
  return () => unlisteners.forEach((unlisten) => unlisten());
}

/**
 * Event the shell emits after every settings write, carrying the stored
 * (clamped) settings. Mirrors `SETTINGS_CHANGED_EVENT` in
 * `src-tauri/src/commands.rs`. Preferences are written in the settings window
 * but rendered in the popover too; this is what keeps a long-lived popover
 * webview current without a poll.
 */
export const SETTINGS_CHANGED_EVENT = 'settings:changed';

/** Subscribe to settings writes from any window. The result unsubscribes. */
export async function onSettingsChanged(
  handler: (settings: AppSettings) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return () => {};
  return listen<AppSettings>(SETTINGS_CHANGED_EVENT, (event) => handler(event.payload));
}

/**
 * Event the shell emits when stored sessions were removed outside a scan
 * (repository opt-out, index clearing). Mirrors `SESSIONS_INVALIDATED_EVENT`
 * in `src-tauri/src/commands.rs`.
 */
export const SESSIONS_INVALIDATED_EVENT = 'sessions:invalidated';

/** Subscribe to out-of-band session-index changes. The result unsubscribes. */
export async function onSessionsInvalidated(handler: () => void): Promise<UnlistenFn> {
  if (!hasShell()) return () => {};
  return listen(SESSIONS_INVALIDATED_EVENT, () => handler());
}

/** Event the shell emits when storage health changes. Mirrors `src-tauri/src/storage_health.rs`. */
export const STORAGE_HEALTH_EVENT = 'storage:health';

/**
 * Subscribe to storage-health changes. The returned function unsubscribes.
 *
 * Only *changes* are emitted, so a machine whose disk is full produces one
 * event rather than one per scan tick.
 */
export async function onStorageHealth(
  handler: (status: StorageHealthPayload) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return () => {};
  return listen<StorageHealthPayload>(STORAGE_HEALTH_EVENT, (event) => handler(event.payload));
}

/**
 * Event the shell emits to move an *already open* settings window to a pane.
 * Mirrors `src-tauri/src/settings.rs`.
 */
export const SETTINGS_PANE_EVENT = 'settings:pane';

/** Subscribe to pane requests aimed at an open settings window. */
export async function onSettingsPaneRequest(
  handler: (pane: string) => void,
): Promise<UnlistenFn> {
  if (!hasShell()) return () => {};
  return listen<string>(SETTINGS_PANE_EVENT, (event) => handler(event.payload));
}

/** Event the shell emits after every update check. Mirrors `src-tauri/src/updates.rs`. */
export const UPDATE_EVENT = 'update:status';

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
  if (!hasShell()) return () => {};
  return listen<UpdateStatusPayload>(UPDATE_EVENT, (event) => handler(event.payload));
}
