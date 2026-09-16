import { useId, useRef, useState, type ReactNode, type KeyboardEvent } from "react"

import { ScrollPane } from "../../components/ui/ScrollPane"
import { useViewportWidth } from "../../lib/viewport"

export interface CollectionItem {
  id: string
  label: string
  description?: string
}

interface CollectionSelection<T extends CollectionItem> {
  compact: boolean
  selectedId: string | null
  select: (item: T) => void
  openDetail: (item: T) => void
}

interface CollectionDetailPaneProps<T extends CollectionItem> {
  title: string
  items: readonly T[]
  emptyMessage: string
  detailEmptyMessage: string
  /** A feature may supply its own unselected detail presentation. */
  detailEmptyContent?: ReactNode
  renderDetail: (item: T) => ReactNode
  selection?: T | null
  onSelectionChange?: (item: T) => void
  /** A shell request that must reveal the compact detail pane. */
  externalDetailRevealRevision?: number
  detailOwnsViewport?: boolean
  /** Feature lists own their viewport when this slot is supplied. */
  renderCollection?: (selection: CollectionSelection<T>) => ReactNode
}

/** Provide stable collection and detail columns without owning feature data. */
export function CollectionDetailPane<T extends CollectionItem>({
  title,
  items,
  emptyMessage,
  detailEmptyMessage,
  detailEmptyContent,
  renderDetail,
  renderCollection,
  selection: controlledSelection,
  onSelectionChange,
  externalDetailRevealRevision,
  detailOwnsViewport = false,
}: CollectionDetailPaneProps<T>) {
  const compact = useViewportWidth() < 900
  const [detailOpen, setDetailOpen] = useState(false)
  const [dismissedExternalRevealRevision, setDismissedExternalRevealRevision] = useState<
    number | null
  >(null)
  const focusedExternalRevealRevision = useRef<number | null>(null)
  const restoreCollectionFocus = useRef(false)
  const id = useId()
  const [localSelection, setSelection] = useState<T | null>(null)
  const selection = controlledSelection === undefined ? localSelection : controlledSelection
  const selected = items.find((item) => item.id === selection?.id) ?? selection
  const selectedId = selected?.id ?? null
  const select = (item: T) => {
    if (onSelectionChange) onSelectionChange(item)
    else setSelection(item)
  }
  const openDetail = (item: T) => {
    select(item)
    setDetailOpen(true)
    queueMicrotask(() => document.getElementById(`${id}-detail`)?.focus())
  }

  const hasExternalDetailReveal =
    (externalDetailRevealRevision ?? 0) > 0 &&
    externalDetailRevealRevision !== dismissedExternalRevealRevision
  const showDetail = !compact || (selected !== null && (detailOpen || hasExternalDetailReveal))
  const backToCollection = () => {
    restoreCollectionFocus.current = true
    setDismissedExternalRevealRevision(externalDetailRevealRevision ?? null)
    setDetailOpen(false)
  }

  return (
    <div className="main-window-collection-detail" data-single-pane={compact || undefined}>
      <section
        ref={(node) => {
          if (!node || (compact && showDetail) || !restoreCollectionFocus.current) return
          node.focus()
          const restore = () => {
            if (document.activeElement !== node) {
              restoreCollectionFocus.current = false
              return true
            }
            const target = node.querySelector<HTMLElement>(
              '[aria-selected="true"], [aria-current="true"]',
            )
            if (!target) return false
            target.focus({ preventScroll: true })
            restoreCollectionFocus.current = false
            return true
          }
          if (restore()) return
          const observer = new MutationObserver(() => {
            if (restore()) observer.disconnect()
          })
          observer.observe(node, { childList: true, subtree: true })
          return () => observer.disconnect()
        }}
        tabIndex={-1}
        onBlur={(event) => {
          if (
            event.target === event.currentTarget ||
            !event.currentTarget.contains(event.relatedTarget)
          )
            restoreCollectionFocus.current = false
        }}
        hidden={compact && showDetail}
        className="main-window-collection"
        aria-labelledby={`${id}-collection-title`}
      >
        <h1 id={`${id}-collection-title`} className="sr-only">
          {title}
        </h1>
        {renderCollection ? (
          renderCollection({ compact, selectedId, select, openDetail })
        ) : (
          <ScrollPane className="min-h-0" viewportClassName="main-window-collection-scroll">
            <CollectionList
              compact={compact}
              items={items}
              selectedId={selectedId}
              select={select}
              openDetail={openDetail}
              label={title}
              emptyMessage={emptyMessage}
            />
          </ScrollPane>
        )}
      </section>
      <section
        hidden={!showDetail}
        data-detail-pane
        ref={(node) => {
          if (
            !node ||
            !hasExternalDetailReveal ||
            focusedExternalRevealRevision.current === externalDetailRevealRevision
          )
            return
          focusedExternalRevealRevision.current = externalDetailRevealRevision ?? null
          queueMicrotask(() => {
            if (node.isConnected && !node.hidden) node.focus()
          })
        }}
        onFocus={(event) => {
          if (event.target === event.currentTarget)
            event.currentTarget
              .querySelector<HTMLElement>("[data-detail-focus-target]")
              ?.focus()
        }}
        id={`${id}-detail`}
        tabIndex={-1}
        className="main-window-detail"
        aria-labelledby={`${id}-detail-title`}
      >
        {compact && (
          <div className="collection-back">
            <button type="button" className="ui-push-button" onClick={backToCollection}>
              Back to {title.toLowerCase()}
            </button>
          </div>
        )}
        <h2 id={`${id}-detail-title`} tabIndex={-1} className="sr-only">
          {selected?.label ?? "Details"}
        </h2>
        {selected && !items.some((item) => item.id === selected.id) && (
          <p role="status" className="type-callout text-label-secondary px-4 py-2">
            This item is outside the current list.
          </p>
        )}
        {!selected && detailEmptyContent ? (
          detailEmptyContent
        ) : detailOwnsViewport && selected ? (
          renderDetail(selected)
        ) : (
          <ScrollPane
            key={selectedId}
            className="min-h-0"
            viewportClassName="main-window-detail-scroll"
          >
            <div className="main-window-pane">
              {selected ? (
                renderDetail(selected)
              ) : (
                <p className="type-body text-label-secondary">{detailEmptyMessage}</p>
              )}
            </div>
          </ScrollPane>
        )}
      </section>
    </div>
  )
}

/** The default collection has one focus target and no nested controls per row. */
function CollectionList<T extends CollectionItem>({
  compact,
  items,
  selectedId,
  select,
  openDetail,
  label,
  emptyMessage,
}: CollectionSelection<T> & { items: readonly T[]; label: string; emptyMessage: string }) {
  const listId = useId()
  const refs = useRef(new Map<string, HTMLDivElement>())
  const tabStop = items.some((item) => item.id === selectedId) ? selectedId : items[0]?.id
  function onKeyDown(event: KeyboardEvent<HTMLDivElement>, index: number): void {
    let next: number
    if (event.key === "ArrowDown") next = Math.min(index + 1, items.length - 1)
    else if (event.key === "ArrowUp") next = Math.max(index - 1, 0)
    else if (event.key === "Home") next = 0
    else if (event.key === "End") next = items.length - 1
    else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault()
      const item = items[index]
      if (item) {
        if (event.key === "Enter") openDetail(item)
        else select(item)
      }
      return
    } else return
    event.preventDefault()
    const item = items[next]
    if (!item) return
    select(item)
    refs.current.get(item.id)?.focus()
  }
  if (!items.length)
    return (
      <p className="main-window-list-empty type-body text-label-secondary">{emptyMessage}</p>
    )
  return (
    <div role="listbox" aria-label={label} className="main-window-collection-list">
      {items.map((item, index) => (
        <div
          key={item.id}
          ref={(node) => {
            if (node) refs.current.set(item.id, node)
            else refs.current.delete(item.id)
          }}
          role="option"
          aria-selected={item.id === selectedId}
          aria-label={item.label}
          aria-describedby={item.description ? `${listId}-${item.id}-description` : undefined}
          tabIndex={item.id === tabStop ? 0 : -1}
          className="main-window-collection-row rounded-control text-label"
          onClick={(event) => {
            event.currentTarget.focus()
            if (compact) openDetail(item)
            else select(item)
          }}
          onKeyDown={(event) => onKeyDown(event, index)}
        >
          <span className="type-body block truncate" title={item.label}>
            {item.label}
          </span>
          {item.description && (
            <span
              id={`${listId}-${item.id}-description`}
              className="type-footnote block truncate text-label-secondary"
              title={item.description}
            >
              {item.description}
            </span>
          )}
        </div>
      ))}
    </div>
  )
}
