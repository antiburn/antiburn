// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Agent display registry.
 *
 * Maps the kebab-case slug the engine emits for an agent onto the three facts
 * the UI needs about it: what to call it, which icon slot it asks for, where
 * its sessions come from, and whether the analysis engine has a dedicated
 * adapter for its transcript format.
 *
 * The registry holds an icon *name*, never artwork. Rendering an icon is the
 * caller's job: components in this app take a `renderAgentIcon` slot and are
 * perfectly happy when nothing fills it.
 */

/** Where a session was discovered from. */
export type AgentSurface = 'cli' | 'ide_desktop' | 'unknown';

/** Everything the presentation layer knows about one agent. */
interface AgentInfo {
  displayName: string;
  /** Icon slot name. A name, not an asset — see the module comment. */
  icon: string;
  /**
   * Slug-only fallback for the session surface. Bi-modal agents (both a CLI
   * and an editor) return `unknown`; only a classified session can say which.
   */
  defaultSurface: AgentSurface;
  /**
   * Whether the analysis engine has a dedicated adapter for this agent's
   * transcript format. Agents on the generic fallback report `false`, and the
   * UI uses that to explain an empty analytics view instead of implying the
   * session was uninteresting.
   */
  supportsAnalytics: boolean;
}

/** Fallback icon slot name for a slug the registry does not know. */
export const GENERIC_AGENT_ICON = 'generic-agent';

const AGENTS: Record<string, AgentInfo> = {
  'claude-code': {
    displayName: 'Claude Code',
    icon: 'claude',
    defaultSurface: 'unknown',
    supportsAnalytics: true,
  },
  codex: {
    displayName: 'Codex',
    icon: 'codex',
    defaultSurface: 'unknown',
    supportsAnalytics: true,
  },
  cursor: {
    displayName: 'Cursor',
    icon: 'cursor',
    defaultSurface: 'unknown',
    supportsAnalytics: true,
  },
  copilot: {
    displayName: 'GitHub Copilot',
    icon: 'copilot',
    defaultSurface: 'unknown',
    supportsAnalytics: false,
  },
  cline: {
    displayName: 'Cline',
    icon: 'cline',
    defaultSurface: 'unknown',
    supportsAnalytics: false,
  },
  opencode: {
    displayName: 'OpenCode',
    icon: 'opencode',
    defaultSurface: 'cli',
    supportsAnalytics: true,
  },
  kiro: {
    displayName: 'Kiro',
    icon: 'kiro',
    defaultSurface: 'ide_desktop',
    supportsAnalytics: false,
  },
  'amp-code': {
    displayName: 'Amp',
    icon: 'amp',
    defaultSurface: 'cli',
    supportsAnalytics: false,
  },
  antigravity: {
    displayName: 'Antigravity',
    icon: 'antigravity',
    defaultSurface: 'unknown',
    supportsAnalytics: true,
  },
  windsurf: {
    displayName: 'Windsurf',
    icon: 'windsurf',
    defaultSurface: 'ide_desktop',
    supportsAnalytics: false,
  },
  pi: {
    displayName: 'Pi',
    icon: 'pi',
    defaultSurface: 'cli',
    supportsAnalytics: false,
  },
};

/** Every agent slug the registry knows, in declaration order. */
export const AGENT_SLUGS: readonly string[] = Object.keys(AGENTS);

/**
 * A user-friendly display name for a slug.
 *
 * An unknown slug is title-cased rather than dropped, so a newly released
 * agent still renders as *something* before the registry catches up.
 */
export function agentDisplayName(slug: string): string {
  return (
    AGENTS[slug]?.displayName ??
    slug.replace(/-/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase())
  );
}

/** The icon slot name for a slug; {@link GENERIC_AGENT_ICON} when unknown. */
export function agentIconName(slug: string): string {
  return AGENTS[slug]?.icon ?? GENERIC_AGENT_ICON;
}

/**
 * Slug-only fallback for the session surface. Use when a session carries no
 * classified surface yet. Bi-modal agents return `unknown` here.
 */
export function defaultAgentSurface(slug: string): AgentSurface {
  return AGENTS[slug]?.defaultSurface ?? 'unknown';
}

/**
 * Whether the analysis engine has a dedicated adapter for this agent. Agents
 * on the generic fallback (and unknown slugs) return false.
 */
export function agentSupportsAnalytics(slug: string): boolean {
  return AGENTS[slug]?.supportsAnalytics ?? false;
}
