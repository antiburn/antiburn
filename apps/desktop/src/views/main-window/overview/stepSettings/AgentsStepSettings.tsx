import { agentIntegrationsSession } from "../../../../lib/agentIntegrationsSession"
import type { IntegrationVendor } from "../../../../lib/agentIntegrationsIpc"
import { ChevronDown } from "lucide-react"
import { useCallback, useId, useState, useSyncExternalStore } from "react"

import { pluralize } from "../../../../lib/presentation/text"
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
import { createExternalStore } from "../../../../lib/externalStore"
import {
  EMPTY_LIVE_USAGE,
  getLiveUsage,
  onLiveUsageChanged,
  refreshLiveUsage,
  type AgentSessionLocations,
} from "../../../../lib/ipc"
import {
  agentListName,
  agentLoginLabel,
  agentStatus,
  meterForAgent,
  type AgentStatus,
} from "../../../../lib/presentation/agentStatus"
import { AGENT_SLUGS } from "../../../../lib/presentation/agents"
import { agentSessionCounts, subscribeAgentSessionCounts } from "./agentSessionCounts"
import { SettingsSectionGroup } from "../../../settings/SettingsSearchRows"
import { useAppSettings } from "../../../settings/useAppSettings"
import { AgentSettingsList } from "./AgentSettingsList"

/** `titled` shows the section title. The first-run step card already names it. */
const vendors: Record<string, IntegrationVendor> = {
  "claude-code": "claude",
  codex: "codex",
  cursor: "cursor",
  opencode: "open_code",
  pi: "pi",
}
function inactiveSubscription() {
  return () => undefined
}

export function AgentsStepSettings({
  titled = false,
  manageConnections = false,
  onOpenCli,
}: {
  titled?: boolean
  manageConnections?: boolean
  onOpenCli?: () => void
}) {
  const connections = useSyncExternalStore(
    manageConnections ? agentIntegrationsSession.subscribe : inactiveSubscription,
    agentIntegrationsSession.getSnapshot,
  )
  const { settings, update } = useAppSettings()
  const disabledAgents = settings.disabledAgents
  const counts = useSyncExternalStore(
    subscribeAgentSessionCounts,
    agentSessionCounts,
    agentSessionCounts,
  )
  const locations = useSyncExternalStore(
    subscribeAgentSessionLocations,
    agentSessionLocationsSnapshot,
    agentSessionLocationsSnapshot,
  )
  const locationsByAgent = new Map(
    (locations ?? []).map((entry) => [entry.agent, entry.locations]),
  )
  // Login and desktop-app detection for each agent's status. Read the cached
  // snapshot and follow updates. Opening the pane is a user action, so it
  // asks for one check. That check can refresh an expired Claude login, and
  // the login state here then agrees with the meter.
  const [liveStore] = useState(() =>
    createExternalStore({
      initial: EMPTY_LIVE_USAGE,
      load: getLiveUsage,
      subscribe: async (set) => {
        const unlisten = await onLiveUsageChanged(set)
        void refreshLiveUsage().catch(() => undefined)
        return unlisten
      },
    }),
  )
  const liveMeters = useSyncExternalStore(liveStore.subscribe, liveStore.getSnapshot).meters

  const setAgentEnabled = useCallback(
    (slug: string, enabled: boolean) => {
      const next = new Set(disabledAgents)
      if (enabled) next.delete(slug)
      else next.add(slug)
      void update({ disabledAgents: [...next].sort() })
    },
    [disabledAgents, update],
  )

  const sessionsByAgent = new Map(counts.map((row) => [row.agent, row.sessions]))
  // The scan lists every agent from its start, so a row with no sessions that
  // is not done means the agent's search is still running.
  const searchedAgents = new Set(counts.filter((row) => row.done).map((row) => row.agent))
  const agentRows = [...AGENT_SLUGS].sort(
    (a, b) => (sessionsByAgent.get(b) ?? 0) - (sessionsByAgent.get(a) ?? 0),
  )

  return (
    <SettingsSectionGroup hideTitle={!titled} searchId="sourceAgents">
      {manageConnections && (
        <p className="type-footnote text-label-secondary text-pretty">
          Switch off an agent to hide its sessions and reports. Automatic checks continue.
        </p>
      )}
      {manageConnections && connections.error && (
        <p className="type-footnote text-system-red-text" role="alert">
          {connections.error}
        </p>
      )}
      <Card>
        {!manageConnections && (
          <ToggleListIntro>
            Switching off an agent hides its sessions from the list and reports.
          </ToggleListIntro>
        )}

        {manageConnections ? (
          <AgentSettingsList
            rows={agentRows.map((slug) => ({
              slug,
              accountLabel: agentLoginLabel(meterForAgent(slug, liveMeters)),
              desktopAppLabel: meterForAgent(slug, liveMeters)?.desktopAppLabel,
              connection: connections.statuses.find((entry) => entry.vendor === vendors[slug]),
              sessions: sessionsByAgent.get(slug) ?? 0,
              status: agentStatus(
                sessionsByAgent.get(slug) ?? 0,
                meterForAgent(slug, liveMeters),
              ),
              searched: searchedAgents.has(slug),
              locations: locationsByAgent.get(slug) ?? [],
              enabled: !disabledAgents.includes(slug),
            }))}
            busy={connections.busy}
            onOpenCli={onOpenCli ?? (() => undefined)}
            onEnabledChange={setAgentEnabled}
          />
        ) : (
          <ToggleList>
            {agentRows.map((slug) => (
              <AgentRow
                key={slug}
                slug={slug}
                sessions={sessionsByAgent.get(slug) ?? 0}
                status={agentStatus(
                  sessionsByAgent.get(slug) ?? 0,
                  meterForAgent(slug, liveMeters),
                )}
                searched={searchedAgents.has(slug)}
                locations={locationsByAgent.get(slug) ?? []}
                enabled={!disabledAgents.includes(slug)}
                onEnabledChange={(next) => setAgentEnabled(slug, next)}
              />
            ))}
          </ToggleList>
        )}
      </Card>
    </SettingsSectionGroup>
  )
}

function AgentRow({
  slug,
  sessions,
  status,
  searched,
  locations,
  enabled,
  onEnabledChange,
}: {
  slug: string
  sessions: number
  status: AgentStatus
  searched: boolean
  locations: AgentSessionLocations["locations"]
  enabled: boolean
  onEnabledChange: (enabled: boolean) => void
}) {
  const [locationsOpen, setLocationsOpen] = useState(false)
  const locationsId = useId()
  return (
    <ToggleListRow
      icon={renderAgentIcon(slug, 15)}
      name={agentListName(slug)}
      detail={
        locations.length > 0 && (
          <button
            type="button"
            aria-expanded={locationsOpen}
            aria-controls={locationsId}
            onClick={() => setLocationsOpen(!locationsOpen)}
            className="relative inline-flex items-center gap-1 type-footnote text-label-secondary before:absolute before:inset-x-0 before:-inset-y-3 before:content-[''] hover:text-label active:opacity-100"
          >
            {`Searched ${locations.length} ${pluralize(locations.length, "location", "locations")}`}
            <ChevronDown
              size={12}
              strokeWidth={2}
              aria-hidden="true"
              className={cn(
                "transition-transform duration-[var(--duration-fast)] ease-out",
                locationsOpen && "rotate-180",
              )}
            />
          </button>
        )
      }
      facts={status.facts}
      controls={
        sessions > 0 && (
          <ToggleSwitch
            checked={enabled}
            onCheckedChange={onEnabledChange}
            aria-label={`Show ${agentListName(slug)} sessions`}
          />
        )
      }
      status={sessions === 0 && searched && "No sessions"}
    >
      {status.note && <p className="type-footnote text-label-secondary">{status.note}</p>}
      {locationsOpen && (
        <div id={locationsId} className="pt-1">
          <ul className="overflow-x-auto">
            {locations.map((location) => (
              <li
                key={location.path}
                className="font-mono type-caption whitespace-nowrap text-label-tertiary"
              >
                <span className={location.found ? undefined : "opacity-60"}>
                  {location.path}
                </span>
                {!location.found && <span className="font-sans"> · Not found</span>}
              </li>
            ))}
          </ul>
        </div>
      )}
    </ToggleListRow>
  )
}
