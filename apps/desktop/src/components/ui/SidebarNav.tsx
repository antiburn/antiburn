// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { LucideIcon } from "lucide-react"
import { Fragment, useRef, type KeyboardEvent, type ReactNode } from "react"

export type SidebarNavItem = {
  id: string
  label: string
  icon: LucideIcon
  /** Draw a hairline above this row to set it apart from the group before it. */
  separatorBefore?: boolean
}

/** Source-list navigation for a multi-pane window.
 *
 *  Exposed as a vertical tablist with a roving tabindex: only the selected row
 *  is tabbable, and ↑/↓/Home/End move both selection and focus. Each row's
 *  `aria-controls` points at `<id>-panel`, so the pane it drives should carry
 *  that id with `role="tabpanel"` and `aria-labelledby="<id>-tab"`.
 *
 *  `role="tablist"` lives on the inner scroller, not the root chrome
 *  container, because an optional `footer` can render below the row list — a
 *  quit action, say — and a non-`role="tab"` child of a tablist is an ARIA
 *  violation. Splitting chrome (width/border/background) from role
 *  (scroll/selection/keyboard) keeps the footer outside the tablist entirely
 *  while still reading as one sidebar column. */
export function SidebarNav({
  items,
  value,
  onChange,
  ariaLabel,
  className = "",
  header,
  footer,
}: {
  items: ReadonlyArray<SidebarNavItem>
  value: string
  onChange: (next: string) => void
  ariaLabel: string
  className?: string
  /** Optional non-tab content above the row list. */
  header?: ReactNode
  /** Optional non-tab content pinned below the row list, past a hairline. */
  footer?: ReactNode
}) {
  const rowRefs = useRef(new Map<string, HTMLButtonElement>())

  function handleKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (items.length === 0) return
    const current = items.findIndex((item) => item.id === value)
    let next: number
    if (e.key === "ArrowDown") next = (current + 1) % items.length
    else if (e.key === "ArrowUp") next = (current - 1 + items.length) % items.length
    else if (e.key === "Home") next = 0
    else if (e.key === "End") next = items.length - 1
    else return

    e.preventDefault()
    const target = items[next]
    if (!target || target.id === value) return
    onChange(target.id)
    // The row already exists in the DOM regardless of selection, so focus can
    // move synchronously — no need to wait for the re-render.
    rowRefs.current.get(target.id)?.focus()
  }

  // Geometry, all on the 4px spacing scale: `h-9` (36px) rows spaced by `gap-2`
  // (8px). Taller than the 28px source-list minimum on purpose — the rows read
  // as a primary navigation surface at this size, and the extra gap keeps each
  // selection fill and focus ring a distinct `rounded-control` shape instead of
  // merging into one continuous block. Group hairlines stay at `my-1`, so a
  // group break reads as 25px of separation against the 8px plain row gap.
  return (
    <div
      className={`flex w-[var(--sidebar-width)] shrink-0 flex-col border-r border-separator bg-surface-sidebar ${className}`.trimEnd()}
    >
      {header && <div className="px-5 pb-1 pt-4">{header}</div>}
      <div
        role="tablist"
        aria-orientation="vertical"
        aria-label={ariaLabel}
        onKeyDown={handleKeyDown}
        className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto px-2 py-3"
      >
        {items.map((item) => {
          const Icon = item.icon
          const selected = item.id === value
          return (
            <Fragment key={item.id}>
              {item.separatorBefore && (
                <div role="presentation" className="my-1 h-px bg-separator" />
              )}
              <button
                ref={(node) => {
                  if (node) rowRefs.current.set(item.id, node)
                  else rowRefs.current.delete(item.id)
                }}
                type="button"
                role="tab"
                id={`${item.id}-tab`}
                aria-selected={selected}
                aria-controls={`${item.id}-panel`}
                tabIndex={selected ? 0 : -1}
                onClick={() => onChange(item.id)}
                className={`type-body flex h-9 items-center gap-3 rounded-control px-3 transition-colors duration-[120ms] ease-out ${
                  selected
                    ? "bg-surface-selected text-label"
                    : "text-label hover:bg-surface-hover"
                }`}
              >
                <Icon size={16} strokeWidth={2} className="shrink-0" aria-hidden="true" />
                <span className="truncate">{item.label}</span>
              </button>
            </Fragment>
          )
        })}
      </div>
      {footer && (
        <div className="px-2 pb-3">
          <div role="presentation" className="mb-2 h-px bg-separator" />
          {footer}
        </div>
      )}
    </div>
  )
}
