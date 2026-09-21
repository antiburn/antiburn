import { Search, X } from "lucide-react"
import { useCallback, useId, useRef, useState } from "react"
import { groupAppResults, type AppSearchResult } from "../../lib/appSearch"

export function AppSearch({
  onChoose,
  onClose,
}: {
  onChoose: (result: AppSearchResult) => Promise<void>
  onClose: () => void
}) {
  const [query, setQuery] = useState("")
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [error, setError] = useState(false)
  const [pending, setPending] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  const dialog = useRef<HTMLDialogElement>(null)
  const choosing = useRef(false)
  const live = useRef(false)
  const id = useId()
  const groups = groupAppResults(query)
  const results = groups.flatMap((group) => group.results)
  const active = results.find((result) => result.id === selectedId) ?? results[0]

  async function choose(result: AppSearchResult) {
    if (choosing.current) return
    choosing.current = true
    setPending(true)
    setError(false)
    // Release modal focus before the destination reveals its own target.
    dialog.current?.close()
    try {
      await onChoose(result)
      if (live.current) onClose()
    } catch {
      if (live.current) {
        choosing.current = false
        setError(true)
        setPending(false)
        dialog.current?.showModal()
        input.current?.focus()
      }
    }
  }
  const mountDialog = useCallback((node: HTMLDialogElement | null) => {
    if (!node) return
    dialog.current = node
    live.current = true
    node.showModal()
    input.current?.focus()
    return () => {
      live.current = false
      node.close()
      if (!choosing.current) {
        const target = document.querySelector<HTMLButtonElement>("[data-app-search-trigger]")
        target?.focus()
      }
    }
  }, [])
  return (
    <dialog
      aria-label="Search antiburn"
      className="app-search rounded-control border border-separator bg-surface-overlay text-label shadow-popover"
      ref={mountDialog}
      onCancel={(event) => {
        event.preventDefault()
        onClose()
      }}
      onClick={(event) => {
        if (event.target !== event.currentTarget) return
        const bounds = event.currentTarget.getBoundingClientRect()
        if (
          event.clientX < bounds.left ||
          event.clientX > bounds.right ||
          event.clientY < bounds.top ||
          event.clientY > bounds.bottom
        )
          onClose()
      }}
    >
      <div className="flex items-center gap-3 border-b border-separator px-4 py-2">
        <Search size={16} aria-hidden="true" className="shrink-0 text-label-secondary" />
        <input
          ref={input}
          role="combobox"
          aria-label="Search antiburn"
          aria-autocomplete="list"
          aria-expanded="true"
          aria-controls={`${id}-results`}
          aria-activedescendant={active ? `${id}-${active.id}` : undefined}
          className="min-w-0 flex-1 bg-transparent type-body"
          placeholder="Search views, features, settings and checks…"
          maxLength={200}
          value={query}
          onChange={(event) => {
            setQuery(event.target.value)
            setSelectedId(null)
            setError(false)
          }}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault()
              const index = results.findIndex((result) => result.id === active?.id)
              const next =
                results[
                  (index + (event.key === "ArrowDown" ? 1 : -1) + results.length) %
                    results.length
                ]
              if (next) {
                setSelectedId(next.id)
                document
                  .getElementById(`${id}-${next.id}`)
                  ?.scrollIntoView({ block: "nearest" })
              }
            } else if (event.key === "Enter" && active) {
              event.preventDefault()
              void choose(active)
            }
          }}
        />
        <button
          type="button"
          aria-label="Close search"
          className="main-window-tool"
          onClick={onClose}
        >
          <X size={16} aria-hidden="true" />
        </button>
      </div>
      {error && (
        <p role="alert" className="px-4 py-2 type-body text-system-red-text">
          Could not open this destination. Try again.
        </p>
      )}
      <div
        id={`${id}-results`}
        role="listbox"
        aria-label="Search results"
        aria-busy={pending}
        className="app-search-results p-2"
      >
        {groups.map((group, index) => (
          <div key={group.label} role="group" aria-labelledby={`${id}-group-${index}`}>
            <div
              id={`${id}-group-${index}`}
              role="presentation"
              className="px-3 pb-1 pt-3 type-caption text-label-secondary"
            >
              {group.label}
            </div>
            {group.results.map((result) => (
              <div
                key={result.id}
                id={`${id}-${result.id}`}
                role="option"
                aria-selected={active?.id === result.id}
                className="app-search-result rounded-control px-3 py-2"
                onPointerMove={() => setSelectedId(result.id)}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => void choose(result)}
              >
                <span className="block type-body text-label">{result.label}</span>
                {result.detail !== group.label && (
                  <span className="block type-caption text-label-secondary">
                    {result.detail}
                  </span>
                )}
              </div>
            ))}
          </div>
        ))}
      </div>
      <p
        role="status"
        className={results.length ? "sr-only" : "px-4 py-6 type-body text-label-secondary"}
      >
        {results.length ? `${results.length} results` : "No matching destinations."}
      </p>
      <p className="border-t border-separator px-4 py-2 type-caption text-label-secondary">
        ↑ ↓ to move · Enter to open · Esc to close
      </p>
    </dialog>
  )
}
