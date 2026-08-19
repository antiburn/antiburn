// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { ReactNode } from "react"

/** A labelled group of cards. The header sits a step above the row labels
 *  inside those cards (15px vs 13px, full-contrast vs secondary), so a pane
 *  scans as titled groups rather than one undifferentiated run of rows.
 *
 *  The weight override drops the 600 that `type-title-3` bakes in: size and
 *  contrast carry the hierarchy here, and at this size the semibold read as
 *  shouting over the rows it introduces.
 *
 *  It needs the `!` because the `.type-*` scale is declared outside any
 *  `@layer` (styles/typography.css), and unlayered CSS outranks anything in
 *  Tailwind's `utilities` layer — a plain `font-normal` here silently does
 *  nothing. */
export function SectionGroup({
  title,
  trailing,
  children,
  className = "",
}: {
  title?: string
  /** Optional content pinned to the header's right-hand side (e.g. a status
   *  badge) — rendered only alongside a `title`. */
  trailing?: ReactNode
  children: ReactNode
  className?: string
}) {
  return (
    <section className={`space-y-2 ${className}`.trimEnd()}>
      {title && (
        <div className="flex items-center justify-between gap-2 px-1">
          <h2 className="type-title-3 font-normal! text-label">{title}</h2>
          {trailing}
        </div>
      )}
      {children}
    </section>
  )
}
