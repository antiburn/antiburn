import { activitySearchScope, searchScopeLabel } from "../../lib/sessionSearchScope"
import { Search, X } from "lucide-react"
import {
  useCallback,
  useId,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
} from "react"
import {
  groupAppResults,
  evidenceAppResult,
  searchApp,
  sessionAppResult,
  searchTargetKey,
  type AppSearchResult,
} from "../../lib/appSearch"

import { renderAgentIcon } from "../../lib/agentIcon"
import { modelRunNames, modelShortName } from "../../lib/presentation/models"
import { EVIDENCE_KIND_LABELS } from "../../lib/presentation/sessionEvidence"
import { CountPill } from "../../components/ui/CountPill"

import { SessionSearchSession } from "./SessionSearchSession"
import { searchEvidencePreview, searchEvidenceHighlights } from "./searchEvidencePreview"
import { DeepSearchSession, deepSessionKey } from "./DeepSearchSession"
import { DeepSearchPanel, DeepSearchEmptyState } from "./DeepSearchPanel"

function selectionKey(result: AppSearchResult | undefined): string | null {
  if (!result || result.target.kind !== "session") return null
  if (result.evidence) return deepSessionKey(result.evidence.session)
  return decodeURIComponent(result.id.replace(/^local:/, "").slice("session:".length))
}

function SessionSearchMetadata({ entry }: { entry: NonNullable<AppSearchResult["session"]> }) {
  const models = modelRunNames(entry.models.map((model) => ({ model })))
  const repository = entry.repository || entry.cwdLabel
  return (
    <span className="flex min-w-0 flex-1 items-center gap-x-2 overflow-hidden whitespace-nowrap text-label-tertiary">
      <span className="inline-flex shrink-0" aria-hidden="true">
        {renderAgentIcon(entry.agent, 12, undefined, "neutral")}
      </span>
      {repository && (
        <span
          title={repository}
          className="min-w-0 shrink truncate font-mono type-metadata tabular-nums"
        >
          {repository}
        </span>
      )}
      {models[0] && (
        <span
          title={models.join("\n")}
          className="inline-flex min-w-0 shrink-0 max-w-1/2 items-baseline gap-x-1.5 type-callout"
        >
          <span className="min-w-0 truncate font-semibold! text-label-secondary">
            {modelShortName(models[0])}
          </span>
          {models.length > 1 && <CountPill count={models.length - 1} prefix="+" />}
        </span>
      )}
      {entry.wslDistro && (
        <span title={`WSL: ${entry.wslDistro}`} className="min-w-0 truncate type-caption">
          WSL: {entry.wslDistro}
        </span>
      )}
    </span>
  )
}

function EvidenceExcerpt({
  evidence,
  query,
  showSource = true,
}: {
  evidence: NonNullable<AppSearchResult["evidence"]>
  query: string
  showSource?: boolean
}) {
  return (
    <span
      className="truncated-text-lines"
      style={{ "--truncated-text-lines": 2 } as CSSProperties}
      title={`${EVIDENCE_KIND_LABELS[evidence.kind]}${evidence.truncated || evidence.coverage.state === "partial" ? " · Limited excerpt" : ""}`}
    >
      {showSource && (
        <span className="text-label-tertiary">{EVIDENCE_KIND_LABELS[evidence.kind]}: </span>
      )}
      {searchEvidenceHighlights(searchEvidencePreview(evidence.excerpt, query), query).map(
        (part, index) =>
          part.match ? (
            <mark key={index} className="bg-surface-selected text-label">
              {part.text}
            </mark>
          ) : (
            part.text
          ),
      )}
    </span>
  )
}

export function AppSearch({
  activityWindowDays = 7,
  onChoose,
  onClose,
}: {
  onChoose: (result: AppSearchResult) => Promise<void>
  onClose: () => void
  activityWindowDays?: number
}) {
  const [scope] = useState(() => activitySearchScope(activityWindowDays))
  const scopeLabel = searchScopeLabel(scope)
  const [sessionSearch] = useState(() => {
    const session = new SessionSearchSession()
    session.setScope(scope)
    return session
  })
  const [deepSearch] = useState(() => {
    const session = new DeepSearchSession()
    session.setScope(scope)
    return session
  })
  const deep = useSyncExternalStore(
    deepSearch.subscribe,
    deepSearch.getSnapshot,
    deepSearch.getSnapshot,
  )
  const sessions = useSyncExternalStore(
    sessionSearch.subscribe,
    sessionSearch.getSnapshot,
    sessionSearch.getSnapshot,
  )
  const { query } = sessions
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [error, setError] = useState(false)
  const [pending, setPending] = useState(false)
  const input = useRef<HTMLInputElement>(null)
  const startButtonRef = useCallback((node: HTMLButtonElement | null) => {
    if (!node) return
    return () => {
      if (document.activeElement === node) input.current?.focus()
    }
  }, [])
  const dialog = useRef<HTMLDialogElement>(null)
  const keyboardNavigation = useRef(false)
  const arrivals = useRef({ query: "", keys: new Set<string>() })
  const choosing = useRef(false)
  const live = useRef(false)
  const id = useId()
  const evidenceBySession = new Map(
    deep.results
      .map(evidenceAppResult)
      .filter((result) => result.evidence)
      .map((result) => [searchTargetKey(result), result]),
  )
  const allSessions = sessions.results.map(sessionAppResult).map((result) => {
    const evidence = evidenceBySession.get(searchTargetKey(result))
    return evidence ? { ...evidence, id: result.id } : result
  })
  const visibleSessions = allSessions.slice(0, sessions.expanded ? undefined : 5)
  const selectedSession = allSessions.find((result) => result.id === selectedId)
  if (selectedSession && !visibleSessions.includes(selectedSession))
    visibleSessions[visibleSessions.length - 1] = selectedSession
  const catalogResults = searchApp(query)
  const deterministicGroups = groupAppResults(
    query,
    catalogResults,
    visibleSessions,
    selectedId,
  )
  const deterministicResults = deterministicGroups.flatMap((group) => group.results)
  const quickTargets = new Set(deterministicResults.map(searchTargetKey))
  const deepResults = deep.results
    .map(evidenceAppResult)
    .filter((result) => !quickTargets.has(searchTargetKey(result)))
  const searching = sessions.loading || sessions.indexing
  const quickGroups = deterministicGroups
  const proposedGroups = deepResults.length
    ? [...quickGroups, { label: "Search results", results: deepResults }]
    : quickGroups
  const [order, setOrder] = useState<{
    query: string
    keys: string[]
    labels: Record<string, string>
  }>({ query, keys: [], labels: {} })
  const proposedResults = proposedGroups.flatMap((group) => group.results)
  const proposedKeys = proposedResults.map(searchTargetKey)
  const keys =
    deep.phase !== "idle" && order.query === query
      ? [
          ...order.keys.filter((key) => proposedKeys.includes(key)),
          ...proposedKeys.filter((key) => !order.keys.includes(key)),
        ]
      : proposedKeys
  const selectedKey = proposedResults.find((result) => result.id === selectedId)
  if (selectedKey && deep.phase !== "idle" && order.query === query) {
    const key = searchTargetKey(selectedKey)
    const previousIndex = order.keys.indexOf(key)
    const currentIndex = keys.indexOf(key)
    if (previousIndex >= 0 && currentIndex >= 0 && previousIndex !== currentIndex) {
      keys.splice(currentIndex, 1)
      keys.splice(Math.min(previousIndex, keys.length), 0, key)
    }
  }
  const proposedLabels = Object.fromEntries(
    proposedGroups.flatMap((group) =>
      group.results.map((result) => [searchTargetKey(result), group.label]),
    ),
  )
  const labels =
    deep.phase !== "idle" && order.query === query
      ? Object.fromEntries(keys.map((key) => [key, order.labels[key] ?? proposedLabels[key]!]))
      : proposedLabels
  if (order.query !== query || order.keys.join("\n") !== keys.join("\n"))
    setOrder({ query, keys, labels })
  const groups =
    deep.phase === "idle"
      ? proposedGroups
      : keys.reduce<{ label: string; results: AppSearchResult[] }[]>((groups, key) => {
          const result = proposedResults.find((result) => searchTargetKey(result) === key)
          if (!result) return groups
          const label = labels[key] ?? "Search results"
          const last = groups.at(-1)
          if (last?.label === label) last.results.push(result)
          else groups.push({ label, results: [result] })
          return groups
        }, [])
  const canShowMore = (!sessions.expanded && sessions.results.length > 5) || sessions.hasMore
  const results = groups.flatMap((group) => group.results)
  const contentComplete =
    deep.phase === "finished" && Boolean(deep.response?.coverage.scopeExhausted)
  const contentEmpty =
    Boolean(query.trim()) &&
    !results.length &&
    !sessions.loading &&
    !sessions.indexing &&
    contentComplete
  const active =
    results.find((result) => result.id === selectedId) ??
    deterministicResults[0] ??
    (deep.phase !== "idle" ? results[0] : undefined)
  if (active && selectedId !== active.id) setSelectedId(active.id)

  async function choose(result: AppSearchResult) {
    if (choosing.current) return
    choosing.current = true
    setPending(true)
    setError(false)
    deepSearch.close()
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
  const mountDialog = useCallback(
    (node: HTMLDialogElement | null) => {
      if (!node) return
      dialog.current = node
      live.current = true
      node.showModal()
      input.current?.focus()
      return () => {
        live.current = false
        deepSearch.close()
        node.close()
        if (!choosing.current) {
          const target = document.querySelector<HTMLButtonElement>("[data-app-search-trigger]")
          target?.focus()
        }
      }
    },
    [deepSearch],
  )
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
          placeholder="Search views, settings, sessions and checks…"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
          maxLength={200}
          value={query}
          onChange={(event) => {
            sessionSearch.setQuery(event.target.value)
            deepSearch.setQuery(
              event.target.value,
              !(event.nativeEvent as InputEvent).isComposing,
            )
            setSelectedId(searchApp(event.target.value)[0]?.id ?? null)
            setError(false)
          }}
          onCompositionStart={() => deepSearch.close()}
          onCompositionEnd={(event) => deepSearch.setQuery(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.nativeEvent.isComposing) return
            if (event.key === "ArrowDown" || event.key === "ArrowUp") {
              event.preventDefault()
              keyboardNavigation.current = true
              dialog.current?.setAttribute("data-keyboard-navigation", "true")
              const index = results.findIndex((result) => result.id === active?.id)
              if (
                event.key === "ArrowDown" &&
                !sessions.error &&
                canShowMore &&
                index >= results.length - 2
              ) {
                sessionSearch.more()
                if (index === results.length - 1) return
              }
              const next =
                results[
                  (index + (event.key === "ArrowDown" ? 1 : -1) + results.length) %
                    results.length
                ]
              if (next) {
                setSelectedId(next.id)
                deepSearch.select(selectionKey(next))
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
        className="app-search-results"
        onWheel={() => {
          keyboardNavigation.current = false
          dialog.current?.removeAttribute("data-keyboard-navigation")
        }}
        onTouchStart={() => {
          keyboardNavigation.current = false
          dialog.current?.removeAttribute("data-keyboard-navigation")
        }}
        onScroll={(event) => {
          const node = event.currentTarget
          if (
            !sessions.error &&
            canShowMore &&
            node.scrollHeight - node.scrollTop - node.clientHeight < node.clientHeight / 2
          )
            sessionSearch.more()
        }}
      >
        <div
          id={`${id}-results`}
          role="listbox"
          aria-label="Search results"
          aria-busy={pending}
          className="p-2"
        >
          {groups.map((group, index) => (
            <div
              key={`${index}-${group.label}`}
              role="group"
              aria-labelledby={`${id}-group-${index}`}
            >
              <div
                id={`${id}-group-${index}`}
                role="presentation"
                className="px-3 pb-1 pt-3 type-caption text-label-secondary"
              >
                {group.label === "Sessions" && group.results.some((result) => result.session)
                  ? `${group.label} · ${scopeLabel}`
                  : group.label}
              </div>
              {group.results.map((result) => (
                <div
                  key={result.id}
                  id={`${id}-${result.id}`}
                  role="option"
                  aria-selected={active?.id === result.id}
                  ref={(node) => {
                    if (!node) return
                    if (arrivals.current.query !== query)
                      arrivals.current = { query, keys: new Set() }
                    const key = searchTargetKey(result)
                    if (query.trim() && result.session && !arrivals.current.keys.has(key)) {
                      node.dataset.sessionArrival = "true"
                      arrivals.current.keys.add(key)
                    }
                    if (active?.id === result.id) {
                      deepSearch.select(selectionKey(result))
                      if (keyboardNavigation.current) node.scrollIntoView({ block: "nearest" })
                    }
                  }}
                  aria-label={
                    result.session
                      ? `${result.label}. ${result.detail}${result.evidence ? `. ${result.evidence.excerpt}` : ""}`
                      : undefined
                  }
                  className="app-search-result rounded-control px-3 py-2"
                  onPointerMove={() => {
                    keyboardNavigation.current = false
                    dialog.current?.removeAttribute("data-keyboard-navigation")
                    setSelectedId(result.id)
                    deepSearch.select(selectionKey(result))
                  }}
                  onMouseDown={(event) => event.preventDefault()}
                  onClick={() => void choose(result)}
                >
                  <span
                    title={result.label}
                    className="block w-full truncate type-body text-label"
                  >
                    {result.label}
                  </span>
                  {result.session ? (
                    <span className="flex min-w-0 items-center justify-between gap-x-3">
                      <SessionSearchMetadata entry={result.session} />
                    </span>
                  ) : result.detail !== group.label ? (
                    <span className="block type-caption text-label-secondary">
                      {result.detail}
                    </span>
                  ) : null}
                </div>
              ))}
            </div>
          ))}
        </div>
        {canShowMore && !sessions.error && (
          <div
            aria-hidden="true"
            ref={(node) => {
              if (!node || typeof IntersectionObserver === "undefined") return
              const observer = new IntersectionObserver(
                (entries) => {
                  if (entries.some((entry) => entry.isIntersecting)) sessionSearch.more()
                },
                { root: node.parentElement },
              )
              observer.observe(node)
              return () => observer.disconnect()
            }}
          />
        )}
        {query.trim() && sessions.error && (
          <div className="px-4 py-2 type-caption text-label-secondary">
            {sessions.error && <p>Session search could not finish.</p>}
            <button
              type="button"
              disabled={sessions.loading || sessions.indexing}
              className="app-search-control rounded-control px-3 text-label hover:bg-surface-hover disabled:opacity-50"
              onClick={() => {
                sessionSearch.retry()
                input.current?.focus()
              }}
            >
              Refresh sessions
            </button>
          </div>
        )}
        {contentEmpty && <DeepSearchEmptyState state={deep} scopeLabel={scopeLabel} />}
        <p
          role="status"
          className={
            results.length ||
            deep.phase !== "idle" ||
            searching ||
            (!results.length && !sessions.error)
              ? "sr-only"
              : "px-4 py-6 type-body text-label-secondary"
          }
        >
          {deep.phase !== "idle"
            ? deep.phase === "finished" &&
              deep.response?.coverage.scopeExhausted &&
              !deep.response.coverage.unavailableSessions &&
              !deep.response.coverage.changedSessions
              ? `${deep.response.totalMatchingSessions} content matches. ${scopeLabel} searched.`
              : `${deep.response?.totalMatchingSessions ?? 0} content matches so far. ${scopeLabel}: search may be incomplete.`
            : searching || (!results.length && !sessions.error)
              ? "Searching sessions…"
              : results.length
                ? `${results.length} results${sessions.error ? ". Session search is unavailable." : ""}`
                : sessions.error
                  ? "No matches. Session search is unavailable."
                  : `No matches. Sessions: ${scopeLabel.toLowerCase()}.`}
        </p>
      </div>
      {query.trim() && active?.evidence?.excerpt.trim() && (
        <section
          aria-label="Session excerpt"
          ref={(node) => {
            if (node && keyboardNavigation.current && active)
              document
                .getElementById(`${id}-${active.id}`)
                ?.scrollIntoView({ block: "nearest" })
          }}
          className="app-search-preview border-t border-separator px-4 py-2 type-callout text-label-secondary"
        >
          <div className="mb-1 flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
            <h3 className="type-callout font-semibold text-label">Why this matched?</h3>
            <span className="rounded-control bg-surface-tertiary/40 px-2 type-caption text-label-tertiary">
              {EVIDENCE_KIND_LABELS[active.evidence.kind]}
            </span>
          </div>
          <div className="app-search-inline-excerpt">
            <EvidenceExcerpt evidence={active.evidence} query={query} showSource={false} />
          </div>
        </section>
      )}
      {query.trim() &&
        deep.phase !== "idle" &&
        !(contentComplete && !results.length) &&
        (deep.showProgress ||
          ["stopped", "partial", "failed", "finished"].includes(deep.phase)) && (
          <DeepSearchPanel
            state={deep}
            scopeLabel={scopeLabel}
            onStop={deepSearch.stop}
            onContinue={deepSearch.continue}
            onRetry={() => {
              deepSearch.close()
              deepSearch.start()
            }}
          />
        )}
      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-separator px-4 py-2 type-caption text-label-secondary">
        <span>
          {contentEmpty ? "Esc to close" : "↑ ↓ to move · Enter to open · Esc to close"}
        </span>
        {query.trim() && deep.phase === "idle" && (
          <button
            type="button"
            className="app-search-control rounded-control px-2 text-label hover:bg-surface-hover"
            ref={startButtonRef}
            title={`Search messages, reasoning and tools on this device · ${scopeLabel}.`}
            onClick={() => {
              deepSearch.select(selectionKey(active))
              deepSearch.start()
              input.current?.focus()
            }}
          >
            Search session content
          </button>
        )}
      </div>
    </dialog>
  )
}
