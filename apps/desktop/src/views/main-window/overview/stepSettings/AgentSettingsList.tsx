import { Check, ChevronRight, Circle, TriangleAlert } from "lucide-react"
import { useId, useRef, useState, type KeyboardEvent } from "react"

import { Tooltip } from "../../../../components/presentation/Tooltip"
import { ToggleSwitch } from "../../../../components/ui/ToggleSwitch"
import { renderAgentIcon } from "../../../../lib/agentIcon"
import type { IntegrationStatus } from "../../../../lib/agentIntegrationsIpc"
import { cn } from "../../../../lib/cn"
import type { AgentSessionLocations } from "../../../../lib/ipc"
import { pluralize } from "../../../../lib/presentation/text"
import { agentListName, type AgentStatus } from "../../../../lib/presentation/agentStatus"
import {
  AgentConnectionDetails,
  integrationBadge,
  integrationTitle,
} from "../../../settings/AgentConnectionDetails"
import "./agent-settings.css"

type AgentTab = "locations" | "integration"

export interface AgentSettingsRow {
  slug: string
  sessions: number
  status: AgentStatus
  accountLabel?: string
  desktopAppLabel?: string | undefined
  searched: boolean
  locations: AgentSessionLocations["locations"]
  enabled: boolean
  connection: IntegrationStatus | undefined
}

export function AgentSettingsList({
  rows,
  busy,
  onOpenCli,
  onEnabledChange,
}: {
  rows: AgentSettingsRow[]
  busy: boolean
  onOpenCli: () => void
  onEnabledChange: (slug: string, enabled: boolean) => void
}) {
  const [expanded, setExpanded] = useState<{ slug: string; tab: AgentTab } | null>(null)
  return (
    <div className="agent-settings-list divide-y divide-separator">
      {rows.map((row) => (
        <AgentRow
          key={row.slug}
          row={row}
          expanded={expanded?.slug === row.slug}
          tab={expanded?.slug === row.slug ? expanded.tab : undefined}
          busy={busy}
          onOpenCli={onOpenCli}
          onToggle={() =>
            setExpanded(
              expanded?.slug === row.slug ? null : { slug: row.slug, tab: "locations" },
            )
          }
          onTab={(tab) => setExpanded({ slug: row.slug, tab })}
          onEnabledChange={(enabled) => onEnabledChange(row.slug, enabled)}
        />
      ))}
    </div>
  )
}

function AgentRow({
  row,
  expanded,
  tab,
  busy,
  onOpenCli,
  onToggle,
  onTab,
  onEnabledChange,
}: {
  row: AgentSettingsRow
  expanded: boolean
  tab: AgentTab | undefined
  busy: boolean
  onOpenCli: () => void
  onToggle: () => void
  onTab: (tab: AgentTab) => void
  onEnabledChange: (enabled: boolean) => void
}) {
  const id = useId()
  const [retainedTab, setRetainedTab] = useState<AgentTab>("locations")
  const [contentMounted, setContentMounted] = useState(expanded)
  if (expanded && !contentMounted) setContentMounted(true)
  if (tab && tab !== retainedTab) setRetainedTab(tab)
  const integrationTab = useRef<HTMLButtonElement | null>(null)
  const focusIntegration = useRef(false)
  const name = agentListName(row.slug)
  const locationSummary = row.searched
    ? `Searched ${row.locations.length} ${pluralize(row.locations.length, "location", "locations")}`
    : "Searching locations…"
  const detected =
    row.sessions > 0 || row.status.found || row.locations.some((location) => location.found)
  const accountLabel = row.accountLabel ?? row.status.note
  const accountSummary = [row.desktopAppLabel && "Desktop", accountLabel]
    .filter(Boolean)
    .join(" · ")
  const sessionSummary =
    row.sessions > 0
      ? row.status.facts
      : !row.searched
        ? "Searching…"
        : row.locations.length > 0
          ? "No sessions yet"
          : ""
  const badge = row.connection && integrationBadge(row.connection, name)
  const selectedTab = row.connection ? (tab ?? retainedTab) : "locations"
  const tabs = row.connection
    ? (["locations", "integration"] as const)
    : (["locations"] as const)

  function handleTabKey(event: KeyboardEvent<HTMLButtonElement>, index: number) {
    const direction = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0
    const next =
      event.key === "Home" ? 0 : event.key === "End" ? tabs.length - 1 : index + direction
    if (!direction && event.key !== "Home" && event.key !== "End") return
    event.preventDefault()
    const nextTab = tabs[(next + tabs.length) % tabs.length]!
    onTab(nextTab)
    event.currentTarget.parentElement
      ?.querySelector<HTMLButtonElement>(`[data-agent-tab="${nextTab}"]`)
      ?.focus()
  }

  return (
    <div className="agent-settings-item" data-agent={row.slug}>
      <div
        className={cn(
          "agent-settings-row px-3.5 py-1 hover:bg-surface-hover",
          expanded && "bg-surface-selected/50",
        )}
      >
        <button
          type="button"
          className="agent-settings-disclosure flex min-w-0 items-center gap-2.5 rounded-control py-1 text-left active:opacity-100"
          aria-label={`${expanded ? "Collapse" : "Expand"} ${name}`}
          aria-description={[accountSummary, locationSummary, sessionSummary]
            .filter(Boolean)
            .join(" · ")}
          aria-expanded={expanded}
          aria-controls={`${id}-details`}
          onClick={onToggle}
        >
          <ChevronRight
            size={12}
            aria-hidden="true"
            className={cn(
              "agent-settings-chevron shrink-0 text-label-secondary",
              expanded && "rotate-90",
            )}
          />
          <span className="flex w-4 shrink-0 justify-center">
            {renderAgentIcon(row.slug, 16)}
          </span>
          <span className="min-w-0">
            <span className="flex flex-wrap items-center gap-x-1 type-callout">
              <span className="font-medium! text-label">{name}</span>
              {accountSummary && (
                <span className="rounded-control bg-surface-card px-1.5 py-0 type-caption text-label-secondary">
                  {accountSummary}
                </span>
              )}
            </span>
            <span className="flex flex-wrap items-baseline gap-x-1 type-footnote tabular-nums text-label-tertiary">
              <span>{locationSummary}</span>
              {sessionSummary && (
                <>
                  <span aria-hidden="true">·</span>
                  <span>{sessionSummary}</span>
                </>
              )}
            </span>
          </span>
        </button>
        <div className="agent-settings-trailing">
          <span className="agent-settings-badge-slot">
            {badge?.tone === "attention" && (
              <Tooltip label={badge.description}>
                <button
                  type="button"
                  className="agent-settings-badge whitespace-nowrap rounded-control type-caption font-medium! active:opacity-100 agent-settings-badge-attention border border-system-orange/25 bg-system-orange/10 hover:bg-system-orange/15"
                  aria-label={`Open Antiburn integration for ${name}`}
                  aria-description={badge.description}
                  aria-expanded={expanded}
                  aria-controls={`${id}-details`}
                  onClick={() => {
                    focusIntegration.current = true
                    onTab("integration")
                    if (expanded && integrationTab.current) {
                      integrationTab.current.focus()
                      focusIntegration.current = false
                    }
                  }}
                >
                  <TriangleAlert size={11} strokeWidth={2.5} aria-hidden="true" />
                  <span>Integration</span>
                </button>
              </Tooltip>
            )}
          </span>
          <span className="agent-settings-switch flex justify-end">
            <ToggleSwitch
              checked={detected && row.enabled}
              disabled={!detected}
              onCheckedChange={onEnabledChange}
              aria-label={`Show ${name} sessions`}
              aria-describedby={!detected ? `${id}-unavailable` : undefined}
            />
            {!detected && (
              <span id={`${id}-unavailable`} className="sr-only">
                {row.searched
                  ? "Unavailable until Antiburn detects this agent."
                  : "Searching for this agent."}
              </span>
            )}
          </span>
        </div>
      </div>
      <div
        id={`${id}-details`}
        className="agent-settings-collapse"
        data-expanded={expanded}
        aria-hidden={!expanded}
        inert={!expanded}
        onTransitionEnd={(event) => {
          if (
            event.target === event.currentTarget &&
            event.propertyName === "grid-template-rows" &&
            !expanded
          )
            setContentMounted(false)
        }}
      >
        <div className="agent-settings-collapse-clip">
          {(expanded || contentMounted) && (
            <div className="agent-settings-details pb-5 pr-4 pt-1">
              <div
                role="tablist"
                aria-label={`${name} details`}
                className="mb-4 flex gap-5 border-b border-separator"
              >
                {tabs.map((value, index) => {
                  const button = (
                    <button
                      key={value}
                      ref={
                        value === "integration"
                          ? (node) => {
                              integrationTab.current = node
                              if (node && focusIntegration.current) {
                                node.focus()
                                focusIntegration.current = false
                              }
                            }
                          : undefined
                      }
                      type="button"
                      role="tab"
                      id={`${id}-${value}-tab`}
                      data-agent-tab={value}
                      aria-description={
                        value === "integration" && badge && row.connection
                          ? integrationTitle(row.connection)
                          : undefined
                      }
                      aria-selected={selectedTab === value}
                      aria-controls={`${id}-panel`}
                      tabIndex={selectedTab === value ? 0 : -1}
                      onClick={() => onTab(value)}
                      onKeyDown={(event) => handleTabKey(event, index)}
                      className={cn(
                        "agent-settings-tab inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap border-b-2 type-footnote font-medium! active:opacity-100",
                        selectedTab === value
                          ? "border-label text-label"
                          : "border-transparent text-label-secondary hover:text-label",
                      )}
                    >
                      {value === "locations" ? "Session locations" : "Integration"}
                      {value === "integration" && badge && (
                        <span
                          className="inline-flex w-3 shrink-0 items-center justify-center"
                          aria-hidden="true"
                        >
                          {badge?.tone === "success" ? (
                            <Check
                              size={12}
                              strokeWidth={2.5}
                              className="agent-settings-badge-success"
                            />
                          ) : badge?.tone === "attention" ? (
                            <TriangleAlert
                              size={12}
                              strokeWidth={2.5}
                              className="agent-settings-badge-attention"
                            />
                          ) : badge?.tone === "neutral" ? (
                            <Circle
                              size={12}
                              strokeWidth={2}
                              className="text-label-secondary"
                            />
                          ) : null}
                        </span>
                      )}
                    </button>
                  )
                  return value === "integration" && badge && row.connection ? (
                    <Tooltip key={value} label={integrationTitle(row.connection)}>
                      {button}
                    </Tooltip>
                  ) : (
                    button
                  )
                })}
              </div>
              <div
                role="tabpanel"
                id={`${id}-panel`}
                aria-labelledby={`${id}-${selectedTab}-tab`}
              >
                {selectedTab === "integration" && row.connection ? (
                  <AgentConnectionDetails
                    key={row.connection.vendor}
                    status={row.connection}
                    name={name}
                    busy={busy}
                    onOpenCli={onOpenCli}
                    onFocusIntegration={() => integrationTab.current?.focus()}
                  />
                ) : row.locations.length > 0 ? (
                  <ul className="space-y-1 type-caption text-label-secondary">
                    {row.locations.map((location) => (
                      <li key={location.path}>
                        <span className="select-text break-all font-mono">{location.path}</span>
                        {!location.found && (
                          <span className="text-label-tertiary"> · Not found</span>
                        )}
                      </li>
                    ))}
                  </ul>
                ) : (
                  <p className="type-footnote text-label-secondary">
                    No session locations available.
                  </p>
                )}
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
