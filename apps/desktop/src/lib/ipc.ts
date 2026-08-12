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

/** Every persisted preference. Mirrors Rust `AppSettings`. */
export interface AppSettings {
  theme: ThemePreference;
  /** Calendar days of activity the popover list shows. */
  activityWindowDays: number;
  /** False until the first-run flow finishes. */
  onboardingCompleted: boolean;
  /** Recorded; applied by the platform at next launch. */
  launchAtLogin: boolean;
  autoUpdate: boolean;
}

/** Where the app came from. Mirrors Rust `AppInfo`. */
export interface AppInfo {
  appVersion: string;
  pricingCatalogVersion: string;
  schemaVersion: number;
  dataDir: string;
  /** False in development builds, where the updater plugin is not installed. */
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
  error: string | null;
  agents: AgentScanState[];
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

/** Opens (or refocuses) the standalone settings window. */
export async function openSettingsWindow(): Promise<void> {
  if (!hasShell()) return;
  await invoke('open_settings_window');
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
