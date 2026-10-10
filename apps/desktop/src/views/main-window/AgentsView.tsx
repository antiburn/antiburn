import { useSyncExternalStore } from "react"
import { Settings } from "lucide-react"
import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { renderAgentIcon } from "../../lib/agentIcon"
import { relativeTime } from "../../lib/presentation/relativeTime"
import { overviewProgress } from "./overview/overviewProgressStore"
import { subscribeAgentsProgress } from "./overview/agentsProgress"

function subscribeInactive() {
  return () => undefined
}

export function AgentsView({
  active,
  entries,
  onSettings,
  onSessions,
}: {
  active: boolean
  entries: readonly SessionListEntry[]
  onSettings: () => void
  onSessions: (agent: string) => void
}) {
  const progress = useSyncExternalStore(
    active ? subscribeAgentsProgress : subscribeInactive,
    overviewProgress,
    overviewProgress,
  )
  const found = progress.agents.rows.filter((row) => row.sessions > 0)
  const other = progress.agents.rows.filter((row) => row.sessions === 0)
  return (
    <ScrollPane className="h-full">
      <div className="mx-auto flex max-w-3xl flex-col gap-(--space-lg) p-(--space-xl)">
        <header className="flex items-start justify-between gap-(--space-md)">
          <div>
            <h1 className="type-title-2 text-label">Agents</h1>
            <p className="mt-1 type-callout text-label-secondary">
              Coding agents found in your recent local session logs.
            </p>
          </div>
          <button
            type="button"
            onClick={onSettings}
            className="ui-push-button"
            aria-label="Agent settings"
          >
            <Settings size={16} aria-hidden="true" /> <span className="ml-2">Settings</span>
          </button>
        </header>
        {found.length === 0 && (
          <p role="status" className="type-body text-label-secondary">
            {progress.agents.done
              ? "No agents found in recent session logs."
              : "Looking for agents…"}
          </p>
        )}
        <div className="flex flex-col divide-y divide-separator">
          {found.map((row) => {
            const recent = entries
              .filter((entry) => entry.agent === row.agent && !entry.remoteHostId)
              .sort((a, b) => b.timestamp.localeCompare(a.timestamp))[0]
            return (
              <article
                key={row.agent}
                className="flex items-center gap-(--space-md) py-(--space-lg)"
              >
                {renderAgentIcon(row.agent, 28)}
                <div className="min-w-0 flex-1">
                  <h2 className="type-headline text-label">{row.label}</h2>
                  <p className="type-callout text-label-secondary">
                    {row.sessions.toLocaleString()} sessions discovered
                    {recent ? ` · Last active ${relativeTime(recent.timestamp)}` : ""}
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => onSessions(row.agent)}
                  className="ui-push-button"
                  aria-label={`View ${row.label} sessions`}
                >
                  View sessions
                </button>
              </article>
            )
          })}
        </div>
        {other.length > 0 && (
          <details className="type-callout text-label-secondary">
            <summary className="cursor-pointer">Other agents checked ({other.length})</summary>
            <ul className="mt-(--space-md) flex flex-col gap-(--space-sm)">
              {other.map((row) => (
                <li key={row.agent} className="flex items-center gap-(--space-sm)">
                  {renderAgentIcon(row.agent, 16)}
                  {row.label}
                  <span className="ml-auto text-label-tertiary">
                    {row.done ? "No recent sessions" : "Checking…"}
                  </span>
                </li>
              ))}
            </ul>
          </details>
        )}
      </div>
    </ScrollPane>
  )
}
