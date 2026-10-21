/**
 * The offer to install the antiburn Claude Code plugin: who sees it, and the
 * words and commands it carries. Pure. The shell facts come from
 * `claudePluginStore.ts`, and the cards that show them live in
 * `components/claudePlugin/`.
 */

import type { LiveUsageMeterPayload } from "./providerUsageIpc"

/** Mirrors Rust `ClaudePluginState`. */
type ClaudePluginState = "installed" | "absent" | "unknown"

/** Mirrors Rust `ClaudePluginStatus`. */
export interface ClaudePluginStatusPayload {
  state: ClaudePluginState
  version: string | null
  enabled: boolean
}

export type ClaudePluginOfferState = "hidden" | "offer" | "installed"

/** Typed into any Claude Code session: Terminal, or the Desktop Code tab. */
export const CLAUDE_PLUGIN_SESSION_COMMAND =
  "/plugin install antiburn --marketplace antiburn/antiburn"

/** The same install from a shell, as `plugins/README.md` gives it. */
export const CLAUDE_PLUGIN_SHELL_COMMANDS = [
  "claude plugin marketplace add https://github.com/antiburn/antiburn.git",
  "claude plugin install antiburn@antiburn --scope user",
] as const

export const CLAUDE_PLUGIN_TITLE = "Claude Code Plugin"
export const CLAUDE_PLUGIN_TAGLINE =
  "Usage, resets and pace above your prompt. Free plugin, no account."
export const CLAUDE_PLUGIN_SUMMARY =
  "Five-hour and weekly usage, reset times, and pace, above the prompt in every Claude Code session. No model calls, no antiburn account."

export interface ClaudePluginOfferInput {
  /** `live.meters`. The Anthropic meter says whether Claude Code is here. */
  meters: readonly LiveUsageMeterPayload[]
  /** Claude Code sessions found in local logs. */
  claudeSessions: number
  status: ClaudePluginStatusPayload | null
  dismissed: boolean
}

/**
 * Whether this machine has Claude Code or Claude Desktop. The plugin works in
 * both, so a Desktop-only reader is a reader too.
 */
export function claudeCodePresent(
  meters: readonly LiveUsageMeterPayload[],
  claudeSessions: number,
): boolean {
  if (claudeSessions > 0) return true
  return meters.some(
    (meter) =>
      meter.provider === "anthropic" &&
      (meter.detection === "signedIn" ||
        meter.detection === "installedNotSignedIn" ||
        Boolean(meter.desktopAppLabel)),
  )
}

/**
 * `installed` outranks everything and is never dismissible. `offer` needs
 * Claude Code here, a registry that says the plugin is absent, and no
 * dismissal. A status we could not read, or have not read yet, hides the
 * offer: no nagging a reader we cannot see.
 */
export function claudePluginOffer(input: ClaudePluginOfferInput): ClaudePluginOfferState {
  if (input.status?.state === "installed") return "installed"
  if (input.status?.state !== "absent") return "hidden"
  if (input.dismissed) return "hidden"
  return claudeCodePresent(input.meters, input.claudeSessions) ? "offer" : "hidden"
}

const DISMISSAL_KEY = "antiburn.claudePluginOffer.v1"

/** True once the reader has dismissed the sidebar card. The dismissal is for good. */
export function readClaudePluginOfferDismissed(): boolean {
  try {
    const raw = localStorage.getItem(DISMISSAL_KEY)
    if (!raw) return false
    const parsed: unknown = JSON.parse(raw)
    return (
      parsed != null &&
      typeof parsed === "object" &&
      typeof (parsed as { dismissedAt?: unknown }).dismissedAt === "string"
    )
  } catch {
    return false
  }
}

export function writeClaudePluginOfferDismissed(now = new Date()): void {
  try {
    localStorage.setItem(DISMISSAL_KEY, JSON.stringify({ dismissedAt: now.toISOString() }))
  } catch {
    // The card still hides for this run when preference storage is unavailable.
  }
}
