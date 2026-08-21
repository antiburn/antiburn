// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Zap } from "lucide-react"

import type { SkillDetail } from "../../../lib/types/session"
import type { MarkerKind } from "./HypnogramMarkers"
import { SkillMarkerTooltip } from "./SkillMarkerTooltip"

/**
 * Show gold dots for `Skill` tool calls on the hypnogram.
 * The hover card describes the skills that ran.
 */
export const skillMarkerKind: MarkerKind<SkillDetail> = {
  // Match the spawn card width so both cards read as one family.
  maxWidth: 270,
  dotFill: (dark) =>
    dark
      ? "color-mix(in srgb, var(--color-system-gold) 55%, transparent)"
      : "color-mix(in srgb, var(--color-system-gold) 20%, transparent)",
  tetherColor: "var(--color-system-gold-text)",
  textColor: "var(--color-system-gold-text)",
  // The stroke keeps the number clear on the dark tint.
  textStroke: (dark) => (dark ? "0.5px var(--color-system-gold-text)" : undefined),
  dotContent: (members) =>
    members.length > 1 ? members.length : <Zap size={14} aria-hidden="true" />,
  ariaLabel: (members) =>
    members.length > 1
      ? `${members.length} skills used`
      : `Skill: ${members[0]?.data.name ?? "unknown"}`,
  renderTooltip: (members) => <SkillMarkerTooltip members={members} />,
}
