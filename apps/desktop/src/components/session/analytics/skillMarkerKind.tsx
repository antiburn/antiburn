// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Zap } from "lucide-react"

import type { SkillDetail } from "../../../lib/types/session"
import type { MarkerKind } from "./HypnogramMarkers"
import { SkillMarkerTooltip } from "./SkillMarkerTooltip"

export interface SkillMarkerKindOptions {
  /** Reveal a skill's `SKILL.md`; omitted hides the affordance. */
  onRevealPath?: (path: string) => void
  /** Accessible name for the reveal control. */
  revealLabel?: string
}

/**
 * The skill-invocation marker species: gold dots on the hypnogram at each
 * `Skill` tool call, with a hover card describing what ran.
 *
 * A factory rather than a constant, because the reveal capability has to be
 * injected — the presentation layer cannot reach the filesystem itself.
 * {@link skillMarkerKind} is the no-capability default.
 */
export function createSkillMarkerKind(
  options: SkillMarkerKindOptions = {},
): MarkerKind<SkillDetail> {
  return {
    // Match the spawn card's width so the two species read as one family.
    maxWidth: 270,
    dotFill: (dark) =>
      dark
        ? "color-mix(in srgb, var(--color-system-gold) 55%, transparent)"
        : "color-mix(in srgb, var(--color-system-gold) 20%, transparent)",
    tetherColor: "var(--color-system-gold-text)",
    textColor: "var(--color-system-gold-text)",
    // Dark mode needs a hairline same-color stroke, paired with the heavier
    // weight the overlay applies, for the digit to stay legible on the tint.
    textStroke: (dark) => (dark ? "0.5px var(--color-system-gold-text)" : undefined),
    dotContent: (members) =>
      members.length > 1 ? members.length : <Zap size={14} aria-hidden="true" />,
    ariaLabel: (members) =>
      members.length > 1
        ? `${members.length} skills used`
        : `Skill: ${members[0]?.data.name ?? "unknown"}`,
    renderTooltip: (members) => (
      <SkillMarkerTooltip
        members={members}
        {...(options.onRevealPath ? { onRevealPath: options.onRevealPath } : {})}
        {...(options.revealLabel ? { revealLabel: options.revealLabel } : {})}
      />
    ),
  }
}

/** The skill marker species with no host capabilities wired in. */
export const skillMarkerKind: MarkerKind<SkillDetail> = createSkillMarkerKind()
