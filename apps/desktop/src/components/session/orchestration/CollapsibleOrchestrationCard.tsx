// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronDown, ChevronUp } from "lucide-react"
import { useState, type ReactNode } from "react"

export interface CollapsibleOrchestrationCardProps {
  /** Leading glyph in the header. */
  icon?: ReactNode
  /** Collapsed header text. */
  title: string
  /** Extra classes for the expanded body wrapper, on top of the base padding. */
  bodyClassName?: string
  /** Revealed when expanded. */
  children: ReactNode
}

/**
 * The shared card the orchestration badges are built on: an indigo surface
 * with a collapsible header — glyph, title, and a chevron that appears on
 * hover — that expands to reveal its body.
 *
 * The chevron is hover-revealed rather than always painted because these cards
 * sit above a session's charts and only some of them have anything worth
 * expanding; a permanent chevron would advertise depth on every one. The header
 * is a real `button` with `aria-expanded`, so the disclosure is announced and
 * keyboard-operable whether or not the chevron is showing.
 */
export function CollapsibleOrchestrationCard({
  icon,
  title,
  bodyClassName,
  children,
}: CollapsibleOrchestrationCardProps) {
  const [expanded, setExpanded] = useState(false)
  const Chevron = expanded ? ChevronUp : ChevronDown

  return (
    <div className="mx-3 mb-3 overflow-hidden rounded-xl bg-system-indigo/10">
      <button
        type="button"
        onClick={() => setExpanded((e) => !e)}
        aria-expanded={expanded}
        className="group flex w-full items-center gap-1.5 px-3 py-2 text-left transition-colors hover:bg-system-indigo/15"
      >
        {icon}
        <span className="type-callout text-system-indigo-text">{title}</span>
        <Chevron
          size={14}
          aria-hidden="true"
          className="ml-auto shrink-0 text-system-indigo-text opacity-0 transition-opacity group-hover:opacity-100"
        />
      </button>

      {expanded && <div className={`px-2 pt-0.5 pb-2 ${bodyClassName ?? ""}`}>{children}</div>}
    </div>
  )
}
