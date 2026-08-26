// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Check, Copy, Info } from "lucide-react"
import { useId, useState } from "react"

import { cn } from "../../lib/cn"
import { HOTSPOT_COPY, hotspotCountLabel, type HotspotFinding } from "./hotspot"

/**
 * The block at the foot of the activity surface.
 *
 * It names the most common Hygiene & Efficiency finding across the assessed
 * 30-day cohort, says how many sessions carry it, and hands over one pasteable
 * fix. The info icon on the claim line opens the evidence behind the count.
 *
 * Four things about it are decided rather than incidental:
 *
 * - **No finding, no block.** With nothing to say this renders `null`, and the
 *   session list takes the space back. Not an empty shell and not an "all
 *   clear" message: a cohort that is half-read must never render as clean
 *   (FR-14), and nothing at all cannot be misread.
 * - **The window never resizes.** The activity surface stays at
 *   `DEFAULT_POPOVER_HEIGHT`. Opening the detail takes its height out of the
 *   session list, which already scrolls. A finding with many evidence rows
 *   scrolls inside the detail instead of pushing the list out of the window.
 * - **The detail opens below the fix field.** The pasteable line sits in the
 *   same place open or closed, so the reader can copy without waiting for a
 *   reflow to settle.
 * - **One orange mark, and it is the rule across the top.** The fix field is a
 *   plain stroked field and the count is ordinary label text. The brand carries
 *   further when it marks one thing than when it fills three.
 */

/** How long the field shows a check after a successful write. */
const COPIED_FEEDBACK_MS = 2000

/**
 * How tall the opened detail may grow before it scrolls.
 *
 * The block earns its height from the session list. Past this the list would
 * have no rows left to give, so the evidence scrolls in place instead.
 */
const DETAIL_MAX_HEIGHT = "max-h-56"

export interface HotspotBlockProps {
  /** The winning finding, or null when the report has nothing to say. */
  finding: HotspotFinding | null
}

export function HotspotBlock({ finding }: HotspotBlockProps) {
  const [open, setOpen] = useState(false)
  const [copied, setCopied] = useState(false)
  const detailId = useId()

  // Reading `finding` before the hooks would change the hook count between a
  // report with a finding and one without.
  if (!finding) return null

  const copy = HOTSPOT_COPY[finding.category]

  // The revert is work the click causes, so the click owns it. A timer that
  // outlives the block sets state on an unmounted component, which React
  // ignores; an effect here would only exist to cancel a timer nobody is
  // waiting on.
  const onCopy = () => {
    void navigator.clipboard
      .writeText(finding.fix)
      .then(() => {
        setCopied(true)
        setTimeout(() => setCopied(false), COPIED_FEEDBACK_MS)
      })
      .catch(() => {
        // The webview can refuse the write — no user activation, or no focus.
        // The check never appears, which is the honest report: the reader can
        // select the line and copy it. A 26px field has no room to say more,
        // and an unhandled rejection would only shout in the console.
      })
  }

  return (
    <div className="shrink-0 border-t border-brand-tint">
      <div className="flex flex-col gap-1.5 px-4 pt-2.5 pb-3">
        {/* The whole claim line is the disclosure control, not just the icon:
            a 12px glyph is a poor target for the one gesture this block
            offers. */}
        <button
          type="button"
          aria-expanded={open}
          aria-controls={detailId}
          onClick={() => setOpen((value) => !value)}
          // Same reason as `ui/Disclosure`: the global button:active rule
          // (styles/controls.css) makes a heading twitch.
          className="-mx-1 flex items-baseline gap-1.5 rounded-control px-1 text-left active:transform-none active:opacity-100"
        >
          <Info
            size={12}
            strokeWidth={2}
            aria-hidden="true"
            className={cn(
              "shrink-0 self-center transition-colors duration-[var(--duration-fast)]",
              open ? "text-label" : "text-label-tertiary",
            )}
          />
          <span className="type-body whitespace-nowrap text-label tabular-nums">
            {hotspotCountLabel(finding.sessions)}
          </span>
          <span className="type-body min-w-0 truncate text-label">{copy.name}</span>
          {finding.saving && (
            <span className="type-body ml-auto whitespace-nowrap text-label-tertiary tabular-nums">
              {finding.saving}
            </span>
          )}
        </button>

        {/* The field itself copies. A 13px icon target beside a wide, obviously
            selectable command is the smaller of the two things a reader aims
            at, so the whole field takes the click and the icon only reports
            what happened. */}
        <button
          type="button"
          onClick={onCopy}
          aria-label={copied ? `Copied ${finding.fix}` : `Copy ${finding.fix}`}
          className="flex h-[26px] items-center gap-2 rounded-control border border-separator px-2 text-left active:transform-none active:opacity-100"
        >
          <code className="type-footnote min-w-0 flex-1 truncate font-mono text-label">
            {finding.fix}
          </code>
          {copied ? (
            <Check
              size={13}
              strokeWidth={2.5}
              aria-hidden="true"
              className="shrink-0 text-system-green"
            />
          ) : (
            <Copy
              size={13}
              strokeWidth={2}
              aria-hidden="true"
              className="shrink-0 text-label"
            />
          )}
        </button>

        {/* Unmounted rather than hidden, so closed evidence stays out of the
            accessibility tree and out of find-in-page. */}
        {open && (
          <div
            id={detailId}
            className={cn(
              "mt-1 flex flex-col gap-2 overflow-y-auto border-t border-separator pt-2",
              DETAIL_MAX_HEIGHT,
            )}
          >
            <p className="type-footnote m-0 text-pretty text-label-secondary">
              {copy.mechanism}
            </p>
            <dl className="m-0 flex flex-col">
              {finding.evidence.map((row) => (
                <div
                  key={row.label}
                  className="flex items-baseline gap-2 border-b border-separator py-1 first:border-t"
                >
                  <dt className="type-caption truncate text-label-tertiary">{row.label}</dt>
                  <dd className="type-caption m-0 ml-auto whitespace-nowrap text-label tabular-nums">
                    {row.value}
                  </dd>
                </div>
              ))}
            </dl>
          </div>
        )}
      </div>
    </div>
  )
}
