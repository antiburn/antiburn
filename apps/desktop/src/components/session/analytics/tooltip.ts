// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shared surface for the analytics chart tooltips.
 *
 * These are cursor-following hover cards, not the platform `Tooltip` — they
 * track the pointer across a chart rather than anchoring to a trigger — so they
 * carry their own surface.
 */

import type { CSSProperties } from "react"

/** The frosted-glass surface every analytics chart tooltip paints on. */
export const GLASS_TOOLTIP_STYLE: CSSProperties = {
  background: "color-mix(in srgb, var(--color-surface) 88%, transparent)",
  backdropFilter: "blur(6px)",
  WebkitBackdropFilter: "blur(6px)",
  border: "1px solid var(--color-separator)",
  borderRadius: 8,
  boxShadow: "0 2px 8px rgb(0 0 0 / 0.12)",
  fontSize: 11,
}
