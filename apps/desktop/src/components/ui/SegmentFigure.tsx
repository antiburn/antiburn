// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cn } from "../../lib/cn"

/**
 * The numeric readout of an instrument.
 *
 * The figure takes the type style of the label beside it and changes only the
 * digits: `tabular-nums` gives every digit the same width, so a figure never
 * shifts as it ticks. An earlier draft set a monospace family here, and the
 * review removed it — a second typeface for a two-character percentage read
 * as a different kind of thing from the label it sits beside.
 *
 * Every numeric readout renders through this one component. The LED
 * segment-display treatment the design asks for has no visual reference yet;
 * when one arrives, the style changes here and nowhere else. Do not write
 * `tabular-nums` at a call site — that is what breaks the funnel.
 */
export function SegmentFigure({
  children,
  className = "",
}: {
  children: string
  className?: string
}) {
  return <span className={cn("tabular-nums", className)}>{children}</span>
}
