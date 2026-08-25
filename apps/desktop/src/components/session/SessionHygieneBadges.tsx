// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  Brain,
  Check,
  Droplet,
  Hammer,
  LifeBuoy,
  Rabbit,
  Sunset,
  X,
  type LucideIcon,
} from "lucide-react"

import { cn } from "../../lib/cn"
import type { MockSessionHygieneCheck } from "../../lib/presentation/mockSessionHygiene"
import { Tooltip } from "../presentation/Tooltip"

/* One glyph per check id. The glyph names the check; the color names the state. */
const CHECK_ICONS: Record<MockSessionHygieneCheck["id"], LucideIcon> = {
  sessionOverdepth: LifeBuoy,
  modelOverthinking: Brain,
  overpoweredSubagents: Hammer,
  obsoleteModel: Sunset,
  fastModeOveruse: Rabbit,
  excessCacheRehydration: Droplet,
}

function HygieneGlyph({
  check,
  markIndex,
}: {
  check: MockSessionHygieneCheck
  markIndex: number
}) {
  const Icon = CHECK_ICONS[check.id]
  const Mark = check.passed ? Check : X
  return (
    <Tooltip label={check.title} delayMs={150}>
      <span
        aria-label={check.title}
        // The index sets the mark's place in the left-to-right sequence.
        style={{ "--hygiene-mark-index": markIndex } as React.CSSProperties}
        className={cn(
          "session-hygiene-glyph relative flex h-5 w-5 shrink-0 items-center justify-center",
          check.passed ? "text-label-tertiary" : "text-brand-tint",
          !check.passed && "mt-[1.5px]",
        )}
      >
        <Icon size={14} strokeWidth={1.75} aria-hidden="true" />
        {/* The state mark rises in above the glyph on row hover; the glyph
            label carries the state for assistive tech. */}
        <span
          aria-hidden="true"
          className={cn(
            "session-hygiene-mark flex items-center justify-center",
            check.passed ? "text-(--color-system-green)" : "text-brand-tint",
          )}
        >
          <Mark size={9} strokeWidth={3} />
        </span>
      </span>
    </Tooltip>
  )
}

/**
 * Bare hygiene glyphs for one session row.
 *
 * Failed checks render in the flow, brand orange, always visible. Passed
 * checks render inside `.session-hygiene-pass`, which session-rows.css
 * anchors to the left edge of the rail and fans out on row hover.
 *
 * The fragment relies on its host: the rail must be a positioned flex row,
 * and the row must carry the `session-row` class for the hover trigger.
 */
export function SessionHygieneBadges({ checks }: { checks: MockSessionHygieneCheck[] }) {
  const passed = checks.filter((check) => check.passed)
  const failed = checks.filter((check) => !check.passed)
  return (
    <>
      {passed.length > 0 && (
        <div className="session-hygiene-pass">
          {passed.map((check, index) => (
            <HygieneGlyph key={check.id} check={check} markIndex={index} />
          ))}
        </div>
      )}
      {failed.length > 0 && (
        // The -top-px trues the glyphs up against the cost pill.
        <div className="relative -top-px flex items-center gap-1">
          {failed.map((check, index) => (
            <HygieneGlyph key={check.id} check={check} markIndex={passed.length + index} />
          ))}
        </div>
      )}
    </>
  )
}
