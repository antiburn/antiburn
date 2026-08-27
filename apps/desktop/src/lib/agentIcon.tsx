/**
 * Filling the presentation layer's agent-icon slot.
 *
 * The presentation components take a `renderAgentIcon` slot and never ship
 * artwork; the registry in `lib/presentation/agents` resolves a slug to an
 * icon *name*, not an asset. This module answers that slot with three tiers:
 *
 * 1. **A brand mark** with recorded provenance — CC0-licensed path data,
 *    verified against each icon's recorded source so a name collision cannot
 *    smuggle in the wrong brand. `simple-icons` supplies all but one (its
 *    `AMP` is Google's web framework, not the Amp agent, so Amp deliberately
 *    has no entry here, and its only OpenAI-adjacent icon is the discontinued
 *    `OpenAI Gym`); OpenAI's mark comes from `lib/brandMarks`, which carries
 *    its own provenance and a test pinning it to its source.
 * 2. **A letter tile** for known agents without a usable mark (Kiro,
 *    Antigravity, Amp): the display name's initial in a small rounded tile,
 *    the same treatment the provider glyphs use.
 * 3. **A surface glyph** for `generic-agent` — terminal for CLI, editor panel
 *    for IDE, neutral mark otherwise — since an unknown agent has no name to
 *    draw.
 *
 * Colour follows the same evidence rule as the artwork. Most marks render in
 * the theme's brand-mark ink (`--color-agent-mark`) so the set reads as one
 * set in both themes; a mark whose identity *is* its colour opts into
 * {@link BRAND_COLORED} and renders in the hex the package records for it,
 * never a hex chosen here. Use is nominative — identifying which vendor's
 * agent produced a session.
 */

import { Bot, PanelsTopLeft, SquareTerminal, type LucideIcon } from "lucide-react"
import type { ReactNode } from "react"
import {
  siClaude,
  siCline,
  siCursor,
  siGithubcopilot,
  siOpencode,
  siPi,
  siWindsurf,
} from "simple-icons"

import { fromSimpleIcons, OPENAI_MARK, type BrandMark } from "./brandMarks"
import { agentDisplayName, agentIconName, type AgentSurface } from "./presentation/agents"

/**
 * Registry icon name → brand mark. Keyed by the registry's icon names (not
 * slugs) so aliases like `claude-code` → `claude` resolve once, upstream.
 */
export const BRAND_MARKS: Record<string, BrandMark> = {
  claude: fromSimpleIcons(siClaude),
  cursor: fromSimpleIcons(siCursor),
  copilot: fromSimpleIcons(siGithubcopilot),
  cline: fromSimpleIcons(siCline),
  opencode: fromSimpleIcons(siOpencode),
  windsurf: fromSimpleIcons(siWindsurf),
  pi: fromSimpleIcons(siPi),
  codex: OPENAI_MARK,
}

/**
 * Marks that render in their brand's own hex rather than the theme ink.
 *
 * The bar is that the colour carries the identity — Claude's clay orange is
 * as much the mark as its shape, and flattening it to ink loses the brand
 * rather than unifying it. Everything else stays ink, because a row of eleven
 * competing brand colours reads as noise and several vendor hexes are
 * near-black or near-white, so they vanish in one theme or the other.
 *
 * The value is always the mark's recorded `hex`, never a hex written here, so
 * a member without one is a bug the tests catch rather than a silent fallback.
 */
export const BRAND_COLORED: ReadonlySet<string> = new Set(["claude"])

/** The glyph for a surface. Unknown surfaces get the neutral agent mark. */
function glyphFor(surface: AgentSurface | undefined): LucideIcon {
  if (surface === "cli") return SquareTerminal
  if (surface === "ide_desktop") return PanelsTopLeft
  return Bot
}

function Mark({ name, mark, size }: { name: string; mark: BrandMark; size: number }) {
  return (
    <svg
      viewBox={mark.viewBox}
      width={size}
      height={size}
      fill="currentColor"
      // Brand-coloured marks override the inherited ink; the rest inherit it.
      style={BRAND_COLORED.has(name) ? { color: `#${mark.hex}` } : undefined}
      aria-hidden="true"
    >
      <path d={mark.path} />
    </svg>
  )
}

/** The display name's initial, in the provider-glyph tile treatment. */
function LetterTile({ name, size }: { name: string; size: number }) {
  return (
    <span
      aria-hidden="true"
      className="inline-flex items-center justify-center rounded-[25%] border border-separator bg-surface-secondary font-medium leading-none"
      style={{ width: size, height: size, fontSize: Math.max(8, Math.round(size * 0.52)) }}
    >
      {name.charAt(0).toUpperCase()}
    </span>
  )
}

/**
 * Render one agent's icon.
 *
 * Matches the `SessionAgentIconRenderer` / `AgentIconRenderer` signatures the
 * presentation components declare, so it can be passed straight into either.
 */
export function renderAgentIcon(slug: string, size: number, surface?: AgentSurface): ReactNode {
  const iconName = agentIconName(slug)
  const mark = BRAND_MARKS[iconName]
  const Glyph = glyphFor(surface)
  return (
    <span
      // The seam: the registry's icon name for this slug, so artwork can be
      // swapped later without every call site learning about it.
      data-agent-icon={iconName}
      className="inline-flex items-center justify-center text-agent-mark"
      // Decorative on its own — the row's title carries the session's name —
      // but the agent is not otherwise stated, so it gets a label.
      role="img"
      aria-label={agentDisplayName(slug)}
    >
      {mark ? (
        <Mark name={iconName} mark={mark} size={size} />
      ) : iconName === "generic-agent" ? (
        <Glyph size={size} strokeWidth={1.75} aria-hidden="true" />
      ) : (
        <LetterTile name={agentDisplayName(slug)} size={size} />
      )}
    </span>
  )
}
