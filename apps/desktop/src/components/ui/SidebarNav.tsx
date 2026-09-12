import type { LucideIcon } from "lucide-react"
import { Fragment, useRef, type KeyboardEvent, type ReactNode, type RefObject } from "react"

import { cn } from "../../lib/cn"
import { CountPill } from "./CountPill"

/** A nested row under a top-level `SidebarNavItem`, one level deep. A child
 *  needs no icon of its own. */
export type SidebarNavChildItem = {
  id: string
  label: string
  icon?: LucideIcon
  /** Shown right-aligned as a muted count pill, e.g. a filtered session count. */
  count?: number
  /** Draw a hairline above this row to set it apart from the group before it. */
  separatorBefore?: boolean
  /** The panel id this row's `aria-controls` points at. Defaults to
   *  `${id}-panel`. Set this when several children share one parent panel. */
  controls?: string
}

export type SidebarNavItem = {
  id: string
  label: string
  icon: LucideIcon
  /** Shown right-aligned as a muted count pill, e.g. a filtered session count. */
  count?: number
  /** Draw a hairline above this row to set it apart from the group before it. */
  separatorBefore?: boolean
  /** Rows nested under this item, one level deep. They join the same tablist,
   *  in document order right after their parent. */
  children?: ReadonlyArray<SidebarNavChildItem>
}

/** One row in the flattened tablist: a top-level item, or one of its
 *  children. Flattening keeps keyboard navigation and the roving tabindex
 *  working over parent and child rows alike, in document order. */
type FlatRow =
  { item: SidebarNavItem; isChild: false } | { item: SidebarNavChildItem; isChild: true }

function flattenItems(items: ReadonlyArray<SidebarNavItem>): FlatRow[] {
  const rows: FlatRow[] = []
  for (const item of items) {
    rows.push({ item, isChild: false })
    for (const child of item.children ?? []) {
      rows.push({ item: child, isChild: true })
    }
  }
  return rows
}

/** Source-list navigation for a multi-pane window.
 *
 *  Exposed as a vertical tablist with a roving tabindex: only the selected row
 *  is tabbable, and ↑/↓/Home/End move both selection and focus. Each row's
 *  `aria-controls` points at `<id>-panel`, so the pane it drives should carry
 *  that id with `role="tabpanel"` and `aria-labelledby="<id>-tab"`.
 *
 *  A top-level item may nest child rows one level deep. Children flatten into
 *  the same tablist in document order (parent, then its children, then the
 *  next top-level item), so Arrow/Home/End keyboard navigation and the roving
 *  tabindex treat every row alike regardless of nesting.
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
  const rows = flattenItems(items)

  function handleKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    if (rows.length === 0) return
    const current = rows.findIndex((row) => row.item.id === value)
    let next: number
    if (e.key === "ArrowDown") next = (current + 1) % rows.length
    else if (e.key === "ArrowUp") next = (current - 1 + rows.length) % rows.length
    else if (e.key === "Home") next = 0
    else if (e.key === "End") next = rows.length - 1
    else return

    e.preventDefault()
    const target = rows[next]
    if (!target || target.item.id === value) return
    onChange(target.item.id)
    // The row already exists in the DOM regardless of selection, so focus can
    // move synchronously — no need to wait for the re-render.
    rowRefs.current.get(target.item.id)?.focus()
  }

  // Geometry, all on the 4px spacing scale: `h-9` (36px) top-level rows spaced
  // by `gap-2` (8px). Taller than the 28px source-list minimum on purpose —
  // the rows read as a primary navigation surface at this size, and the extra
  // gap keeps each selection fill and focus ring a distinct `rounded-control`
  // shape instead of merging into one continuous block. Group hairlines stay
  // at `my-1`, so a group break reads as 25px of separation against the 8px
  // plain row gap. A child row is a shorter `h-8` (32px), indented with
  // `pl-8` in place of the parent's `px-3` so its label sits clear of the
  // parent's icon column; a child's own hairline keeps that same indent.
  return (
    <div
      className={cn(
        "flex w-[var(--sidebar-width)] shrink-0 flex-col border-r border-separator bg-surface-sidebar",
        className,
      )}
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
          const selected = item.id === value
          return (
            <Fragment key={item.id}>
              {item.separatorBefore && (
                <div role="presentation" className="my-1 h-px bg-separator" />
              )}
              <SidebarNavRow
                item={item}
                selected={selected}
                onChange={onChange}
                rowRefs={rowRefs}
                heightClass="h-9"
                paddingClass="px-3"
              />
              {item.children?.map((child) => {
                const childSelected = child.id === value
                return (
                  <Fragment key={child.id}>
                    {child.separatorBefore && (
                      <div role="presentation" className="my-1 ml-8 h-px bg-separator" />
                    )}
                    <SidebarNavRow
                      item={child}
                      selected={childSelected}
                      onChange={onChange}
                      rowRefs={rowRefs}
                      heightClass="h-8"
                      paddingClass="pl-8 pr-3"
                    />
                  </Fragment>
                )
              })}
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

/** One tablist row, shared by a top-level item and a child item. */
function SidebarNavRow({
  item,
  selected,
  onChange,
  rowRefs,
  heightClass,
  paddingClass,
}: {
  item: SidebarNavItem | SidebarNavChildItem
  selected: boolean
  onChange: (next: string) => void
  rowRefs: RefObject<Map<string, HTMLButtonElement>>
  heightClass: string
  paddingClass: string
}) {
  const Icon = item.icon
  const controls = ("controls" in item && item.controls) || `${item.id}-panel`
  return (
    <button
      ref={(node) => {
        if (node) rowRefs.current.set(item.id, node)
        else rowRefs.current.delete(item.id)
      }}
      type="button"
      role="tab"
      id={`${item.id}-tab`}
      aria-selected={selected}
      aria-controls={controls}
      // Set the accessible name to the label alone when a count pill is
      // present. This keeps the name stable and free of the count digits,
      // which the row already shows as visible text.
      aria-label={item.count !== undefined ? item.label : undefined}
      tabIndex={selected ? 0 : -1}
      onClick={() => onChange(item.id)}
      className={cn(
        "type-body flex items-center gap-3 rounded-control transition-colors duration-[var(--duration-fast)] ease-out",
        heightClass,
        paddingClass,
        selected ? "bg-surface-selected text-label" : "text-label hover:bg-surface-hover",
      )}
    >
      {Icon && <Icon size={16} strokeWidth={2} className="shrink-0" aria-hidden="true" />}
      <span className="truncate">{item.label}</span>
      {item.count !== undefined && <CountPill count={item.count} className="ml-auto" />}
    </button>
  )
}
