import { ChevronDown } from "lucide-react"
import { useCallback, useId, useState, useSyncExternalStore } from "react"

import { Card } from "../../../../components/ui/Card"
import {
  ToggleList,
  ToggleListIntro,
  ToggleListRow,
} from "../../../../components/ui/ToggleList"
import { ToggleSwitch } from "../../../../components/ui/ToggleSwitch"
import {
  agentSessionLocationsSnapshot,
  subscribeAgentSessionLocations,
} from "../../../../lib/agentSessionLocationsStore"
import { renderAgentIcon } from "../../../../lib/agentIcon"
import { cn } from "../../../../lib/cn"
import type { AgentSessionLocations } from "../../../../lib/ipc"
import { AGENT_SLUGS, agentDisplayName } from "../../../../lib/presentation/agents"
import { overviewProgress, subscribeOverviewProgress } from "../overviewProgressStore"
import { StepSettingsSectionGroup } from "./StepSettingsSearchRows"
import { useAppSettings } from "../../../settings/useAppSettings"

export function AgentsStepSettings() {
  const { settings, update } = useAppSettings()
  const disabledAgents = settings.disabledAgents
  const progress = useSyncExternalStore(
    subscribeOverviewProgress,
    overviewProgress,
    overviewProgress,
  )
  const locations = useSyncExternalStore(
    subscribeAgentSessionLocations,
    agentSessionLocationsSnapshot,
    agentSessionLocationsSnapshot,
  )
  const locationsByAgent = new Map(
    (locations ?? []).map((entry) => [entry.agent, entry.locations]),
  )

  const setAgentEnabled = useCallback(
    (slug: string, enabled: boolean) => {
      const next = new Set(disabledAgents)
      if (enabled) next.delete(slug)
      else next.add(slug)
      void update({ disabledAgents: [...next].sort() })
    },
    [disabledAgents, update],
  )

  const sessionsByAgent = new Map(progress.agents.rows.map((row) => [row.agent, row.sessions]))
  // The scan lists every agent from its start, so a row with no sessions that
  // is not done means the agent's search is still running.
  const searchedAgents = new Set(
    progress.agents.rows.filter((row) => row.done).map((row) => row.agent),
  )
  const agentRows = [...AGENT_SLUGS].sort(
    (a, b) => (sessionsByAgent.get(b) ?? 0) - (sessionsByAgent.get(a) ?? 0),
  )

  return (
    <StepSettingsSectionGroup hideTitle searchId="sourceAgents">
      <Card>
        <ToggleListIntro>
          Switching off an agent hides its sessions from the list and reports.
        </ToggleListIntro>

        <ToggleList>
          {agentRows.map((slug) => (
            <AgentRow
              key={slug}
              slug={slug}
              sessions={sessionsByAgent.get(slug) ?? 0}
              searched={searchedAgents.has(slug)}
              locations={locationsByAgent.get(slug) ?? []}
              enabled={!disabledAgents.includes(slug)}
              onEnabledChange={(next) => setAgentEnabled(slug, next)}
            />
          ))}
        </ToggleList>
      </Card>
    </StepSettingsSectionGroup>
  )
}

function AgentRow({
  slug,
  sessions,
  searched,
  locations,
  enabled,
  onEnabledChange,
}: {
  slug: string
  sessions: number
  /** Whether the agent's search has finished. */
  searched: boolean
  locations: AgentSessionLocations["locations"]
  enabled: boolean
  onEnabledChange: (enabled: boolean) => void
}) {
  const [open, setOpen] = useState(false)
  const listId = useId()

  return (
    <ToggleListRow
      icon={renderAgentIcon(slug, 15)}
      name={agentDisplayName(slug)}
      detail={
        locations.length > 0 && (
          <button
            type="button"
            aria-expanded={open}
            aria-controls={listId}
            onClick={() => setOpen((value) => !value)}
            className="inline-flex items-center gap-1 type-footnote text-label-secondary hover:text-label-tertiary active:opacity-100"
          >
            Searched {locations.length} {locations.length === 1 ? "location" : "locations"}
            <ChevronDown
              size={12}
              strokeWidth={2}
              aria-hidden="true"
              className={cn(
                "transition-transform duration-[var(--duration-fast)] ease-out",
                open && "rotate-180",
              )}
            />
          </button>
        )
      }
      facts={sessions > 0 && `${sessions} ${sessions === 1 ? "session" : "sessions"}`}
      controls={
        sessions > 0 && (
          <ToggleSwitch
            checked={enabled}
            onCheckedChange={onEnabledChange}
            aria-label={`Show ${agentDisplayName(slug)} sessions`}
          />
        )
      }
      status={sessions === 0 && searched && <span className="opacity-60">Not found</span>}
    >
      {open && (
        <ul id={listId} className="overflow-x-auto pt-1">
          {locations.map((location) => (
            <li
              key={location.path}
              className="font-mono type-caption whitespace-nowrap text-label-tertiary"
            >
              <span className={location.found ? undefined : "opacity-60"}>{location.path}</span>
              {!location.found && <span className="font-sans"> · Not found</span>}
            </li>
          ))}
        </ul>
      )}
    </ToggleListRow>
  )
}
