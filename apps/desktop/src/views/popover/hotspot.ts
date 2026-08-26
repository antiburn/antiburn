// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * The contract between the local insights report and the popover's hotspot
 * block.
 *
 * A hotspot is the single most common Hygiene & Efficiency finding across the
 * assessed 30-day cohort. The report ranks its findings and hands over one
 * winner; this module says what that winner looks like on the way in.
 *
 * The report does not exist yet (`docs/plans/hotspot-popover-block.md`), so
 * nothing produces a `HotspotFinding` today. The types are here first because
 * the presentation is what is under design review, and freezing the shape is
 * what lets the two halves land in either order.
 */

/**
 * The nine canonical Hygiene & Efficiency categories.
 *
 * These are fixed identifiers from
 * `docs/plans/local-insights-architecture.md`, not generated strings. The order
 * is the tie-break order for two findings with equal session counts and equal
 * token estimates.
 */
export const HOTSPOT_CATEGORIES = [
  "sessionsOverDepth",
  "modelOverthinking",
  "overpoweredSubagents",
  "unusedMcpServers",
  "unusedBuiltInTools",
  "unusedSkills",
  "oldModelUsage",
  "overuseOfFastMode",
  "cacheChurn",
] as const

export type HotspotCategory = (typeof HOTSPOT_CATEGORIES)[number]

/**
 * One counted row in the opened evidence table.
 *
 * `HotspotFinding` is the only name that leaves this module, so this stays
 * local until a second reader of the report needs it.
 */
interface HotspotEvidenceRow {
  /** What was counted. Fixed copy, never a repository or a path. */
  label: string
  /** The count, already formatted for display. */
  value: string
}

/**
 * The winning finding, ready to render.
 *
 * Every number arrives preformatted. The report owns locale and rounding
 * because it also owns the pricing catalog and the token arithmetic, and a
 * second rounding rule in the view would let the same figure print two ways.
 */
export interface HotspotFinding {
  category: HotspotCategory
  /** Sessions in the assessed cohort that carry this finding. */
  sessions: number
  /**
   * What acting on it is worth, already prefixed with `≈` because it is an
   * estimate. Null when the pricing catalog cannot value the category.
   */
  saving: string | null
  /**
   * The one pasteable line: a CLI command, a settings key, or a model id.
   * Never prose, because the block gives it one line and copies it verbatim.
   */
  fix: string
  /**
   * The counters the detector already recorded, in the order to show them:
   * the size of the problem first, then the proof it is real.
   *
   * The block puts no ceiling on how many. A long list scrolls inside the
   * opened detail rather than pushing the session list out of the window.
   */
  evidence: readonly HotspotEvidenceRow[]
}

/**
 * What each category's display name is, and the one sentence that says why it
 * costs anything.
 *
 * This copy lives in the view rather than in the report payload. It names no
 * repository, no path and no number, so sending it across the IPC boundary
 * would move a constant between processes for nothing — and the privacy rule
 * in `local-insights-architecture.md` is easiest to hold when the boundary
 * carries only counts and slots.
 */
export const HOTSPOT_COPY: Record<HotspotCategory, { name: string; mechanism: string }> = {
  sessionsOverDepth: {
    name: "Sessions over depth",
    mechanism:
      "Past this depth every turn resends the whole conversation, so the cache read grows with each message.",
  },
  modelOverthinking: {
    name: "Model overthinking",
    mechanism:
      "These sessions ran a higher reasoning tier than the work needed, and the extra thinking is billed.",
  },
  overpoweredSubagents: {
    name: "Overpowered subagents",
    mechanism:
      "Subagents inherit the main model unless a cheaper one is set. Most of these runs were searches and file reads.",
  },
  unusedMcpServers: {
    name: "Unused MCP servers",
    mechanism:
      "Their tool definitions load into every session they are configured for, even when nothing calls them.",
  },
  unusedBuiltInTools: {
    name: "Unused built-in tools",
    mechanism:
      "Every enabled tool adds its definition to the prompt, whether or not the session ever calls it.",
  },
  unusedSkills: {
    name: "Unused skills",
    mechanism:
      "A loaded skill puts its description in the prompt for every session, and these were never invoked.",
  },
  oldModelUsage: {
    name: "Old model usage",
    mechanism: "These sessions ran a model that a newer one now beats on both price and speed.",
  },
  overuseOfFastMode: {
    name: "Overuse of fast mode",
    mechanism:
      "Fast mode is priced above the standard tier, and these sessions did not need the speed.",
  },
  cacheChurn: {
    name: "Cache churn",
    mechanism:
      "Switching models or leaving a session idle drops the cached prefix, so the next turn pays to rebuild it.",
  },
}

/**
 * The count as the claim line prints it: `34×`, with a hair space so the
 * multiplication sign does not crowd the digits.
 */
export function hotspotCountLabel(sessions: number): string {
  return `${sessions.toLocaleString()}\u200a×`
}
